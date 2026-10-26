//! 终端输出。判定逻辑一个字都不在这里——这里只负责把已经算好的结果画出来。
//!
//! 排版遵循一条原则：**每一项都要能同时回答「这是什么、多大、删了会怎样」**。
//! 做不到这三点，用户就得再去查一遍，那这个工具就白做了。

use crate::paths::display_path;
use crate::scan::{ScanItem, ScanReport};
use molt_core::{Evidence, Verdict};
use molt_scan::human_bytes;

/// 状态词。用灰/橙/绿三色，与判定三态一一对应。
fn status_style(v: &Verdict) -> (&'static str, &'static str) {
    match v {
        Verdict::Reclaimable => ("\x1b[32m", "可回收"), // 绿
        Verdict::Costly { .. } => ("\x1b[33m", "有代价"), // 橙
        Verdict::Protected { .. } => ("\x1b[90m", "受保护"), // 灰
    }
}

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";

/// 打印完整清单。
pub fn print_report(report: &ScanReport, show_protected: bool) {
    println!();
    println!("{}净灵 · Cleanling{}", BOLD, RESET);
    println!("{}扫描完成，只读，未删除任何东西{}", DIM, RESET);
    println!();
    println!(
        "  \x1b[32m可回收\x1b[0m {}   \x1b[33m有代价\x1b[0m {}   \x1b[90m受保护\x1b[0m {}",
        human_bytes(report.reclaimable_bytes()),
        human_bytes(report.costly_bytes()),
        human_bytes(report.protected_bytes()),
    );
    println!();

    let mut any = false;
    for item in &report.items {
        if !show_protected && matches!(item.candidate.verdict, Verdict::Protected { .. }) {
            continue;
        }
        any = true;
        print_item(item);
    }

    if !any {
        println!("  没找到可清理的内容，磁盘很干净。");
    }

    if !report.denied_roots.is_empty() {
        println!();
        println!("  {}以下位置没有读取权限，本次未能统计：{}", DIM, RESET);
        for p in &report.denied_roots {
            println!("    {}", display_path(std::path::Path::new(p)));
        }
        println!(
            "  {}可在「系统设置 → 隐私与安全性 → 完全磁盘访问权限」里授权后重试。{}",
            DIM, RESET
        );
    }

    println!();
    if report.reclaimable_bytes() > 0 {
        println!(
            "{}下一步：molt clean --apply（先移进隔离区，{} 天内可 restore）{}",
            DIM,
            crate::paths::DEFAULT_RETAIN_DAYS,
            RESET
        );
    }
}

/// 打印单项。格式固定为三行：路径 / 说明 / 判定与代价。
fn print_item(item: &ScanItem) {
    let c = &item.candidate;
    let (color, label) = status_style(&c.verdict);
    let size = human_bytes(c.alloc_bytes.max(c.bytes));

    println!(
        "  {}{}{}  {}  {}",
        color,
        mark(&c.verdict),
        RESET,
        display_path(&c.path),
        size
    );

    // 「这是什么」：优先用规则里的说明，它比任何通用归类都准确
    let what = item
        .candidate
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::Description { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "未分类".to_string());

    let files_note = if c.files > 0 {
        format!(" · {} 个文件", c.files)
    } else {
        String::new()
    };

    println!("    {}{}· {}{}{}", what, files_note, color, label, RESET);

    // 建议清理：有代价、但很久没动过。只说「值得先处理」，不说「可以随便删」——
    // 它的代价依旧成立，所以后面紧跟着仍旧是那句代价说明。
    if c.suggest_clean {
        let days = c.evidence.iter().find_map(|e| match e {
            Evidence::LastUsed { days_ago } => Some(*days_ago),
            _ => None,
        });
        let note = match days {
            Some(d) => format!("建议清理 · {} 天未使用", d),
            None => "建议清理".to_string(),
        };
        println!("    \x1b[36m{}{}\x1b[0m", note, RESET);
    }

    // 「删了会怎样」——受保护项也要写清楚为什么，否则用户会反复怀疑
    println!("    {}{}{}", DIM, c.verdict.explanation(), RESET);

    for e in &c.evidence {
        match e {
            Evidence::LastUsed { days_ago } => {
                println!("    {}{} 天未使用{}", DIM, days_ago, RESET);
            }
            Evidence::InUse { name, .. } => {
                println!("    {}正在被 {} 使用{}", DIM, name, RESET);
            }
            Evidence::SecretFingerprint { file, .. } => {
                println!("    {}疑似密钥文件：{}{}", DIM, display_path(file), RESET);
            }
            _ => {}
        }
    }
    println!();
}

fn mark(v: &Verdict) -> &'static str {
    match v {
        Verdict::Reclaimable => "□",
        Verdict::Costly { .. } => "□",
        Verdict::Protected { .. } => "■",
    }
}

/// JSON 输出。给脚本用——`jq` 能直接吃。
pub fn print_json(report: &ScanReport) {
    #[derive(serde::Serialize)]
    struct Out<'a> {
        path: String,
        rule_id: String,
        bytes: u64,
        alloc_bytes: u64,
        files: u64,
        verdict: &'a Verdict,
        explanation: String,
        /// 是否「有意价但久未使用」。只是建议，不改变 verdict。
        suggest_clean: bool,
    }

    let items: Vec<Out> = report
        .items
        .iter()
        .map(|i| Out {
            path: i.candidate.path.display().to_string(),
            rule_id: i.discovered.rule_id.clone(),
            bytes: i.candidate.bytes,
            alloc_bytes: i.candidate.alloc_bytes,
            files: i.candidate.files,
            verdict: &i.candidate.verdict,
            explanation: i.candidate.verdict.explanation(),
            suggest_clean: i.candidate.suggest_clean,
        })
        .collect();

    let payload = serde_json::json!({
        "summary": report.summary_line(),
        "reclaimable_bytes": report.reclaimable_bytes(),
        "costly_bytes": report.costly_bytes(),
        "protected_bytes": report.protected_bytes(),
        "suggest_clean_count": report.suggest_clean_count(),
        "suggest_clean_bytes": report.suggest_clean_bytes(),
        "items": items,
    });
    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{CostKind, ItemKind, ProtectReason, RebuildCost};

    fn item(path: &str, v: Verdict, bytes: u64) -> ScanItem {
        let c = molt_core::Candidate::new(path, ItemKind::Dir, v)
            .with_size(bytes, bytes, 1)
            .push_evidence(Evidence::Description {
                text: "测试缓存".into(),
            });
        ScanItem {
            discovered: molt_scan::Discovered {
                path: path.into(),
                rule_id: "t".into(),
                pattern: path.into(),
            },
            candidate: c,
        }
    }

    #[test]
    fn totals_split_by_verdict() {
        let report = ScanReport {
            items: vec![
                item("/a", Verdict::Reclaimable, 100),
                item(
                    "/b",
                    Verdict::Costly {
                        rebuild: RebuildCost::new(CostKind::Recompile, "重编"),
                    },
                    200,
                ),
                item(
                    "/c",
                    Verdict::Protected {
                        reason: ProtectReason::UserRecord,
                    },
                    400,
                ),
            ],
            denied_roots: vec![],
        };
        assert_eq!(report.reclaimable_bytes(), 100);
        assert_eq!(report.costly_bytes(), 200);
        assert_eq!(report.protected_bytes(), 400);
    }

    #[test]
    fn reclaimable_uses_alloc_bytes_when_larger() {
        // 小文件被文件系统按块分配，实际占用更大；预计释放要报大的那个
        let mut c = molt_core::Candidate::new("/a", ItemKind::Dir, Verdict::Reclaimable);
        c.bytes = 10;
        c.alloc_bytes = 4096;
        let report = ScanReport {
            items: vec![ScanItem {
                discovered: molt_scan::Discovered {
                    path: "/a".into(),
                    rule_id: "t".into(),
                    pattern: "/a".into(),
                },
                candidate: c,
            }],
            denied_roots: vec![],
        };
        assert_eq!(report.reclaimable_bytes(), 4096);
    }

    #[test]
    fn summary_line_mentions_all_three() {
        let report = ScanReport::default();
        let line = report.summary_line();
        assert!(line.contains("可回收"));
        assert!(line.contains("有代价"));
        assert!(line.contains("受保护"));
    }
}
