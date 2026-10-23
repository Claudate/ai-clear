//! 产品的核心词汇：一个候选清理项，以及它「删了会怎样」。
//!
//! 三态判定（Reclaimable / Costly / Protected）是这个系统里唯一的判定词汇：
//! CLI、GUI、审计日志都用它，任何地方都不允许再发明第四种状态。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// 候选条目的形态。区分它是因为不同形态的删除语义不同：
/// `SqliteCluster` 必须整簇处理（单删 -wal/-shm 会损坏库），`Symlink` 只能删链接本身。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    File,
    Dir,
    Symlink,
    SqliteCluster,
}

/// 删除后的重建代价类型。`LoseRecord` 与前面几项有本质区别：
/// 前几项是「花时间能恢复」，它是「根本恢复不了」——所以它只用于 Costly 项的说明，
/// 真正不可恢复的数据必须判为 Protected。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostKind {
    /// 重新编译（Rust target、go-build）
    Recompile,
    /// 重新下载（cargo registry、npm/pip 缓存）
    Redownload,
    /// 重新安装依赖（node_modules、.venv）
    Reinstall,
    /// 重新建立索引（编辑器 / 语言服务器）
    Reindex,
    /// 会打断别的脚本或配置的引用（cpa 里被 import 脚本引用的目录）
    BreakScript,
    /// 丢失历史版本，但不影响继续工作
    LoseRecord,
}

impl CostKind {
    /// 面向用户的一句话，用在清单页右栏。
    pub fn label(self) -> &'static str {
        match self {
            CostKind::Recompile => "重新编译",
            CostKind::Redownload => "重新下载",
            CostKind::Reinstall => "重新安装依赖",
            CostKind::Reindex => "重新建立索引",
            CostKind::BreakScript => "会打断现有脚本引用",
            CostKind::LoseRecord => "会丢失历史版本",
        }
    }
}

/// 重建代价。`est_seconds` 允许为 None——宁可显示「取决于网络」也不编数字，
/// 因为一个对不上的时间估计会直接毁掉用户对整份清单的信任。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebuildCost {
    pub kind: CostKind,
    pub est_seconds: Option<u64>,
    pub needs_network: bool,
    /// 向用户解释这条代价的具体依据，例如「cpa 的 Go 项目全量重编约 2-4 分钟」
    pub note: String,
}

impl RebuildCost {
    pub fn new(kind: CostKind, note: impl Into<String>) -> Self {
        Self {
            kind,
            est_seconds: None,
            needs_network: false,
            note: note.into(),
        }
    }

    pub fn with_network(mut self) -> Self {
        self.needs_network = true;
        self
    }

    pub fn with_seconds(mut self, secs: u64) -> Self {
        self.est_seconds = Some(secs);
        self
    }

    /// 清单页显示用的短句。未知时长时说清楚「为什么不知道」，不要留空。
    pub fn summary(&self) -> String {
        match (self.est_seconds, self.needs_network) {
            (Some(s), true) => format!("{}，约 {}（需联网）", self.kind.label(), human_duration(s)),
            (Some(s), false) => format!("{}，约 {}", self.kind.label(), human_duration(s)),
            (None, true) => format!("{}，耗时取决于网络", self.kind.label()),
            (None, false) => self.kind.label().to_string(),
        }
    }
}

/// 把秒数说成人话。超过一小时就只说小时，避免「约 127 分钟」这种没人能用的数字。
fn human_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{} 秒", secs)
    } else if secs < 3600 {
        format!("{} 分钟", secs / 60)
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{} 小时", h)
        } else {
            format!("{} 小时 {} 分钟", h, m)
        }
    }
}

/// 为什么这一项不能动。每一种都要能直接显示给用户，不能只是内部枚举。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectReason {
    /// 命中代码内嵌的硬禁区（钥匙串、私钥、配置文件、SQLite 副本边车文件）
    HardDeny { rule: String },
    /// 内容里扫到了疑似密钥
    SecretLike { pattern: String },
    /// 文件被进程打开，删了会让对方出错
    InUse { by: Option<String> },
    /// 是用户的记录而非常规缓存（会话历史、文件版本历史）
    UserRecord,
    /// 符号链接解析后跑到了授权根之外
    SymlinkEscape { resolved: PathBuf },
    /// 无权限读取（TCC 拒绝），无法判定就不动手
    NoPermission,
    /// 父目录与子项判定冲突，只能子项级操作
    MixedContent,
}

impl fmt::Display for ProtectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtectReason::HardDeny { rule } => write!(f, "受保护路径（{rule}）"),
            ProtectReason::SecretLike { pattern } => write!(f, "疑似包含密钥（{pattern}）"),
            ProtectReason::InUse { by: Some(p) } => write!(f, "正在被 {p} 使用"),
            ProtectReason::InUse { by: None } => write!(f, "正在被进程使用"),
            ProtectReason::UserRecord => write!(f, "这是你的记录，不是缓存"),
            ProtectReason::SymlinkEscape { resolved } => {
                write!(f, "链接指向清理范围之外（{}）", resolved.display())
            }
            ProtectReason::NoPermission => write!(f, "没有读取权限，已跳过"),
            ProtectReason::MixedContent => write!(f, "目录内混有受保护内容，只能逐项处理"),
        }
    }
}

/// 判定结果。这是三态，不是布尔——因为「能删」和「不能删」之间还有一整片区域，
/// 而用户真正需要的正是那片区域的量化（§二.2 的 4.15 G 全部落在这里）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// 删了自动重建，用户无感
    Reclaimable,
    /// 可重建，但要付出代价
    Costly { rebuild: RebuildCost },
    /// 不可选，且必须给出原因
    Protected { reason: ProtectReason },
}

impl Verdict {
    /// 唯一需要看权限的地方：只有 Reclaimable 默认勾选。
    pub fn default_checked(&self) -> bool {
        matches!(self, Verdict::Reclaimable)
    }

    pub fn is_actionable(&self) -> bool {
        !matches!(self, Verdict::Protected { .. })
    }

    /// 清单页左侧的状态词。用灰不用红——红色会让人以为「危险但可删」。
    pub fn status_label(&self) -> &'static str {
        match self {
            Verdict::Reclaimable => "可回收",
            Verdict::Costly { .. } => "有代价",
            Verdict::Protected { .. } => "受保护",
        }
    }

    /// 为什么这条是现在这个判定。UI 与审计都用它，不允许各处自己拼文案。
    pub fn explanation(&self) -> String {
        match self {
            Verdict::Reclaimable => "删掉后会自动重建，不影响使用".to_string(),
            Verdict::Costly { rebuild } => rebuild.summary(),
            Verdict::Protected { reason } => reason.to_string(),
        }
    }
}

/// 支撑判定的证据。不是日志——是给用户看的「凭什么这么说」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// 命中了哪条规则
    RuleHit { rule_id: String, pattern: String },
    /// 硬禁区命中
    HardDeny { rule: String, matched: String },
    /// 密钥指纹命中，只记模式名，绝不记录命中的内容本身
    SecretFingerprint { pattern: String, file: PathBuf },
    /// 被哪个进程占用
    InUse { pid: u32, name: String },
    /// 最近一次访问时间，用来判断「多少天没用过」
    LastUsed { days_ago: u64 },
    /// 这条路径是干什么的——AI 解释或内置文案都落在这里
    Description { text: String },
}

/// 一个候选清理项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub path: PathBuf,
    pub kind: ItemKind,
    /// 逻辑大小：把目录下所有文件大小相加
    pub bytes: u64,
    /// 实际占用：考虑硬链接与 APFS 稀疏文件后的真实节省量。
    /// 报「预计释放」时用这个，用 bytes 会重演 BuhoCleaner「发现 11.74 GB / 选中 10.16 GB」的信任事故。
    pub alloc_bytes: u64,
    pub files: u64,
    pub last_used: Option<std::time::SystemTime>,
    pub rule_id: Option<String>,
    pub verdict: Verdict,
    pub evidence: Vec<Evidence>,
    /// 是否值得优先处理：有代价、但已经很久没动过。
    ///
    /// 这是**给人看的建议**，不是第四种判定——它不改变 `verdict`，也不参与
    /// `default_checked()` 与无人值守清理的选择。默认永远为 false：
    /// 只有规则写了 `stale_days` 且实测闲置时间达到阈值时才会被置上。
    #[serde(default)]
    pub suggest_clean: bool,
}

impl Candidate {
    pub fn new(path: impl Into<PathBuf>, kind: ItemKind, verdict: Verdict) -> Self {
        Self {
            path: path.into(),
            kind,
            bytes: 0,
            alloc_bytes: 0,
            files: 0,
            last_used: None,
            rule_id: None,
            verdict,
            evidence: Vec::new(),
            suggest_clean: false,
        }
    }

    pub fn with_size(mut self, bytes: u64, alloc_bytes: u64, files: u64) -> Self {
        self.bytes = bytes;
        self.alloc_bytes = alloc_bytes;
        self.files = files;
        self
    }

    pub fn with_rule(mut self, rule_id: impl Into<String>) -> Self {
        self.rule_id = Some(rule_id.into());
        self
    }

    /// 追加一条证据。保持链式语义（返回 self），与 `with_size` 等构建器一致——
    /// 之前返回 `()` 会让 `let mut c = ...; c.push_evidence(..)` 之后再使用 `c` 编译失败。
    pub fn push_evidence(mut self, e: Evidence) -> Self {
        self.evidence.push(e);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reclaimable_is_checked_by_default() {
        assert!(Verdict::Reclaimable.default_checked());
        assert!(!Verdict::Costly {
            rebuild: RebuildCost::new(CostKind::Recompile, "重编")
        }
        .default_checked());
        assert!(!Verdict::Protected {
            reason: ProtectReason::UserRecord
        }
        .default_checked());
    }

    #[test]
    fn protected_is_not_actionable() {
        assert!(Verdict::Reclaimable.is_actionable());
        assert!(!Verdict::Protected {
            reason: ProtectReason::HardDeny { rule: "ssh".into() }
        }
        .is_actionable());
    }

    #[test]
    fn unknown_duration_says_so_instead_of_guessing() {
        let cost = RebuildCost::new(CostKind::Redownload, "重新拉取 crate").with_network();
        assert_eq!(cost.summary(), "重新下载，耗时取决于网络");
        assert!(cost.est_seconds.is_none());
    }

    #[test]
    fn durations_do_not_show_absurd_minutes() {
        assert_eq!(human_duration(45), "45 秒");
        assert_eq!(human_duration(300), "5 分钟");
        assert_eq!(human_duration(3600), "1 小时");
        assert_eq!(human_duration(7620), "2 小时 7 分钟");
    }

    #[test]
    fn secret_evidence_never_carries_the_secret_itself() {
        let e = Evidence::SecretFingerprint {
            pattern: "openai_key".into(),
            file: PathBuf::from("/tmp/a.yaml"),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("openai_key"));
        // 证据里只有模式名和文件名，没有值
        assert!(!json.contains("sk-"));
    }
}
