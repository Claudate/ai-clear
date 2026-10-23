//! 无人值守清理的**选择策略**。
//!
//! 这是整个产品里唯一没有人在现场看清单的路径，所以它必须窄到可以一句话说完：
//! **只碰「规则自己声明了 `auto = true`」且判定为可回收的项**。
//!
//! 为什么把判断放在这里而不是 `main.rs`：这是策略，不是流程。
//! 策略要能单独测——「一条有代价的规则混进来会怎样」这种事，
//! 只有写成纯函数才能在临时目录里验，而不必真装一个定时任务。

use crate::scan::{ScanItem, ScanReport};
use molt_core::{RuleSet, Verdict};

/// 从一次扫描结果里挑出可以无人值守清理的项。
///
/// 三个条件同时成立才入选：
/// 1. 判定是 `Reclaimable`（删了自动重建，用户无感）
/// 2. 候选带有规则 id，且那条规则 `auto = true`
/// 3. 没有规则归属的条目一律排除——没人能说清它是什么，就不该在没人看着时动它
pub fn select<'a>(report: &'a ScanReport, rules: &RuleSet) -> Vec<&'a ScanItem> {
    report
        .items
        .iter()
        .filter(|i| i.candidate.verdict == Verdict::Reclaimable)
        .filter(|i| match &i.candidate.rule_id {
            Some(id) => rules.is_auto(id),
            None => false,
        })
        .collect()
}

/// 被挡在外面、但用户可能以为会被自动清理的项——用于输出里的一句提醒。
///
/// 存在的理由：规则库更新后 `auto` 可能在两版之间被关掉，
/// 用户会以为「以前每周自动清的东西怎么不清了」。明说出数量，比让用户自己发现好。
pub fn blocked_count(report: &ScanReport, rules: &RuleSet) -> usize {
    report
        .items
        .iter()
        .filter(|i| i.candidate.verdict == Verdict::Reclaimable)
        .filter(|i| match &i.candidate.rule_id {
            Some(id) => !rules.is_auto(id),
            None => true,
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{Candidate, Evidence, ItemKind};

    const HOME: &str = "/Users/me";

    fn item(path: &str, rule: &str, verdict: Verdict, bytes: u64) -> ScanItem {
        let mut c = Candidate::new(path, ItemKind::Dir, verdict)
            .with_size(bytes, bytes, 1)
            .push_evidence(Evidence::Description {
                text: "测试".into(),
            });
        c.rule_id = Some(rule.to_string());
        ScanItem {
            discovered: molt_scan::Discovered {
                path: path.into(),
                rule_id: rule.into(),
                pattern: path.into(),
            },
            candidate: c,
        }
    }

    fn rules() -> RuleSet {
        RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 1

[[rule]]
id = "yes"
patterns = ["/a/**"]
what = "自动可清"
auto = true
verdict = { type = "reclaimable" }

[[rule]]
id = "no"
patterns = ["/b/**"]
what = "默认可清但需人看"
verdict = { type = "reclaimable" }

[[rule]]
id = "costly"
patterns = ["/c/**"]
what = "构建产物"
verdict = { type = "costly", cost = "recompile", note = "重编" }
"#,
            HOME,
        )
        .unwrap()
    }

    #[test]
    fn only_auto_declared_rules_are_selected() {
        let report = ScanReport {
            items: vec![
                item("/a/x", "yes", Verdict::Reclaimable, 100),
                item("/b/x", "no", Verdict::Reclaimable, 200),
            ],
            denied_roots: vec![],
        };
        let picked = select(&report, &rules());
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].candidate.path.to_str().unwrap(), "/a/x");
    }

    #[test]
    fn costly_items_never_qualify_even_if_flagged() {
        // 防的是「有人手动改规则给 costly 也加 auto」这种情况：
        // 规则加载期就会拒绝，这里再确认一次判定本身也不放过
        let report = ScanReport {
            items: vec![item(
                "/c/x",
                "costly",
                Verdict::Costly {
                    rebuild: molt_core::RebuildCost::new(molt_core::CostKind::Recompile, "重编"),
                },
                500,
            )],
            denied_roots: vec![],
        };
        assert!(select(&report, &rules()).is_empty());
    }

    #[test]
    fn candidates_without_a_rule_are_excluded() {
        let mut it = item("/z/x", "yes", Verdict::Reclaimable, 10);
        it.candidate.rule_id = None;
        let report = ScanReport {
            items: vec![it],
            denied_roots: vec![],
        };
        assert!(select(&report, &rules()).is_empty());
    }

    #[test]
    fn protected_is_excluded() {
        let report = ScanReport {
            items: vec![item(
                "/a/x",
                "yes",
                Verdict::Protected {
                    reason: molt_core::ProtectReason::UserRecord,
                },
                10,
            )],
            denied_roots: vec![],
        };
        assert!(select(&report, &rules()).is_empty());
    }

    #[test]
    fn blocked_count_counts_reclaimable_that_need_a_human() {
        let report = ScanReport {
            items: vec![
                item("/a/x", "yes", Verdict::Reclaimable, 100),
                item("/b/x", "no", Verdict::Reclaimable, 200),
            ],
            denied_roots: vec![],
        };
        assert_eq!(blocked_count(&report, &rules()), 1);
    }
}
