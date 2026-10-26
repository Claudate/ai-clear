//! `molt` 命令行。四个动作，对应「看清 → 决定 → 收 → 后悔」。
//!
//! 刻意不做「一键全清」：默认动作是扫描（只读），
//! `clean` 必须显式加 `--apply`，非交互终端直接拒绝。
//! 这些不是防呆，是把 cpa 那条硬规则（先盘点、确认后才行）落成代码。

mod auto;
mod paths;
mod render;
mod scan;
mod schedule;

use clap::{Parser, Subcommand};
use molt_core::Verdict;
use std::io::{IsTerminal, Write};

#[derive(Parser)]
#[command(
    name = "molt",
    version,
    about = "净灵 · Cleanling —— AI 与开发工具的缓存清理器"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 只读扫描，列出所有候选与判定，不动任何文件
    Scan {
        /// 连受保护项也列出来（默认隐藏）
        #[arg(long)]
        all: bool,
        /// 机器可读输出
        #[arg(long)]
        json: bool,
        /// 跳过进程占用检查（更快，但不知道谁在用）
        #[arg(long)]
        no_lock_check: bool,
    },
    /// 移进隔离区。不加 --apply 只演练
    Clean {
        /// 真的执行。不加这个参数时只显示会做什么
        #[arg(long)]
        apply: bool,
        /// 连「有代价」的项也一起清（默认只清可回收项）
        #[arg(long)]
        include_costly: bool,
    },
    /// 把隔离区里的东西放回原处
    Restore {
        /// 待清理列表里的编号
        id: i64,
    },
    /// 查看历史与待清理列表
    History {
        /// 显示多少条
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 清掉隔离期满的内容
    Purge,
    /// 无人值守清理：只动规则里声明了 auto 的可回收项。给定时任务用
    Auto {
        /// 真的执行。不加这个参数时只显示会做什么
        #[arg(long)]
        apply: bool,
    },
    /// 管理定时任务（launchd）
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
}

#[derive(Subcommand)]
enum ScheduleAction {
    /// 安装为每周日 03:00 执行 molt auto
    Install,
    /// 移除定时任务
    Uninstall,
    /// 看看现在装没装
    Status,
}

fn main() {
    let cli = Cli::parse();
    let home = paths::home_dir();
    let home_str = home.to_string_lossy().to_string();

    let rules = match molt_rules::load(&home_str, Some(&paths::user_rules_dir())) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("规则库加载失败：{e}");
            std::process::exit(1);
        }
    };

    match cli.command {
        Command::Scan {
            all,
            json,
            no_lock_check,
        } => {
            let report = scan::run(&rules, &home_str, !no_lock_check);
            if json {
                render::print_json(&report);
            } else {
                render::print_report(&report, all);
            }
        }

        Command::Clean {
            apply,
            include_costly,
        } => {
            let report = scan::run(&rules, &home_str, true);
            let mut selected: Vec<&scan::ScanItem> = report
                .by_verdict(|v| *v == Verdict::Reclaimable)
                .into_iter()
                .collect();
            if include_costly {
                selected.extend(report.by_verdict(|v| matches!(v, Verdict::Costly { .. })));
            }

            if selected.is_empty() {
                println!("没有可清理的项。");
                return;
            }

            print_plan(&selected);

            if !apply {
                println!(
                    "\n只是演练，什么都没动。确认无误后加 --apply 执行。\n\
                     执行后内容会移进隔离区，{} 天内都可以用 molt restore 放回去。",
                    paths::DEFAULT_RETAIN_DAYS
                );
                return;
            }

            // 非交互终端拒绝执行：脚本里误跑 `clean --apply` 是最危险的情况
            if !std::io::stdin().is_terminal() {
                eprintln!("拒绝在非交互环境执行。清理需要有人在现场确认。");
                std::process::exit(2);
            }
            if !confirm(selected.len()) {
                println!("已取消。");
                return;
            }

            let store = match molt_act::AuditStore::open(&paths::audit_db()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("打开审计库失败：{e}");
                    std::process::exit(1);
                }
            };
            let batch = molt_act::now_iso8601().replace(':', "-");
            let q = molt_act::Quarantine::new(paths::quarantine_root(), batch);
            let exec = molt_act::Executor::new(store, q, paths::DEFAULT_RETAIN_DAYS);

            let candidates: Vec<molt_core::Candidate> =
                selected.iter().map(|i| i.candidate.clone()).collect();
            let confirmation = molt_act::Confirmation::new(&candidates);
            let outcome = exec.clean(&candidates, &confirmation);

            println!();
            for m in &outcome.moved {
                println!(
                    "  \x1b[32m✓\x1b[0m 已移入隔离区 #{}  {}  ({})",
                    m.id,
                    paths::display_path(&m.original),
                    molt_scan::human_bytes(m.bytes)
                );
            }
            for f in &outcome.failed {
                println!(
                    "  \x1b[33m!\x1b[0m 跳过 {}：{}",
                    paths::display_path(&f.path),
                    f.reason
                );
            }
            println!(
                "\n共腾出 {}。{} 天内可用 molt restore <编号> 放回。",
                molt_scan::human_bytes(outcome.freed_bytes()),
                paths::DEFAULT_RETAIN_DAYS
            );
        }

        Command::Restore { id } => {
            let store = match molt_act::AuditStore::open(&paths::audit_db()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("打开审计库失败：{e}");
                    std::process::exit(1);
                }
            };
            let q = molt_act::Quarantine::new(paths::quarantine_root(), "restore");
            let exec = molt_act::Executor::new(store, q, paths::DEFAULT_RETAIN_DAYS);
            match exec.restore(id) {
                Ok(p) => println!("已还原到 {}", paths::display_path(&p)),
                Err(e) => {
                    eprintln!("还原失败：{e}");
                    std::process::exit(1);
                }
            }
        }

        Command::History { limit } => {
            let store = match molt_act::AuditStore::open(&paths::audit_db()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("打开审计库失败：{e}");
                    std::process::exit(1);
                }
            };
            let pending = store.list_pending(limit).unwrap_or_default();
            if pending.is_empty() {
                println!("隔离区里没有待还原的内容。");
            } else {
                println!("待还原（{} 天内可 restore）：", paths::DEFAULT_RETAIN_DAYS);
                for r in &pending {
                    println!(
                        "  #{:<4} {}  {}  {}",
                        r.id,
                        molt_scan::human_bytes(r.bytes),
                        r.performed_at,
                        paths::display_path(&r.absolute_path)
                    );
                }
            }
        }

        Command::Purge => {
            let store = match molt_act::AuditStore::open(&paths::audit_db()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("打开审计库失败：{e}");
                    std::process::exit(1);
                }
            };
            let q = molt_act::Quarantine::new(paths::quarantine_root(), "purge");
            let exec = molt_act::Executor::new(store, q, paths::DEFAULT_RETAIN_DAYS);
            let outcome = exec.purge_due();
            println!(
                "已清除 {} 项到期的隔离内容，释放 {}。",
                outcome.deleted.len(),
                molt_scan::human_bytes(outcome.deleted_bytes())
            );
            for f in &outcome.failed {
                eprintln!("  未清除：{}（{}）", f.path.display(), f.reason);
            }
        }

        Command::Auto { apply } => {
            run_auto(&rules, &home_str, apply);
        }

        Command::Schedule { action } => match action {
            ScheduleAction::Install => schedule_install(),
            ScheduleAction::Uninstall => schedule_uninstall(),
            ScheduleAction::Status => schedule_status(),
        },
    }
}

/// `molt auto`：无人值守清理。
///
/// 与 `clean` 的关键区别：**不问人**。所以入选条件必须窄到可以一句话说完，
/// 而且条件本身在 `auto::select` 里单测过。
fn run_auto(rules: &molt_core::RuleSet, home: &str, apply: bool) {
    // 定时任务里没有人能回答「有进程正在用这条缓存，还要删吗」——交给闸门直接排除
    let report = scan::run(rules, home, true);
    let selected = auto::select(&report, rules);
    let blocked = auto::blocked_count(&report, rules);

    if selected.is_empty() {
        println!("没有可自动清理的项。");
        if blocked > 0 {
            println!(
                "{} 项可回收内容需要你亲自看过才能清（规则里没有声明 auto）：molt scan",
                blocked
            );
        }
        return;
    }

    let total: u64 = selected
        .iter()
        .map(|i| i.candidate.alloc_bytes.max(i.candidate.bytes))
        .sum();

    println!(
        "自动清理：{} 项，{}（只含规则声明了 auto 的可回收项）",
        selected.len(),
        molt_scan::human_bytes(total)
    );
    if blocked > 0 {
        println!(
            "另有 {} 项可回收内容未动——它们需要人看过：molt scan",
            blocked
        );
    }

    if !apply {
        println!("\n只是演练。加 --apply 执行。");
        return;
    }

    // 在无人值守路径上不弹交互确认，用规则筛选 + 审计来源标记代替
    let store = match molt_act::AuditStore::open(&paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("打开审计库失败：{e}");
            std::process::exit(1);
        }
    };
    let batch = molt_act::now_iso8601().replace(':', "-");
    let q = molt_act::Quarantine::new(paths::quarantine_root(), format!("auto-{batch}"));
    let exec = molt_act::Executor::new(store, q, paths::DEFAULT_RETAIN_DAYS)
        .with_source(molt_act::CleanSource::Auto);

    let candidates: Vec<molt_core::Candidate> =
        selected.iter().map(|i| i.candidate.clone()).collect();
    let confirmation = molt_act::Confirmation::for_auto(&candidates, rules);

    if confirmation.is_empty() {
        println!("没有通过自动清理闸门的项。");
        return;
    }

    let outcome = exec.clean(&candidates, &confirmation);

    // 无人值守路径顺带排空到期的隔离项：到期的东西本来就该被清掉，
    // 而定时任务是唯一不需要有人守着的时机。放在 clean 之后，
    // 刚移进去的这批（保留 7 天）绝不会被同一次运行立刻删掉。
    let purged = exec.purge_due();

    for m in &outcome.moved {
        println!(
            "  已移入隔离区 #{}  {}  ({})",
            m.id,
            paths::display_path(&m.original),
            molt_scan::human_bytes(m.bytes)
        );
    }
    for f in &outcome.failed {
        println!("  跳过 {}：{}", paths::display_path(&f.path), f.reason);
    }
    println!(
        "\n共移走 {}。{} 天内可用 molt restore <编号> 放回。",
        molt_scan::human_bytes(outcome.freed_bytes()),
        paths::DEFAULT_RETAIN_DAYS
    );
    println!(
        "本次排空到期隔离项 {} 项，真正释放 {}。",
        purged.deleted.len(),
        molt_scan::human_bytes(purged.deleted_bytes())
    );
}

fn schedule_install() {
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("找不到自己的可执行文件路径，无法安装定时任务。");
        std::process::exit(1);
    };

    let home = paths::home_dir();
    let log_dir = schedule::log_dir(&paths::app_dir());
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("创建日志目录失败：{e}");
        std::process::exit(1);
    }

    let plist_path = schedule::plist_path(&home);
    if let Some(parent) = plist_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("创建 LaunchAgents 目录失败：{e}");
            std::process::exit(1);
        }
    }

    let text = schedule::plist(&exe, &log_dir);

    // 顺序很重要：**必须先 bootout 再写**。
    // 已被 launchd 注册的 plist 会被系统保护，直接覆盖会得到 EPERM——
    // 用户看到的是「写入失败」，而真实原因只是文件还注册着。
    let domain = format!("gui/{}", current_uid());
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &domain, &plist_path.display().to_string()])
        .output();

    if let Err(e) = write_plist_with_retry(&plist_path, &text) {
        eprintln!("写入 plist 失败：{e}");
        eprintln!("路径：{}", plist_path.display());
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            eprintln!(
                "如果这个文件已经存在且删不掉，先用 Finder 或 `sudo rm` 删除后重试；\n\
                 macOS 会保护仍然注册在 launchd 里的登录项。"
            );
        }
        std::process::exit(1);
    }

    let out = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &plist_path.display().to_string()])
        .output();

    match out {
        Ok(o) if o.status.success() => {
            println!("已安装定时任务：每周日 03:00 执行 molt auto");
            println!("配置：{}", plist_path.display());
            println!("日志：{}", log_dir.join("auto.log").display());
            println!("卸载：molt schedule uninstall");
        }
        Ok(o) => {
            eprintln!(
                "plist 已写入，但 launchctl 加载失败：{}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            eprintln!("可以手动加载：launchctl load {}", plist_path.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("无法调用 launchctl：{e}");
            std::process::exit(1);
        }
    }
}

/// 写 plist，带退避重试。
///
/// 为什么需要重试：`launchctl bootout` 返回之后，系统还要过几十毫秒才真正释放
/// 对这个文件的保护。这段时间里写入会得到 EPERM——看起来像权限问题，
/// 其实只是时机。用户在「改配置再重装」时必然会撞上。
fn write_plist_with_retry(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut last = None;
    for attempt in 0..8 {
        match std::fs::write(path, text) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(60 * (attempt + 1)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "写入 plist 失败")
    }))
}

fn schedule_uninstall() {
    let plist_path = schedule::plist_path(&paths::home_dir());
    let _ = std::process::Command::new("launchctl")
        .args([
            "bootout",
            &format!("gui/{}", current_uid()),
            &plist_path.display().to_string(),
        ])
        .output();

    match std::fs::remove_file(&plist_path) {
        Ok(()) => println!("已移除定时任务。"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("定时任务本来就没装。")
        }
        Err(e) => {
            eprintln!("移除 plist 失败：{e}");
            std::process::exit(1);
        }
    }
}

fn schedule_status() {
    let plist_path = schedule::plist_path(&paths::home_dir());
    if !plist_path.exists() {
        println!("定时任务：未安装");
        println!("安装：molt schedule install");
        return;
    }
    println!("定时任务：已安装");
    println!("配置：{}", plist_path.display());
    println!("计划：每周日 03:00（launchd 在休眠错过后唤醒时补跑）");

    let out = std::process::Command::new("launchctl")
        .args([
            "print",
            &format!("gui/{}/{}", current_uid(), schedule::LABEL),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => println!("launchd：已加载"),
        _ => println!("launchd：尚未加载（可执行 molt schedule install 重新装）"),
    }
}

/// 当前用户的 uid。launchd 的 gui 域要用它拼 `gui/<uid>`。
///
/// 用 `id -u` 而不读环境变量：这个值必须与登录会话一致，
/// 而 `SUDO_UID` 之类在定时任务的环境里根本不存在。
fn current_uid() -> u32 {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(501)
}

/// 打印「将要做什么」，按类别分组，并在最后给出总量。
fn print_plan(items: &[&scan::ScanItem]) {
    use std::collections::BTreeMap;
    let mut by_rule: BTreeMap<String, (u64, usize)> = BTreeMap::new();

    for i in items {
        let entry = by_rule
            .entry(i.discovered.rule_id.clone())
            .or_insert((0, 0));
        entry.0 += i.candidate.alloc_bytes.max(i.candidate.bytes);
        entry.1 += 1;
    }

    println!("\n将要移入隔离区：");
    for (rule, (bytes, count)) in &by_rule {
        println!(
            "  {:<22} {:>10}  ({} 项)",
            rule,
            molt_scan::human_bytes(*bytes),
            count
        );
    }
    let total: u64 = items
        .iter()
        .map(|i| i.candidate.alloc_bytes.max(i.candidate.bytes))
        .sum();
    println!("  {:<22} {:>10}", "合计", molt_scan::human_bytes(total));
}

/// 要求用户敲 `yes`。不做「按回车继续」——那太容易误触。
fn confirm(count: usize) -> bool {
    print!("\n确认把以上 {count} 项移入隔离区？输入 yes 继续：");
    let _ = std::io::stdout().flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    input.trim().eq_ignore_ascii_case("yes")
}
