//! 规则库：把「哪条路径、是什么、删了怎样」从代码里搬到 TOML。
//!
//! 为什么规则不进数据库：规则需要 diff、review、回滚，这些都是版本控制的事。
//! 为什么规则要能编译期校验（见 `build.rs`）：一条写错的规则在生产环境的表现是「静默不命中」，
//! 而这里静默失败意味着用户以为清干净了、其实没有——所以宁可在编译时炸掉。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::glob::{glob_match, literal_prefix_len};
use crate::model::{CostKind, ItemKind, ProtectReason, RebuildCost, Verdict};

/// 一条规则。`id` 在整个规则库里必须唯一，它同时也是审计日志里的追溯依据。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rule {
    pub id: String,
    /// 路径模式，支持 `*` `?` `**`。以 `~/` 开头的会在加载时展开成家目录。
    pub patterns: Vec<String>,
    /// 这项东西是什么——直接显示给用户，也是 AI 解释缺位时的兜底文案。
    pub what: String,
    pub verdict: RuleVerdict,
    /// 命中的是目录还是文件；不写表示都接受。
    #[serde(default)]
    pub kind: Option<ItemKind>,
    /// 同一条路径被多条规则命中时，值大的赢。默认 0。
    #[serde(default)]
    pub priority: i32,
    /// 更高优先级的规则匹配到同一路径时，本条是否整体作废。
    /// 用于「父目录默认规则」被「子目录特例规则」覆盖的场景。
    #[serde(default)]
    pub overridable: bool,
    /// 是否允许无人值守清理（`molt auto` 只动这一类）。
    ///
    /// 这个开关写在规则里而不是代码里，因为「能不能不看就清」是**关于这项东西的知识**，
    /// 和它的判定理应是同一处。只有 `reclaimable` 能标 true——
    /// 一个有代价的项永远不会被自动清，这是编译期就拦住的。
    #[serde(default)]
    pub auto: bool,
    /// 闲置阈值（天）：命中的路径最近一次访问距今超过 N 天时，
    /// 判定层会给它带一个「建议清理」的标记，让用户先处理最没用的那批。
    ///
    /// 只允许写在 `costly` 规则上：可回收的项本来就该清、受保护的永远不动，
    /// 给它们标闲置阈值只会让规则读起来像在暗示「可以删」。
    #[serde(default)]
    pub stale_days: Option<u64>,
}

/// 规则里表达的判定。刻意与 `Verdict` 分开：规则文件不该能构造任意 `ProtectReason`
/// （比如 `SymlinkEscape` 只能由运行时判定产生，写进规则就是撒谎）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleVerdict {
    Reclaimable,
    Costly {
        cost: CostKind,
        #[serde(default)]
        est_seconds: Option<u64>,
        #[serde(default)]
        needs_network: bool,
        note: String,
    },
    Protected {
        reason: RuleProtectReason,
        /// 补充说明，例如「你的 Codex 对话记录」
        #[serde(default)]
        note: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleProtectReason {
    HardDeny,
    UserRecord,
    MixedContent,
}

impl RuleVerdict {
    pub fn to_verdict(&self, rule_id: &str) -> Verdict {
        match self {
            RuleVerdict::Reclaimable => Verdict::Reclaimable,
            RuleVerdict::Costly {
                cost,
                est_seconds,
                needs_network,
                note,
            } => Verdict::Costly {
                rebuild: RebuildCost {
                    kind: *cost,
                    est_seconds: *est_seconds,
                    needs_network: *needs_network,
                    note: note.clone(),
                },
            },
            RuleVerdict::Protected { reason, note } => {
                // note 是给读规则文件的人看的补充说明（如「你的 Codex 对话记录」）；
                // 判定本身只带原因类型，展示文案按原因生成，避免规则作者写出误导性的话。
                let _ = note;
                Verdict::Protected {
                    reason: match reason {
                        RuleProtectReason::HardDeny => ProtectReason::HardDeny {
                            rule: rule_id.to_string(),
                        },
                        RuleProtectReason::UserRecord => ProtectReason::UserRecord,
                        RuleProtectReason::MixedContent => ProtectReason::MixedContent,
                    },
                }
            }
        }
    }
}

/// 规则文件的外层结构，对应一个 `rules/*.toml`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuleFile {
    pub schema_version: u32,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

/// 加载后的规则库。构造时已经做过完整校验，所以查询路径上不再需要处理错误。
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

/// 规则库校验失败。带上是哪个文件的哪条规则，避免「有一条错了但不知道是哪条」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    pub file: String,
    pub rule_id: Option<String>,
    pub message: String,
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.rule_id {
            Some(id) => write!(f, "{} [{}] {}", self.file, id, self.message),
            None => write!(f, "{} {}", self.file, self.message),
        }
    }
}

impl std::error::Error for RuleError {}

/// 当前支持的规则文件格式版本。读到更高的版本要明确报错，不能静默忽略不认识的字段。
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

impl RuleSet {
    pub fn from_toml(file_name: &str, text: &str, home: &str) -> Result<Self, RuleError> {
        let parsed: RuleFile = toml::from_str(text).map_err(|e| RuleError {
            file: file_name.to_string(),
            rule_id: None,
            message: format!("TOML 解析失败：{e}"),
        })?;

        if parsed.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(RuleError {
                file: file_name.to_string(),
                rule_id: None,
                message: format!(
                    "schema_version={} 不受支持（本版本只认 {}）",
                    parsed.schema_version, SUPPORTED_SCHEMA_VERSION
                ),
            });
        }

        let mut rules = Vec::with_capacity(parsed.rules.len());
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();

        for rule in &parsed.rules {
            // 规则 id 重复会让审计日志无法追溯到底是哪条规则干的事
            if let Some(prev_pattern) = seen.insert(rule.id.as_str(), &rule.patterns[0]) {
                let _ = prev_pattern;
                return Err(RuleError {
                    file: file_name.to_string(),
                    rule_id: Some(rule.id.clone()),
                    message: "规则 id 重复".to_string(),
                });
            }
            if rule.patterns.is_empty() {
                return Err(RuleError {
                    file: file_name.to_string(),
                    rule_id: Some(rule.id.clone()),
                    message: "至少要有一个 pattern".to_string(),
                });
            }
            if rule.what.trim().is_empty() {
                return Err(RuleError {
                    file: file_name.to_string(),
                    rule_id: Some(rule.id.clone()),
                    message: "what 不能为空——它就是显示给用户的那句话".to_string(),
                });
            }
            if let RuleVerdict::Costly { note, .. } = &rule.verdict {
                if note.trim().is_empty() {
                    return Err(RuleError {
                        file: file_name.to_string(),
                        rule_id: Some(rule.id.clone()),
                        message: "有代价的项必须写明代价（note 不能为空）".to_string(),
                    });
                }
            }
            // 自动清理意味着没人看。没人看的清理只允许作用在「删了自动重建、无感」的东西上。
            // 允许一条有代价的规则 auto，等于让一次定时任务悄悄花掉用户半小时重编译。
            if rule.auto && !matches!(rule.verdict, RuleVerdict::Reclaimable) {
                return Err(RuleError {
                    file: file_name.to_string(),
                    rule_id: Some(rule.id.clone()),
                    message: "只有可回收的规则才能标记 auto——有代价的项必须有人在现场看一眼"
                        .to_string(),
                });
            }
            // 闲置阈值只对「有代价」有意义：可回收的项默认就该清，受保护的永远不动。
            // 写在这两类上不会改变任何行为，却会让规则读起来像另一回事——
            // 而规则里一句错话，比一个错字段更难被发现。
            if rule.stale_days.is_some() && !matches!(rule.verdict, RuleVerdict::Costly { .. }) {
                return Err(RuleError {
                    file: file_name.to_string(),
                    rule_id: Some(rule.id.clone()),
                    message: "stale_days 只能写在有代价的规则上——它是「重建不划算，但很久没用了」才用得上的判据"
                        .to_string(),
                });
            }
            rules.push(Rule {
                patterns: rule.patterns.iter().map(|p| expand_home(p, home)).collect(),
                ..rule.clone()
            });
        }

        // 优先级高的排前面，查询时第一个命中即返回，天然实现 overridable 语义
        rules.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| {
                    let al = a
                        .patterns
                        .iter()
                        .map(|p| literal_prefix_len(p))
                        .max()
                        .unwrap_or(0);
                    let bl = b
                        .patterns
                        .iter()
                        .map(|p| literal_prefix_len(p))
                        .max()
                        .unwrap_or(0);
                    bl.cmp(&al)
                })
                .then_with(|| a.id.cmp(&b.id))
        });

        Ok(Self { rules })
    }

    pub fn merge(&mut self, other: RuleSet) -> &mut Self {
        self.rules.extend(other.rules);
        self.rules.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| {
                    let al = a
                        .patterns
                        .iter()
                        .map(|p| literal_prefix_len(p))
                        .max()
                        .unwrap_or(0);
                    let bl = b
                        .patterns
                        .iter()
                        .map(|p| literal_prefix_len(p))
                        .max()
                        .unwrap_or(0);
                    bl.cmp(&al)
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        self
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter()
    }

    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// 这条规则是否允许无人值守清理。未知规则一律视为不允许。
    pub fn is_auto(&self, id: &str) -> bool {
        self.get(id).map(|r| r.auto).unwrap_or(false)
    }

    /// 这条规则的闲置阈值。未知规则、或没写阈值，都返回 `None`——
    /// 判定层据此决定「不提示」，而不是拿一个默认天数去猜。
    pub fn stale_days(&self, id: &str) -> Option<u64> {
        self.get(id).and_then(|r| r.stale_days)
    }

    /// 找出命中的规则。`overridable` 的规则一旦被更高优先级规则抢先，就不会出现在结果里。
    pub fn matching(&self, path: &str) -> Vec<&Rule> {
        let mut hits = Vec::new();
        let mut blocked_by_priority: Option<i32> = None;
        for rule in &self.rules {
            if let Some(p) = blocked_by_priority {
                if rule.priority < p {
                    // 更具体的规则一旦被更高优先级规则压过，就不再考虑它
                    if rule.overridable {
                        continue;
                    }
                }
            }
            if rule.patterns.iter().any(|pat| glob_match(pat, path)) {
                if rule.overridable && blocked_by_priority.is_none() {
                    blocked_by_priority = Some(rule.priority);
                }
                hits.push(rule);
            }
        }
        hits
    }

    /// 单条最佳匹配。这就是判定流水线里 `match_rule` 的全部。
    pub fn best_match(&self, path: &str) -> Option<&Rule> {
        self.matching(path).into_iter().next()
    }
}

/// 把 `~/` 展开成家目录。规则文件里写绝对家目录路径既不可移植也不该出现。
fn expand_home(pattern: &str, home: &str) -> String {
    if pattern == "~" {
        home.to_string()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        pattern.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn sample(text: &str) -> RuleSet {
        RuleSet::from_toml("test.toml", text, HOME).expect("规则应当能解析")
    }

    #[test]
    fn home_prefix_is_expanded() {
        let rs = sample(
            r#"
schema_version = 1
[[rule]]
id = "cargo-registry"
patterns = ["~/.cargo/registry/**"]
what = "Rust crate 源码缓存"
verdict = { type = "reclaimable" }
"#,
        );
        assert!(rs.best_match("/Users/me/.cargo/registry/index/x").is_some());
        assert!(rs.best_match("/Users/other/.cargo/registry").is_none());
    }

    #[test]
    fn costly_rule_requires_a_note() {
        let err = RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 1
[[rule]]
id = "x"
patterns = ["/a/**"]
what = "build"
verdict = { type = "costly", cost = "recompile", note = "" }
"#,
            HOME,
        )
        .unwrap_err();
        assert!(err.to_string().contains("代价"));
    }

    #[test]
    fn auto_is_rejected_on_costly_rules() {
        let err = RuleSet::from_toml(
            "t.toml",
            r#"
            schema_version = 1
            [[rule]]
            id = "t"
            patterns = ["**/target/**"]
            what = "构建产物"
            auto = true
            verdict = { type = "costly", cost = "recompile", note = "重编五分钟" }
            "#,
            HOME,
        )
        .unwrap_err();
        assert!(err.to_string().contains("auto"), "实际 {err}");
    }

    #[test]
    fn auto_defaults_to_false() {
        let rs = sample(
            r#"
            schema_version = 1
            [[rule]]
            id = "c"
            patterns = ["/a/**"]
            what = "缓存"
            verdict = { type = "reclaimable" }
            "#,
        );
        assert!(!rs.is_auto("c"));
        // 不存在的规则不能被当成允许自动清理
        assert!(!rs.is_auto("nope"));
    }

    #[test]
    fn stale_days_is_accepted_on_costly_rules() {
        let rs = sample(
            r#"
            schema_version = 1
            [[rule]]
            id = "t"
            patterns = ["**/target/**"]
            what = "构建产物"
            stale_days = 30
            verdict = { type = "costly", cost = "recompile", note = "重编五分钟" }
            "#,
        );
        assert_eq!(rs.stale_days("t"), Some(30));
        // 没写阈值，不能说成「闲置 0 天」——那会让每一项都被建议清理
        assert_eq!(rs.stale_days("nope"), None);
    }

    #[test]
    fn stale_days_on_protected_rules_is_rejected() {
        // 受保护的项永远不动，给它写闲置阈值等于在规则文件里暗示「久了就可以删」
        let err = RuleSet::from_toml(
            "t.toml",
            r#"
            schema_version = 1
            [[rule]]
            id = "sess"
            patterns = ["~/.codex/sessions/**"]
            what = "会话历史"
            stale_days = 30
            verdict = { type = "protected", reason = "user_record" }
            "#,
            HOME,
        )
        .unwrap_err();
        assert!(err.to_string().contains("stale_days"), "实际 {err}");
    }

    #[test]
    fn stale_days_on_reclaimable_rules_is_rejected() {
        // 可回收的项本来就该清，阈值对它没有任何意义
        let err = RuleSet::from_toml(
            "t.toml",
            r#"
            schema_version = 1
            [[rule]]
            id = "c"
            patterns = ["/a/**"]
            what = "缓存"
            stale_days = 7
            verdict = { type = "reclaimable" }
            "#,
            HOME,
        )
        .unwrap_err();
        assert!(err.to_string().contains("stale_days"), "实际 {err}");
    }

    #[test]
    fn empty_what_is_rejected() {
        let err = RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 1
[[rule]]
id = "x"
patterns = ["/a/**"]
what = "  "
verdict = { type = "reclaimable" }
"#,
            HOME,
        )
        .unwrap_err();
        assert!(err.to_string().contains("what"));
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let err = RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 1
[[rule]]
id = "dup"
patterns = ["/a/**"]
what = "a"
verdict = { type = "reclaimable" }

[[rule]]
id = "dup"
patterns = ["/b/**"]
what = "b"
verdict = { type = "reclaimable" }
"#,
            HOME,
        )
        .unwrap_err();
        assert!(err.to_string().contains("重复"));
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let err = RuleSet::from_toml(
            "t.toml",
            r#"
schema_version = 99
"#,
            HOME,
        )
        .unwrap_err();
        assert!(err.to_string().contains("schema_version"));
    }

    #[test]
    fn higher_priority_wins() {
        let rs = sample(
            r#"
schema_version = 1
[[rule]]
id = "generic-target"
patterns = ["**/target/**"]
what = "构建产物"
verdict = { type = "reclaimable" }

[[rule]]
id = "cpa-target"
patterns = ["/Users/me/project/cpa/**/target/**"]
what = "cpa 的构建产物"
priority = 10
verdict = { type = "costly", cost = "recompile", note = "全量重编 4-8 分钟" }
"#,
        );
        assert_eq!(
            rs.best_match("/Users/me/project/cpa/CLIProxyAPI/target/debug/x")
                .unwrap()
                .id,
            "cpa-target"
        );
        assert_eq!(
            rs.best_match("/Users/me/other/target/debug/x").unwrap().id,
            "generic-target"
        );
    }

    #[test]
    fn verdict_conversion_keeps_cost_detail() {
        let rs = sample(
            r#"
schema_version = 1
[[rule]]
id = "target"
patterns = ["**/target/**"]
what = "Rust 编译产物"
verdict = { type = "costly", cost = "recompile", est_seconds = 300, needs_network = false, note = "全量重编" }
"#,
        );
        let rule = rs.best_match("/p/target/debug").unwrap();
        match rule.verdict.to_verdict(&rule.id) {
            Verdict::Costly { rebuild } => {
                assert_eq!(rebuild.kind, CostKind::Recompile);
                assert_eq!(rebuild.est_seconds, Some(300));
                assert_eq!(rebuild.summary(), "重新编译，约 5 分钟");
            }
            other => panic!("应为 Costly，实际 {other:?}"),
        }
    }
}
