//! 定时清理：什么时候跑、清哪些类型、上次跑成什么样。
//!
//! 这一层与 `molt auto` 共用同一条安全边界，但选择范围由用户显式勾选：
//! 只有规则库里标了 `auto = true` 且判定为可回收的项才可能入选。
//! 界面上列出的「类型」就是这些规则本身——用户勾的是「npm 下载缓存」这样的名字，
//! 而不是一个自己发明的分类，因此界面文案与规则库不会各自漂移。
//!
//! 调度用 launchd 而不是 cron（理由与 CLI 版一致）：macOS 上 cron 需要按可执行
//! 文件路径单独授权「完全磁盘访问权限」，`cargo build` 换个路径就得重新授权。
//!
//! **定时任务调用的是本程序自己**（`<当前可执行文件> --run-schedule`），
//! 不是另找一个二进制。这样「窗口里勾选的类型」与「夜里真正清理的东西」
//! 用的是同一份实现，不会出现两边各有一套选择逻辑。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use serde::{Deserialize, Serialize};

use crate::worker::{CleanProgress, CleanReport, Progress, ScanResult};

/// 每天还是每周。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Daily,
    Weekly,
}

impl Frequency {
    pub fn label(self) -> &'static str {
        match self {
            Frequency::Daily => "每天",
            Frequency::Weekly => "每周",
        }
    }
}

/// launchd 的 label。与 CLI 安装的 `app.molt.autoclean` 刻意不同：
/// 两个任务同时存在会各清一遍，而用户只会看到一个开关。
pub const LABEL: &str = "app.molt.schedule";

/// 星期名。`0` 是周日，与 launchd 的 `Weekday` 一致。
pub const WEEKDAY_NAMES: [&str; 7] = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleConfig {
    pub schema_version: u32,
    /// 用户是否希望它生效。`false` 时不装 launchd 任务。
    pub enabled: bool,
    pub frequency: Frequency,
    /// 0 = 周日。`Frequency::Daily` 时忽略。
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    /// 参与定时清理的规则 id。空表示「这些类型都不清」——
    /// 不解释成「全部」，否则用户清空勾选后反而会清得更多。
    pub rule_ids: Vec<String>,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            // 默认关闭：无人值守的写盘不该在用户没点过任何开关时发生
            enabled: false,
            frequency: Frequency::Daily,
            weekday: 0,
            hour: 3,
            minute: 0,
            rule_ids: Vec::new(),
        }
    }
}

/// 时间是否合法。非法值不能写进 plist——launchd 对越界字段的反应是静默不加载，
/// 用户会以为「定时任务装好了」，实际永远不跑。
pub fn validate(cfg: &ScheduleConfig) -> Result<(), String> {
    if cfg.hour > 23 {
        return Err(format!("小时必须在 0–23 之间，当前 {}", cfg.hour));
    }
    if cfg.minute > 59 {
        return Err(format!("分钟必须在 0–59 之间，当前 {}", cfg.minute));
    }
    if cfg.weekday > 6 {
        return Err(format!("星期必须在 0–6 之间，当前 {}", cfg.weekday));
    }
    Ok(())
}

impl ScheduleConfig {
    /// 「每天 03:00」/「每周日 03:00」。
    pub fn describe(&self) -> String {
        match self.frequency {
            Frequency::Daily => format!("每天 {:02}:{:02}", self.hour, self.minute),
            Frequency::Weekly => format!(
                "每{} {:02}:{:02}",
                WEEKDAY_NAMES[(self.weekday as usize).min(6)],
                self.hour,
                self.minute
            ),
        }
    }

    pub fn is_selected(&self, rule_id: &str) -> bool {
        self.rule_ids.iter().any(|r| r == rule_id)
    }
}

/// 配置路径。与 `ai.toml` 同一目录，用户只需要记一个地方。
pub fn config_path() -> PathBuf {
    crate::paths::home_dir()
        .join(".config")
        .join("molt")
        .join("schedule.toml")
}

pub fn load_config() -> Result<ScheduleConfig, String> {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("定时配置格式有误：{e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ScheduleConfig::default()),
        Err(e) => Err(format!("读不到定时配置：{e}")),
    }
}

pub fn save_config(cfg: &ScheduleConfig) -> Result<(), String> {
    validate(cfg)?;
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建配置目录失败：{e}"))?;
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败：{e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写配置失败：{e}"))
}

/// 上次执行的结果。写成文件而不是塞进审计库：
/// 它回答的是「这个定时任务最近一次跑成什么样」，与「哪条文件被动了」是两件事，
/// 混进审计表会让历史页出现一条无法还原的假记录。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LastRun {
    pub at: String,
    /// 是否整体成功（0 项失败且没有致命错误）
    pub ok: bool,
    /// 用户勾选了几个类型
    pub types: usize,
    /// 其中真正生效的类型数（勾了但规则里没标 auto 的不算）
    pub types_effective: usize,
    /// 扫描到的候选条数（已按类型与 auto 过滤）
    pub candidates: usize,
    pub done: usize,
    pub bytes: u64,
    pub failures: Vec<(String, String)>,
    /// 一句人话结论
    pub message: String,
}

pub fn last_run_path() -> PathBuf {
    molt_act::paths::app_dir().join("schedule-last.json")
}

pub fn load_last_run() -> Option<LastRun> {
    let text = std::fs::read_to_string(last_run_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_last_run(run: &LastRun) -> Result<(), String> {
    let path = last_run_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(run).map_err(|e| format!("序列化结果失败：{e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写结果失败：{e}"))
}

/// 一个可被定时清理的类型。由规则库派生，不是另写一份清单。
#[derive(Debug, Clone)]
pub struct ScheduleKind {
    pub rule_id: String,
    /// 给人看的名字，来自规则里的 `what`
    pub what: String,
    /// 归到哪一组（AI 工具 / 语言工具链 / 其它）
    pub group: &'static str,
}

/// 从规则库列出「允许无人值守清理」的类型。
///
/// 两个条件缺一不可：规则声明了 `auto = true`，判定是可回收。
/// 后者在规则加载期已经被强制过，这里再确认一次——界面上的勾选项
/// 就是安全边界本身，不能靠上游的约定。
pub fn kinds(rules: &molt_core::RuleSet) -> Vec<ScheduleKind> {
    let mut out: Vec<ScheduleKind> = rules
        .iter()
        .filter(|r| r.auto)
        .filter(|r| matches!(r.verdict, molt_core::RuleVerdict::Reclaimable))
        .map(|r| ScheduleKind {
            rule_id: r.id.clone(),
            what: r.what.clone(),
            group: group_of(&r.id),
        })
        .collect();
    out.sort_by(|a, b| a.group.cmp(b.group).then_with(|| a.what.cmp(&b.what)));
    out
}

/// 分组只影响界面上的排列，不影响任何判定。
fn group_of(rule_id: &str) -> &'static str {
    const AI: [&str; 6] = ["codex", "claude", "cursor", "vscode", "other-ai", "ai"];
    const DEV: [&str; 8] = [
        "npm", "pnpm", "cargo", "go", "py", "gradle", "wrangler", "swift",
    ];
    if AI.iter().any(|p| rule_id.starts_with(p)) {
        "AI 工具"
    } else if DEV.iter().any(|p| rule_id.starts_with(p)) {
        "开发工具链"
    } else {
        "其它"
    }
}

/// 默认勾选：全部可用类型。首次打开开关时，用户看到的是「这些都会被清」，
/// 而不是一个空列表让他逐条找。
pub fn all_rule_ids(rules: &molt_core::RuleSet) -> Vec<String> {
    kinds(rules).into_iter().map(|k| k.rule_id).collect()
}

// ───────────────────────────── launchd ─────────────────────────────

pub fn plist_path() -> PathBuf {
    crate::paths::home_dir()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// 生成 plist。`exe` 必须是绝对路径：launchd 不展开 `~`。
pub fn plist(exe: &Path, cfg: &ScheduleConfig) -> String {
    let bin = exe.display();
    let log_dir = molt_act::paths::log_dir();
    let out = log_dir.join("schedule.log").display().to_string();
    let err = log_dir.join("schedule.err.log").display().to_string();

    // 每天 = 只写 Hour/Minute；每周 = 再加 Weekday。
    // launchd 缺字段表示「每个」，所以「每天」不能写成 Weekday 的某个默认值。
    let calendar = match cfg.frequency {
        Frequency::Daily => format!(
            "        <key>Hour</key>\n        <integer>{}</integer>\n        <key>Minute</key>\n        <integer>{}</integer>\n",
            cfg.hour, cfg.minute
        ),
        Frequency::Weekly => format!(
            "        <key>Weekday</key>\n        <integer>{}</integer>\n        <key>Hour</key>\n        <integer>{}</integer>\n        <key>Minute</key>\n        <integer>{}</integer>\n",
            cfg.weekday, cfg.hour, cfg.minute
        ),
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>

    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>--run-schedule</string>
    </array>

    <key>StartCalendarInterval</key>
    <dict>
{calendar}    </dict>

    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>

    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>

    <!-- 登录时补跑一次会让人刚开机就听到磁盘转，默认关闭 -->
    <key>RunAtLoad</key>
    <false/>
</dict>
</plist>
"#
    )
}

fn uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "501".to_string())
}

/// 任务当前是否真的被 launchd 加载着。
///
/// 只检查 plist 文件是否存在是不够的：文件在、任务没加载是很常见的状态
/// （换了机器、被系统禁用、bootstrap 失败），而界面若把它显示成「已启用」，
/// 用户会等一个永远不会来的清理。
pub fn is_loaded() -> bool {
    matches!(load_status(), Ok(true))
}

/// 同 `is_loaded`，但失败时带回 launchctl 的具体原因，界面能直接展示。
pub fn load_status() -> Result<bool, String> {
    let target = format!("gui/{}/{}", uid(), LABEL);
    let out = std::process::Command::new("launchctl")
        .args(["print", &target])
        .output()
        .map_err(|e| format!("调用 launchctl 失败：{e}"))?;
    if out.status.success() {
        Ok(true)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            "launchctl print 返回非零，任务未加载".to_string()
        } else {
            err
        })
    }
}

/// 安装（或重装）任务。已经加载时先 bootout 再 bootstrap——
/// 直接 bootstrap 一个已存在的 label 会失败，用户会看到「保存失败」。
pub fn install(cfg: &ScheduleConfig) -> Result<(), String> {
    validate(cfg)?;
    let exe = std::env::current_exe().map_err(|e| format!("取不到程序路径：{e}"))?;
    let path = plist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 LaunchAgents 目录失败：{e}"))?;
    }
    std::fs::create_dir_all(molt_act::paths::log_dir()).ok();
    std::fs::write(&path, plist(&exe, cfg)).map_err(|e| format!("写 plist 失败：{e}"))?;

    // 已加载就先卸下；失败不阻断，下面的 bootstrap 会给出真正的原因
    let target = format!("gui/{}/{}", uid(), LABEL);
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let out = std::process::Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{}", uid()),
            &path.display().to_string(),
        ])
        .output()
        .map_err(|e| format!("调用 launchctl 失败：{e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(explain_bootstrap_failure(&err, &path));
    }
    // bootstrap 返回成功不等于任务真的在跑：同一 label 冲突、plist 被系统拦截
    // 都可能让 launchd 静默丢弃。装载后必须查一次真实状态，否则界面会出现
    // 「已启用但系统里没加载」的假象。
    if !is_loaded() {
        let reason = load_status()
            .err()
            .unwrap_or_else(|| "任务未加载".to_string());
        return Err(format!("launchctl 装载后任务仍未生效：{reason}"));
    }
    Ok(())
}

/// 启动时该不该提醒「定时清理配了却不会跑」。
///
/// 返回 `Some(话)` 表示要在首页说出来。查不到配置（从没配过）就不提——
/// 一个从没开过定时清理的人不该在启动时看到一条关于它的警告。
pub fn startup_health_warning() -> Option<&'static str> {
    let cfg = load_config().ok()?;
    let loaded = load_status().is_ok();
    health_warning(cfg.enabled, loaded)
}

/// `startup_health_warning` 的判定部分，纯函数。
///
/// 抽出来是为了能在不起 launchd、不碰真实配置的前提下测这四种组合。
/// 真正的 `load_*` 调用留在外层，测试只对着这张真值表。
pub fn health_warning(enabled: bool, loaded: bool) -> Option<&'static str> {
    if enabled && !loaded {
        Some(
            "定时清理配置为已启用，但系统里没有加载这个任务，到点不会执行。\
             打开「自动清理」重新保存一次即可重新安装。",
        )
    } else {
        None
    }
}

/// 把 `launchctl bootstrap` 的失败翻译成「为什么 + 怎么办」。
///
/// 这个函数存在的唯一理由是那句 `Bootstrap failed: 5: Input/output error`：
/// 它对用户没有任何信息量，看到它的人只会以为软件坏了。实际的成因很集中，
/// 逐条对上就能告诉用户下一步该做什么。
fn explain_bootstrap_failure(raw: &str, plist: &Path) -> String {
    let low = raw.to_lowercase();
    let hint = if low.contains("input/output error") {
        format!(
            "系统拒绝了这份定时任务（launchctl 报 I/O 错误）。常见原因是这份 plist \
             已经躺在 {p} 里但从未被加载过、或者系统里的服务库暂时不接受新任务。\
             先试：关掉这个开关再打开一次；仍然不行就重启一次 Mac，再回来保存。",
            p = plist.display()
        )
    } else if low.contains("already loaded") || low.contains("service already") {
        "这个定时任务已经加载着了，不需要再装一次。先点「刷新状态」确认，\
         要换时间直接保存即可（保存会先卸旧再装新）。"
            .to_string()
    } else if low.contains("no such file") || low.contains("not found") {
        format!(
            "系统找不到这份 plist（{p}）。点「保存」会重新写一份再装；\
             仍然失败的话，检查 ~/Library/LaunchAgents 目录是否可写。",
            p = plist.display()
        )
    } else if low.contains("permission") || low.contains("denied") {
        "系统不允许写入这个用户的定时任务。检查 ~/Library/LaunchAgents 的权限，\
         或者换用有权限的账户。"
            .to_string()
    } else {
        format!("launchctl 没有接受这个任务：{raw}。可先点「刷新状态」再保存一次。")
    };
    format!("安装定时任务失败。{hint}")
}

pub fn uninstall() -> Result<(), String> {
    let target = format!("gui/{}/{}", uid(), LABEL);
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let path = plist_path();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("删除 plist 失败：{e}"))?;
    }
    Ok(())
}

// ───────────────────────────── 执行 ─────────────────────────────

/// 一次定时清理的实际执行。**同步**，只在无窗口模式或后台线程里调用。
///
/// 与界面里手动清理的三点差别：
/// 1. 只选 `auto = true` 且用户勾选的规则；
/// 2. 只走隔离区，不直接删除——没人看着的时候不做不可逆的事；
/// 3. 结果写进 `schedule-last.json`，供界面显示上次跑成什么样。
pub fn run_once(rules: &molt_core::RuleSet, home: &str, cfg: &ScheduleConfig) -> LastRun {
    let at = molt_act::now_iso8601();
    let selected: Vec<String> = cfg
        .rule_ids
        .iter()
        .filter(|id| rules.is_auto(id))
        .cloned()
        .collect();

    let mut run = LastRun {
        at,
        types: cfg.rule_ids.len(),
        types_effective: selected.len(),
        ..Default::default()
    };

    if selected.is_empty() {
        run.ok = false;
        run.message = if cfg.rule_ids.is_empty() {
            "没有选中任何可自动清理的类型，本次未执行".to_string()
        } else {
            format!(
                "选中的 {} 个类型都不允许无人值守清理（规则里没标 auto），本次未执行",
                cfg.rule_ids.len()
            )
        };
        let _ = save_last_run(&run);
        return run;
    }

    let scan = crate::worker::scan_all(rules, home, true);
    let picked: Vec<molt_core::Candidate> = scan
        .items
        .iter()
        .filter(|c| matches!(c.verdict, molt_core::Verdict::Reclaimable))
        .filter(|c| {
            c.rule_id
                .as_deref()
                .map(|id| selected.iter().any(|s| s == id))
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    run.candidates = picked.len();
    if picked.is_empty() {
        run.ok = true;
        run.message = "本次没有需要清理的内容".to_string();
        let _ = save_last_run(&run);
        return run;
    }

    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            run.ok = false;
            run.message = format!("打不开审计库，未执行：{e}");
            let _ = save_last_run(&run);
            return run;
        }
    };
    let batch = molt_act::now_iso8601().replace(':', "-");
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), batch);
    let exec = molt_act::Executor::new(store, q, crate::paths::DEFAULT_RETAIN_DAYS)
        .with_source(molt_act::CleanSource::Auto)
        .with_mode(molt_act::CleanMode::Quarantine);

    let confirmation = molt_act::Confirmation::new(&picked);
    let outcome = exec.clean(&picked, &confirmation);

    run.done = outcome.moved.len();
    run.bytes = outcome.moved.iter().map(|m| m.bytes).sum();
    run.failures = outcome
        .failed
        .iter()
        .map(|f| (f.path.display().to_string(), f.reason.clone()))
        .collect();
    run.ok = run.failures.is_empty();
    run.message = if run.done == 0 {
        format!(
            "没有清理任何内容：{}",
            run.failures
                .first()
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| "没有可处理的内容".to_string())
        )
    } else if run.failures.is_empty() {
        format!(
            "已放入临时空间 {} 项，共 {}",
            run.done,
            molt_scan::human_bytes(run.bytes)
        )
    } else {
        format!(
            "已放入临时空间 {} 项，共 {}；{} 项未处理",
            run.done,
            molt_scan::human_bytes(run.bytes),
            run.failures.len()
        )
    };
    let _ = save_last_run(&run);
    run
}

/// 无窗口模式：`molt-gui --run-schedule`。
///
/// 返回进程退出码。任何失败都写进结果文件并返回非 0，
/// 这样 launchd 的日志里也能看出「跑了但没成」而不是「没跑」。
pub fn run_headless() -> i32 {
    let home = crate::paths::home_dir().to_string_lossy().to_string();
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => {
            let run = LastRun {
                at: molt_act::now_iso8601(),
                ok: false,
                message: e,
                ..Default::default()
            };
            let _ = save_last_run(&run);
            return 2;
        }
    };
    if !cfg.enabled {
        return 0;
    }
    let rules = match molt_rules::load(&home, Some(&molt_act::paths::user_rules_dir())) {
        Ok(r) => r,
        Err(e) => {
            let run = LastRun {
                at: molt_act::now_iso8601(),
                ok: false,
                message: format!("规则库加载失败：{e}"),
                ..Default::default()
            };
            let _ = save_last_run(&run);
            return 1;
        }
    };
    let run = run_once(&rules, &home, &cfg);
    if run.ok {
        0
    } else {
        1
    }
}

/// 界面上跑一次定时清理的后台任务。
pub struct ScheduleJob {
    rx: Receiver<Result<LastRun, String>>,
    finished: bool,
    latest: Option<LastRun>,
}

impl ScheduleJob {
    pub fn start(rules: molt_core::RuleSet, home: String, cfg: ScheduleConfig) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_once(&rules, &home, &cfg)
            }));
            let msg = match ran {
                Ok(run) => Ok(run),
                Err(payload) => {
                    let what = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "未知错误".to_string());
                    Err(format!("定时清理中断：{what}"))
                }
            };
            let _ = tx.send(msg);
        });
        Self {
            rx,
            finished: false,
            latest: None,
        }
    }

    /// 返回完成的结果（若已完成）。
    pub fn poll(&mut self) -> Option<Result<LastRun, String>> {
        if self.finished {
            return None;
        }
        match self.rx.try_recv() {
            Ok(Ok(run)) => {
                self.finished = true;
                self.latest = Some(run.clone());
                Some(Ok(run))
            }
            Ok(Err(e)) => {
                self.finished = true;
                Some(Err(e))
            }
            Err(_) => None,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

/// 让 `Progress` / `CleanProgress` / `CleanReport` 的导入不落空：
/// 它们属于同类后台任务的形状，`schedule` 复用其中的扫描与清理结果类型。
#[allow(dead_code)]
fn _shape(_: Progress, _: CleanProgress, _: CleanReport, _: ScanResult) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_daily() -> ScheduleConfig {
        ScheduleConfig {
            enabled: true,
            frequency: Frequency::Daily,
            hour: 3,
            minute: 30,
            ..Default::default()
        }
    }

    #[test]
    fn daily_plist_has_no_weekday() {
        // 写成 Weekday=0 就变成「只在周日跑」，而用户以为是每天
        let text = plist(
            Path::new("/Applications/Molt.app/Contents/MacOS/molt-gui"),
            &cfg_daily(),
        );
        assert!(!text.contains("<key>Weekday</key>"), "每天不能带 Weekday");
        assert!(text.contains("<integer>3</integer>"));
        assert!(text.contains("<integer>30</integer>"));
        assert!(text.contains("--run-schedule"));
        assert!(!text.contains('~'), "launchd 不展开 ~");
    }

    #[test]
    fn weekly_plist_carries_the_weekday() {
        let cfg = ScheduleConfig {
            frequency: Frequency::Weekly,
            weekday: 5,
            hour: 22,
            minute: 15,
            ..Default::default()
        };
        let text = plist(Path::new("/bin/molt-gui"), &cfg);
        assert!(text.contains("<key>Weekday</key>\n        <integer>5</integer>"));
    }

    #[test]
    fn plist_is_valid_xml() {
        let dir = std::env::temp_dir().join(format!("molt-sched-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.plist");
        std::fs::write(&path, plist(Path::new("/bin/molt-gui"), &cfg_daily())).unwrap();
        let out = std::process::Command::new("plutil")
            .args(["-lint", &path.display().to_string()])
            .output();
        std::fs::remove_dir_all(&dir).ok();
        if let Ok(o) = out {
            assert!(
                o.status.success(),
                "plist 不合法：{:?}",
                String::from_utf8_lossy(&o.stdout)
            );
        }
    }

    #[test]
    fn invalid_times_are_rejected_before_writing() {
        let mut cfg = cfg_daily();
        cfg.hour = 24;
        assert!(validate(&cfg).is_err());
        cfg.hour = 3;
        cfg.minute = 60;
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn describe_reads_like_a_sentence() {
        assert_eq!(cfg_daily().describe(), "每天 03:30");
        let weekly = ScheduleConfig {
            frequency: Frequency::Weekly,
            weekday: 0,
            hour: 3,
            minute: 0,
            ..Default::default()
        };
        assert_eq!(weekly.describe(), "每周日 03:00");
    }

    #[test]
    fn only_auto_reclaimable_rules_become_kinds() {
        let rules = molt_core::RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 1

[[rule]]
id = "auto-cache"
patterns = ["/a/**"]
what = "自动缓存"
auto = true
verdict = { type = "reclaimable" }

[[rule]]
id = "manual-cache"
patterns = ["/b/**"]
what = "需要人看的缓存"
verdict = { type = "reclaimable" }

[[rule]]
id = "build"
patterns = ["/c/**"]
what = "构建产物"
verdict = { type = "costly", cost = "recompile", note = "重编" }
"#,
            "/Users/me",
        )
        .unwrap();
        let ks = kinds(&rules);
        assert_eq!(ks.len(), 1);
        assert_eq!(ks[0].rule_id, "auto-cache");
        assert_eq!(all_rule_ids(&rules), vec!["auto-cache".to_string()]);
    }

    #[test]
    fn empty_selection_never_falls_back_to_everything() {
        // 清空勾选后再保存，不能变成「全部类型」
        let cfg = ScheduleConfig {
            enabled: true,
            rule_ids: Vec::new(),
            ..Default::default()
        };
        assert!(!cfg.is_selected("anything"));
    }

    #[test]
    fn groups_are_stable() {
        assert_eq!(group_of("npm-cache"), "开发工具链");
        assert_eq!(group_of("cursor-cache"), "AI 工具");
        assert_eq!(group_of("ds-store"), "其它");
    }

    #[test]
    fn last_run_round_trips() {
        let run = LastRun {
            at: "2026-10-23T03:00:00Z".into(),
            ok: true,
            types: 3,
            types_effective: 2,
            candidates: 5,
            done: 4,
            bytes: 1024,
            failures: vec![("/x".into(), "占用中".into())],
            message: "已放入临时空间 4 项".into(),
        };
        let text = serde_json::to_string(&run).unwrap();
        let back: LastRun = serde_json::from_str(&text).unwrap();
        assert_eq!(back.done, 4);
        assert_eq!(back.failures.len(), 1);
    }

    /// 启动提醒的真值表：只有「说启用了、系统里没加载」才该出声。
    ///
    /// 其余三种都不提：没开过的人不该看到关于它的警告，没开也没加载是正常状态，
    /// 开了也加载了就是一切正常。多说一句就会变成启动噪音。
    #[test]
    fn startup_warning_only_fires_when_enabled_but_not_loaded() {
        let hit = health_warning(true, false).expect("启用了却没加载，必须提醒");
        assert!(hit.contains("到点不会执行"), "要说清后果：{hit}");
        assert!(hit.contains("自动清理"), "要指出能修它的页面：{hit}");

        assert!(health_warning(true, true).is_none(), "正常状态不提醒");
        assert!(health_warning(false, false).is_none(), "没启用不提醒");
        assert!(health_warning(false, true).is_none(), "没启用不提醒");
    }

    /// 原始英文异常不能就这样丢给用户。
    ///
    /// `Bootstrap failed: 5: Input/output error` 是用户最可能撞见的那一句；
    /// 单独看它只会以为软件坏了。翻译之后必须说清「为什么」和「下一步做什么」，
    /// 而且必须带上那份 plist 的真实路径，用户才找得到地方。
    #[test]
    fn bootstrap_failures_are_explained_in_chinese() {
        let plist = std::path::Path::new("/Users/me/Library/LaunchAgents/app.molt.schedule.plist");

        let io = explain_bootstrap_failure("Bootstrap failed: 5: Input/output error", plist);
        assert!(
            io.contains("系统拒绝了这份定时任务"),
            "要说出发生了什么：{io}"
        );
        assert!(io.contains("重启"), "要给出下一步：{io}");
        assert!(
            io.contains("app.molt.schedule.plist"),
            "要带上真实路径，否则用户不知道该去看哪份文件：{io}"
        );

        let dup = explain_bootstrap_failure("service already loaded", plist);
        assert!(dup.contains("刷新状态"), "已加载时指向「刷新状态」：{dup}");

        let missing = explain_bootstrap_failure("No such file or directory", plist);
        assert!(
            missing.contains("保存"),
            "文件不见了就让他重新保存：{missing}"
        );

        // 认不出来的原因也要原样保留，不能把它吞掉。
        let other = explain_bootstrap_failure("weird new error 42", plist);
        assert!(
            other.contains("weird new error 42"),
            "未知原因要原样带上：{other}"
        );
    }
}
