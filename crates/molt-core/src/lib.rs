//! `molt-core`：判定逻辑。**这一层不碰文件系统**。
//!
//! 这样切的理由：判定是产品里最需要被信任、也最需要被逐条验证的部分。
//! 把它与 IO 分开之后，每一条规则、每一道闸都能用临时构造的元数据直接测，
//! 不需要在真实磁盘上摆一套目录树——而那些树一旦建错，测出来的「通过」是假的。

pub mod glob;
pub mod model;
pub mod rules;

pub use model::{Candidate, CostKind, Evidence, ItemKind, ProtectReason, RebuildCost, Verdict};
pub use rules::{Rule, RuleError, RuleSet, RuleVerdict};

use std::path::Path;

/// 扫描阶段拿到的最小元数据。刻意不含文件内容——判定不该需要读内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMeta {
    pub path: std::path::PathBuf,
    pub kind: ItemKind,
    pub bytes: u64,
    pub alloc_bytes: u64,
    pub files: u64,
    pub last_used_days: Option<u64>,
}

/// 判定输入：元数据 + 可由调用方注入的额外信号。
///
/// 把硬禁区、指纹、占用检查做成「信号」而不是在这里直接调用，
/// 是为了让 core 保持无 IO；真正的实现放在 `molt-safety`，由流水线注入。
#[derive(Debug, Clone, Default)]
pub struct Signals {
    /// 命中的硬禁区规则名（若有）
    pub hard_deny: Option<(String, String)>,
    /// 命中的密钥指纹模式名（若有）
    pub secret: Option<(String, std::path::PathBuf)>,
    /// 占用该路径的进程（pid, 名称）
    pub in_use: Option<(u32, String)>,
    /// 符号链接解析结果，用于逃逸检查
    pub resolved_target: Option<std::path::PathBuf>,
    /// 无读取权限
    pub no_permission: bool,
}

/// 判定流水线的纯逻辑部分：`match_rule → 硬禁区 → 指纹 → 占用 → 逃逸`。
///
/// 顺序是有意的：规则先给出「本来该怎么判」，随后的每一道闸只能让它更保守
/// （Reclaimable → Costly → Protected），**永远不能反向放宽**。
/// 这就是为什么规则库写错也删不掉不该删的东西。
pub fn decide(
    meta: &PathMeta,
    rules: &RuleSet,
    signals: &Signals,
    authorized_roots: &[std::path::PathBuf],
) -> Candidate {
    let mut candidate = Candidate::new(meta.path.clone(), meta.kind, Verdict::Reclaimable)
        .with_size(meta.bytes, meta.alloc_bytes, meta.files);

    // 1) 规则命中：给出初始判定与「这是什么」
    let mut rule_id: Option<String> = None;
    // 规则上的闲置阈值。只对「有代价」有意义，拿不到就是 None。
    let mut stale_days: Option<u64> = None;
    if let Some(path_str) = meta.path.to_str() {
        if let Some(rule) = rules.best_match(path_str) {
            candidate.verdict = rule.verdict.to_verdict(&rule.id);
            candidate = candidate.push_evidence(Evidence::Description {
                text: rule.what.clone(),
            });
            // 把命中的具体 pattern 也记下来，审计时能回答「凭什么」
            if let Some(pat) = rule.patterns.iter().find(|p| glob::glob_match(p, path_str)) {
                candidate = candidate.push_evidence(Evidence::RuleHit {
                    rule_id: rule.id.clone(),
                    pattern: pat.clone(),
                });
            }
            stale_days = rule.stale_days;
            rule_id = Some(rule.id.clone());
        }
    }

    if meta.path.to_str().is_none() {
        // 非 UTF-8 路径无法安全匹配规则，也不该被当成可回收
        candidate.verdict = Verdict::Protected {
            reason: ProtectReason::NoPermission,
        };
    }

    // 2) 硬禁区：规则库也无法覆盖
    if let Some((rule, matched)) = &signals.hard_deny {
        candidate.verdict = Verdict::Protected {
            reason: ProtectReason::HardDeny { rule: rule.clone() },
        };
        candidate = candidate.push_evidence(Evidence::HardDeny {
            rule: rule.clone(),
            matched: matched.clone(),
        });
    }

    // 3) 密钥指纹
    if let Some((pattern, file)) = &signals.secret {
        candidate.verdict = Verdict::Protected {
            reason: ProtectReason::SecretLike {
                pattern: pattern.clone(),
            },
        };
        candidate = candidate.push_evidence(Evidence::SecretFingerprint {
            pattern: pattern.clone(),
            file: file.clone(),
        });
    }

    // 4) 进程占用
    if let Some((pid, name)) = &signals.in_use {
        candidate.verdict = Verdict::Protected {
            reason: ProtectReason::InUse {
                by: Some(name.clone()),
            },
        };
        candidate = candidate.push_evidence(Evidence::InUse {
            pid: *pid,
            name: name.clone(),
        });
    }

    // 5) 符号链接逃逸：链接指向授权根之外时，删链接本身也可能删到目标
    if let (ItemKind::Symlink, Some(target)) = (meta.kind, &signals.resolved_target) {
        if !is_within_any(target, authorized_roots) {
            candidate.verdict = Verdict::Protected {
                reason: ProtectReason::SymlinkEscape {
                    resolved: target.clone(),
                },
            };
        }
    }

    // 6) 无权限
    if signals.no_permission {
        candidate.verdict = Verdict::Protected {
            reason: ProtectReason::NoPermission,
        };
    }

    if let Some(days) = meta.last_used_days {
        candidate = candidate.push_evidence(Evidence::LastUsed { days_ago: days });
        // 顺手把时间戳填进模型里已有的 `last_used`：扫描层只量到「整几天」，
        // 这里按天数回推一个时间点，精度就是那一天，和 evidence 里的数字永远一致。
        candidate.last_used = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(days.saturating_mul(86_400)));
    }

    // 7) 闲置建议：放在所有闸门之后算，用的是**最终**判定。
    // 一条规则先说有代价、随后被占用检查按成受保护时，绝不能还挂着「建议清理」。
    candidate.suggest_clean = should_suggest_clean(&candidate.verdict, stale_days, meta.last_used_days);

    candidate.rule_id = rule_id;
    candidate
}

/// 「建议清理」的判据，单独成函数是为了能逐条单测。
///
/// 四条同时成立才算：
/// - 判定是 `Costly`——可回收的项本来就该清，受保护的永远不动；
/// - 规则写了 `stale_days`——没写阈值就不该替用户假设「多久算久」；
/// - 实测到了闲置天数——没有时间数据就不下结论；
/// - 天数达到阈值。
///
/// `last_used_days` 为 `None` 时一律 false：没数据不等于很久没用，宁可不动。
pub fn should_suggest_clean(
    verdict: &Verdict,
    stale_days: Option<u64>,
    last_used_days: Option<u64>,
) -> bool {
    let (Some(limit), Some(days)) = (stale_days, last_used_days) else {
        return false;
    };
    matches!(verdict, Verdict::Costly { .. }) && days >= limit
}

/// `target` 是否落在任一授权根之下。两侧都必须是已经规范化过的绝对路径。
pub fn is_within_any(target: &Path, roots: &[std::path::PathBuf]) -> bool {
    roots.iter().any(|root| target.starts_with(root))
}

/// 父子互斥检查的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentChildCheck {
    /// 所有子项判定一致，父目录可以整体处理
    Uniform(Verdict),
    /// 混装：父目录不能整体处理，只能逐项
    Mixed { protected_children: usize },
    /// 没有子项信息，按混装处理（保守）
    Unknown,
}

/// §6.4 的核心：一个目录只有在「所有子项判定一致」时才允许被整体删除。
///
/// 这条不是优化，是防事故。`~/Library/Application Support/Code/` 下面同时有
/// `Code Cache`（可再生）和 `User/History`（用户的文件版本记录）——
/// 如果按父目录判定，一次清理会连带删掉历史版本，而且删完才发现。
pub fn check_parent_child(_parent: &Candidate, children: &[Candidate]) -> ParentChildCheck {
    if children.is_empty() {
        return ParentChildCheck::Unknown;
    }
    let protected_children = children
        .iter()
        .filter(|c| !c.verdict.is_actionable())
        .count();
    if protected_children > 0 {
        return ParentChildCheck::Mixed { protected_children };
    }
    let first = children[0].verdict.clone();
    if children.iter().all(|c| c.verdict == first) {
        ParentChildCheck::Uniform(first)
    } else {
        ParentChildCheck::Mixed {
            protected_children: 0,
        }
    }
}

/// 把混装的父目录降级为「只能逐项处理」。
pub fn apply_parent_child_guard(parent: &mut Candidate, check: &ParentChildCheck) {
    match check {
        ParentChildCheck::Uniform(_) => {}
        ParentChildCheck::Mixed { .. } | ParentChildCheck::Unknown => {
            if parent.verdict.is_actionable() {
                parent.verdict = Verdict::Protected {
                    reason: ProtectReason::MixedContent,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CostKind, RebuildCost};
    use std::path::PathBuf;

    const HOME: &str = "/Users/me";

    fn ruleset() -> RuleSet {
        RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 1

[[rule]]
id = "cargo-registry"
patterns = ["~/.cargo/registry/**"]
what = "Rust crate 源码缓存"
verdict = { type = "reclaimable" }

[[rule]]
id = "target"
patterns = ["**/target/**"]
what = "Rust 编译产物"
verdict = { type = "costly", cost = "recompile", note = "全量重编" }

[[rule]]
id = "sessions"
patterns = ["~/.codex/sessions/**"]
what = "Codex 会话历史"
verdict = { type = "protected", reason = "user_record" }
"#,
            HOME,
        )
        .unwrap()
    }

    fn meta(path: &str) -> PathMeta {
        PathMeta {
            path: PathBuf::from(path),
            kind: ItemKind::Dir,
            bytes: 1024,
            alloc_bytes: 1024,
            files: 1,
            last_used_days: Some(3),
        }
    }

    #[test]
    fn rule_gives_initial_verdict_and_description() {
        let c = decide(
            &meta("/Users/me/.cargo/registry/index"),
            &ruleset(),
            &Signals::default(),
            &[PathBuf::from("/Users/me")],
        );
        assert_eq!(c.verdict, Verdict::Reclaimable);
        assert!(c
            .evidence
            .iter()
            .any(|e| matches!(e, Evidence::Description { text } if text.contains("crate"))));
    }

    #[test]
    fn hard_deny_overrides_a_reclaimable_rule() {
        // 即使规则说可回收，硬禁区也把它按死
        let signals = Signals {
            hard_deny: Some(("ssh-keys".into(), "~/.ssh/**".into())),
            ..Default::default()
        };
        let mut m = meta("/Users/me/.cargo/registry/x");
        m.path = PathBuf::from("/Users/me/.cargo/registry/x");
        let c = decide(&m, &ruleset(), &signals, &[PathBuf::from("/Users/me")]);
        assert!(matches!(c.verdict, Verdict::Protected { .. }));
        assert!(c
            .evidence
            .iter()
            .any(|e| matches!(e, Evidence::HardDeny { .. })));
    }

    #[test]
    fn secret_fingerprint_overrides_everything() {
        let signals = Signals {
            secret: Some(("openai_key".into(), PathBuf::from("/x/config.yaml"))),
            ..Default::default()
        };
        let c = decide(
            &meta("/Users/me/.cargo/registry"),
            &ruleset(),
            &signals,
            &[PathBuf::from("/Users/me")],
        );
        match c.verdict {
            Verdict::Protected {
                reason: ProtectReason::SecretLike { pattern },
            } => assert_eq!(pattern, "openai_key"),
            other => panic!("应为密钥保护，实际 {other:?}"),
        }
    }

    #[test]
    fn in_use_beats_reclaimable() {
        let signals = Signals {
            in_use: Some((4242, "Codex".into())),
            ..Default::default()
        };
        let c = decide(
            &meta("/Users/me/.cargo/registry"),
            &ruleset(),
            &signals,
            &[PathBuf::from("/Users/me")],
        );
        assert_eq!(
            c.verdict,
            Verdict::Protected {
                reason: ProtectReason::InUse {
                    by: Some("Codex".into())
                }
            }
        );
    }

    #[test]
    fn gates_never_relax_a_protected_rule() {
        let c = decide(
            &meta("/Users/me/.codex/sessions/abc"),
            &ruleset(),
            &Signals::default(),
            &[PathBuf::from("/Users/me")],
        );
        assert_eq!(
            c.verdict,
            Verdict::Protected {
                reason: ProtectReason::UserRecord
            }
        );
    }

    #[test]
    fn symlink_escape_is_caught() {
        let mut m = meta("/Users/me/.cargo/registry");
        m.kind = ItemKind::Symlink;
        let signals = Signals {
            resolved_target: Some(PathBuf::from("/etc")),
            ..Default::default()
        };
        let c = decide(&m, &ruleset(), &signals, &[PathBuf::from("/Users/me")]);
        assert!(matches!(
            c.verdict,
            Verdict::Protected {
                reason: ProtectReason::SymlinkEscape { .. }
            }
        ));
    }

    #[test]
    fn symlink_inside_root_is_fine() {
        let mut m = meta("/Users/me/.cargo/registry");
        m.kind = ItemKind::Symlink;
        let signals = Signals {
            resolved_target: Some(PathBuf::from("/Users/me/.cargo/registry/real")),
            ..Default::default()
        };
        let c = decide(&m, &ruleset(), &signals, &[PathBuf::from("/Users/me")]);
        assert_eq!(c.verdict, Verdict::Reclaimable);
    }

    #[test]
    fn mixed_parent_cannot_be_deleted_wholesale() {
        let parent = Candidate::new(
            "/Library/Application Support/Code",
            ItemKind::Dir,
            Verdict::Reclaimable,
        );
        let children = vec![
            Candidate::new("/a", ItemKind::Dir, Verdict::Reclaimable),
            Candidate::new(
                "/b",
                ItemKind::Dir,
                Verdict::Protected {
                    reason: ProtectReason::UserRecord,
                },
            ),
        ];
        let check = check_parent_child(&parent, &children);
        assert_eq!(
            check,
            ParentChildCheck::Mixed {
                protected_children: 1
            }
        );

        let mut p = parent;
        apply_parent_child_guard(&mut p, &check);
        assert!(matches!(
            p.verdict,
            Verdict::Protected {
                reason: ProtectReason::MixedContent
            }
        ));
    }

    #[test]
    fn uniform_parent_stays_actionable() {
        let parent = Candidate::new("/a", ItemKind::Dir, Verdict::Reclaimable);
        let children = vec![
            Candidate::new("/a/x", ItemKind::Dir, Verdict::Reclaimable),
            Candidate::new("/a/y", ItemKind::Dir, Verdict::Reclaimable),
        ];
        let check = check_parent_child(&parent, &children);
        assert_eq!(check, ParentChildCheck::Uniform(Verdict::Reclaimable));
    }

    #[test]
    fn empty_children_is_conservatively_mixed() {
        let parent = Candidate::new("/a", ItemKind::Dir, Verdict::Reclaimable);
        assert_eq!(check_parent_child(&parent, &[]), ParentChildCheck::Unknown);
    }

    #[test]
    fn costly_carries_its_note_through_the_pipeline() {
        let c = decide(
            &meta("/p/target/debug"),
            &ruleset(),
            &Signals::default(),
            &[PathBuf::from("/p")],
        );
        match c.verdict {
            Verdict::Costly { ref rebuild } => {
                assert_eq!(rebuild.kind, CostKind::Recompile);
                assert_eq!(rebuild.note, "全量重编");
            }
            other => panic!("应为 Costly，实际 {other:?}"),
        }
        assert!(!c.verdict.default_checked());
    }

    #[test]
    fn rebuild_cost_is_reexported_for_callers() {
        let rc = RebuildCost::new(CostKind::Reindex, "重建索引");
        assert_eq!(rc.summary(), "重新建立索引");
    }

    /// 带闲置阈值的规则库，用于测「建议清理」。
    fn stale_ruleset() -> RuleSet {
        RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 1

[[rule]]
id = "stale-target"
patterns = ["**/target/**"]
what = "Rust 编译产物"
stale_days = 30
verdict = { type = "costly", cost = "recompile", note = "全量重编" }

[[rule]]
id = "fresh-target"
patterns = ["/p/node_modules/**"]
what = "Node 依赖"
verdict = { type = "costly", cost = "reinstall", needs_network = true, note = "重新 install" }
"#,
            HOME,
        )
        .unwrap()
    }

    #[test]
    fn suggest_clean_when_idle_days_exceed_the_threshold() {
        let mut m = meta("/p/target/debug");
        m.last_used_days = Some(62);
        let c = decide(&m, &stale_ruleset(), &Signals::default(), &[PathBuf::from("/p")]);
        assert!(c.suggest_clean, "62 天没动过、阈值 30 天，应当建议清理");
        // 建议归建议，三态判定不能被动过
        assert!(matches!(c.verdict, Verdict::Costly { .. }));
    }

    #[test]
    fn no_suggestion_below_the_threshold() {
        let mut m = meta("/p/target/debug");
        m.last_used_days = Some(3);
        let c = decide(&m, &stale_ruleset(), &Signals::default(), &[PathBuf::from("/p")]);
        assert!(!c.suggest_clean);
    }

    #[test]
    fn unknown_last_used_never_becomes_a_suggestion() {
        // 没数据不等于很久没用。这里必须保守：`None` 一律不置 true。
        let mut m = meta("/p/target/debug");
        m.last_used_days = None;
        let c = decide(&m, &stale_ruleset(), &Signals::default(), &[PathBuf::from("/p")]);
        assert!(
            !c.suggest_clean,
            "读不到访问时间时不能凭规则里的阈值就建议删"
        );
    }

    #[test]
    fn a_rule_without_stale_days_never_suggests() {
        // node_modules 那条没写阈值，即使闲置三年也不该被建议清理
        let mut m = meta("/p/node_modules/x");
        m.last_used_days = Some(1000);
        let c = decide(&m, &stale_ruleset(), &Signals::default(), &[PathBuf::from("/p")]);
        assert!(!c.suggest_clean);
    }

    #[test]
    fn a_gate_that_protects_the_path_clears_the_suggestion() {
        // 规则说有代价且很旧，但占用检查把它按成了受保护——
        // 这时候还挂着「建议清理」就是在骗人去点一个删不掉的东西
        let mut m = meta("/p/target/debug");
        m.last_used_days = Some(90);
        let signals = Signals {
            in_use: Some((4242, "cargo".into())),
            ..Default::default()
        };
        let c = decide(&m, &stale_ruleset(), &signals, &[PathBuf::from("/p")]);
        assert!(matches!(c.verdict, Verdict::Protected { .. }));
        assert!(!c.suggest_clean);
    }

    #[test]
    fn suggestion_never_turns_into_default_checked() {
        // 建议清理只给人看：勾选默认值仍然只看三态
        let mut m = meta("/p/target/debug");
        m.last_used_days = Some(90);
        let c = decide(&m, &stale_ruleset(), &Signals::default(), &[PathBuf::from("/p")]);
        assert!(c.suggest_clean);
        assert!(!c.verdict.default_checked(), "有代价项永远不默认勾选");
    }

    #[test]
    fn should_suggest_clean_requires_every_condition() {
        let costly = Verdict::Costly {
            rebuild: RebuildCost::new(CostKind::Recompile, "重编"),
        };
        assert!(should_suggest_clean(&costly, Some(30), Some(30)));
        assert!(should_suggest_clean(&costly, Some(30), Some(31)));
        assert!(!should_suggest_clean(&costly, Some(30), Some(29)));
        assert!(!should_suggest_clean(&costly, None, Some(999)));
        assert!(!should_suggest_clean(&costly, Some(30), None));
        assert!(!should_suggest_clean(&Verdict::Reclaimable, Some(30), Some(999)));
        assert!(!should_suggest_clean(
            &Verdict::Protected {
                reason: ProtectReason::UserRecord
            },
            Some(30),
            Some(999)
        ));
    }
}
