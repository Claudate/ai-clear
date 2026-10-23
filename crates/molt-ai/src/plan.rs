//! 方案：把 AI 的建议变成「可以拿给用户逐项确认、并且真的能执行」的清单。
//!
//! 这一层是 AI 与不可逆动作之间的唯一闸门，所以它**不相信 AI 的任何一句话**：
//!
//! - AI 只能提路径；路径必须**在本次扫描结果里真实存在**，凭空冒出来的一律拒。
//! - 受保护项（密钥、配置、会话历史、硬禁区、占用中）一律拒，即使 AI 说「可以删」。
//! - 目标的落点是本地算的：隔离区或归档目录。AI 不指定目标路径——让它指定，
//!   它迟早会写出一个 `/tmp/x` 或直接写原地删除。
//! - 释放空间按本地实测重算，不用 AI 报的 `est_bytes`。
//! - 用户点名「默认不要动」的东西（照片/合同/财务文件、系统目录、唯一副本）在这里再拦一次。
//!
//! 被拒的项不会静默消失：它们进 `rejected`，连同原因一起显示给用户。
//! 「AI 建议了 10 项、方案里只有 6 项」必须能解释清楚。

use std::path::{Path, PathBuf};

use molt_core::{Candidate, Verdict};

use crate::report::{AiReport, ProposalAction};

/// 一项待执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAction {
    /// 移到隔离区（可还原）
    Quarantine,
    /// 压缩后放进归档区（可还原，原目录腾空）
    Compress,
}

impl PlanAction {
    pub fn label(self) -> &'static str {
        match self {
            PlanAction::Quarantine => "移入隔离区",
            PlanAction::Compress => "压缩归档",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PlanAction::Quarantine => "quarantine",
            PlanAction::Compress => "compress",
        }
    }
}

/// 方案里的一项。字段齐全是刻意的：用户要能只看这一项就知道会发生什么。
#[derive(Debug, Clone)]
pub struct PlanItem {
    /// 扫描时用的真实绝对路径（执行时用它，不是用 `~/...` 显示串）
    pub source: PathBuf,
    pub source_display: String,
    /// 目标位置。可还原操作里，这就是撤销时读的地方。
    pub target: PathBuf,
    pub target_display: String,
    /// 本地实测的预计释放量（不是 AI 报的）
    pub bytes: u64,
    pub files: u64,
    pub action: PlanAction,
    /// 处理原因：AI 的说法 + 本地判定，两段都要有
    pub reason: String,
    /// AI 对这一条的解释（它是什么、为什么这样处理）。没有解释时为 `None`，
    /// 界面据此不画空行——「没有解释」与「解释为空」在用户眼里是同一件事。
    pub ai_note: Option<String>,
    /// 这一项是否被用户勾选（默认：可回收项勾上，有代价项不勾）
    pub checked: bool,
}

/// 被拒绝的建议。必须显示给用户，否则「AI 说的和方案里的不一致」无法解释。
#[derive(Debug, Clone)]
pub struct RejectedItem {
    pub path: String,
    pub reason: String,
}

/// 一份完整方案。
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub items: Vec<PlanItem>,
    pub rejected: Vec<RejectedItem>,
    /// AI 的总结
    pub summary: String,
    /// 生成方案时用的服务与模型，让用户能看见「这一份是谁出的」
    pub model: String,
}

impl Plan {
    pub fn checked_items(&self) -> impl Iterator<Item = &PlanItem> {
        self.items.iter().filter(|i| i.checked)
    }

    pub fn checked_count(&self) -> usize {
        self.checked_items().count()
    }

    pub fn checked_bytes(&self) -> u64 {
        self.checked_items().map(|i| i.bytes).sum()
    }

    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// 目标落点的根。隔离与归档都在应用数据目录下，
/// 这样「撤销」和「彻底清理」都能沿用已有的隔离区语义。
#[derive(Debug, Clone)]
pub struct PlanRoots {
    pub quarantine_root: PathBuf,
    pub archive_root: PathBuf,
    /// 生成方案的时间（作为批次目录名）
    pub batch: String,
}

impl PlanRoots {
    /// 用 `molt-act` 的路径约定构造。测试里可以换成临时目录。
    pub fn standard(batch: impl Into<String>) -> Self {
        let app = molt_act_paths::app_dir();
        Self {
            quarantine_root: app.join("quarantine"),
            archive_root: app.join("ai-archive"),
            batch: batch.into(),
        }
    }
}

/// 只借用一下路径约定，不依赖整个写盘层。
mod molt_act_paths {
    pub fn app_dir() -> std::path::PathBuf {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        home.join("Library")
            .join("Application Support")
            .join("app.molt")
    }
}

/// 用户明确说过默认不要自动动的东西。命中就拒，且理由要说人话。
const FORBIDDEN_LEAF: [&str; 12] = [
    "Photos Library.photoslibrary",
    "Pictures",
    "Documents",
    "Desktop",
    "Keychains",
    "Cookies",
    "Safari",
    "Chrome",
    "1Password",
    "Library/Keychains",
    ".ssh",
    ".gnupg",
];

/// 只看扩展名就足以判定「这是唯一副本风险很高」的类型。
const UNIQUE_COPY_EXT: [&str; 11] = [
    "jpg", "jpeg", "heic", "raw", "cr2", "nef", "pdf", "doc", "docx", "pages", "numbers",
];

impl Plan {
    /// 按 AI 的建议生成方案。
    ///
    /// `candidates` 是本次扫描的真实结果——AI 提到的路径必须在这里找得到，
    /// 否则视为幻觉并拒绝。
    pub fn build(
        report: &AiReport,
        candidates: &[Candidate],
        home: &str,
        roots: &PlanRoots,
        model: impl Into<String>,
    ) -> Self {
        let mut plan = Plan {
            summary: report.summary.clone(),
            model: model.into(),
            ..Default::default()
        };

        for p in &report.proposals {
            let action = match p.action {
                Some(ProposalAction::Keep) | None => continue,
                Some(ProposalAction::Quarantine) => PlanAction::Quarantine,
                Some(ProposalAction::Compress) => PlanAction::Compress,
            };

            // 1. 路径必须能在本次扫描结果里找到。AI 的 `~/...` 要展开回绝对路径。
            let abs = expand(home, &p.path);
            let Some(cand) = find_candidate(candidates, &abs) else {
                plan.rejected.push(RejectedItem {
                    path: p.path.clone(),
                    reason: "这一项不在本次扫描结果里，已忽略".to_string(),
                });
                continue;
            };

            // 2. 受保护项：任何理由都不动。这是产品承诺，不是防御性编程。
            if let Verdict::Protected { reason } = &cand.verdict {
                plan.rejected.push(RejectedItem {
                    path: p.path.clone(),
                    reason: format!("受保护项（{reason}），不会出现在方案里"),
                });
                continue;
            }

            // 3. 用户点名的「默认不要动」
            if let Some(reason) = forbidden(&cand.path, home) {
                plan.rejected.push(RejectedItem {
                    path: p.path.clone(),
                    reason,
                });
                continue;
            }

            // 4. 目标落点由本地决定
            let id = plan.items.len() as i64 + 1;
            let name = cand
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "item".to_string());
            let target = match action {
                PlanAction::Quarantine => roots
                    .quarantine_root
                    .join(&roots.batch)
                    .join(format!("{id}-{name}")),
                PlanAction::Compress => roots
                    .archive_root
                    .join(&roots.batch)
                    .join(format!("{id}-{name}.tar.gz")),
            };

            // 5. 体积按本地实测重算，AI 报的数字只当参考、不进方案
            let bytes = cand.alloc_bytes.max(cand.bytes);
            plan.items.push(PlanItem {
                source: cand.path.clone(),
                source_display: display(home, &cand.path),
                target_display: target.display().to_string(),
                target,
                bytes,
                files: cand.files,
                action,
                reason: compose_reason(&p.reason, &cand.verdict),
                ai_note: report.explanation_for(&p.path).and_then(|e| e.line()),
                // 与清单页一致：只有「可回收」默认勾上
                checked: matches!(cand.verdict, Verdict::Reclaimable),
            });
        }

        // 同一路径被 AI 提了两次：保留第一条，后面按重复拒掉。
        // 否则执行时第二条必然报「不存在」，看起来像失败。
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut deduped = Vec::with_capacity(plan.items.len());
        for item in plan.items.into_iter() {
            if seen.insert(item.source.clone()) {
                deduped.push(item);
            } else {
                plan.rejected.push(RejectedItem {
                    path: item.source_display.clone(),
                    reason: "同一路径被重复建议，已合并为一条".to_string(),
                });
            }
        }
        plan.items = deduped;

        plan
    }
}

/// 把 `~/x` 展开成绝对路径；已经是绝对路径就原样返回。
fn expand(home: &str, p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        Path::new(home).join(rest)
    } else if p == "~" {
        PathBuf::from(home)
    } else {
        PathBuf::from(p)
    }
}

/// 逐条解释的映射：扫描时的绝对路径 → 一句话说明。
///
/// 与 `Plan::items` 不同，这里的覆盖面是**整份扫描结果**：
/// 受保护项、模型建议保持不动的项、以及在方案层被拒的项都会出现在映射里。
/// 清单页要把说明挂在每一行上，只覆盖「可执行项」等于告诉用户
/// 「你看见的大部分数据 AI 没看过」——而这正是这次要解决的问题。
///
/// 模型给出的路径不一定在扫描结果里（幻觉），所以只接受能对上的；
/// 对不上的静默丢弃，不编一个近似匹配。
pub fn explanations(
    report: &AiReport,
    candidates: &[Candidate],
    home: &str,
) -> std::collections::HashMap<std::path::PathBuf, String> {
    let mut out = std::collections::HashMap::new();
    for e in &report.explanations {
        let Some(line) = e.line() else { continue };
        let abs = expand(home, &e.path);
        if let Some(c) = candidates.iter().find(|c| c.path == abs) {
            out.insert(c.path.clone(), line);
        }
    }
    out
}
fn find_candidate<'a>(items: &'a [Candidate], abs: &Path) -> Option<&'a Candidate> {
    items.iter().find(|c| c.path == abs)
}

/// 「默认不应自动执行」清单在方案层的落地。
fn forbidden(path: &Path, home: &str) -> Option<String> {
    // 只能在授权根内动手：家目录之外（系统目录、别的用户）一律拒
    if !path.starts_with(home) {
        return Some("不在家目录内（可能是系统目录），不会处理".to_string());
    }

    let s = path.to_string_lossy();
    for leaf in FORBIDDEN_LEAF {
        if s.contains(leaf) {
            return Some(format!("属于用户数据区（{leaf}），不会自动处理"));
        }
    }

    // 唯一副本风险：照片 / 合同 / 财务类文件单独一个的情况不自动处理。
    //
    // 只看扩展名，不看 `is_file()`：这个判断发生在生成方案时，而方案可能在
    // 文件已被别的进程移走后生成；等真去 stat 一次再决定要不要拦，等于给
    // 「文件刚好不在」留了一个放行口。宁可多拦——被拦下的项用户仍能在清单页手动选。
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_lowercase();
        if UNIQUE_COPY_EXT.contains(&ext.as_str()) {
            return Some("可能是照片 / 合同 / 财务文件，不自动处理".to_string());
        }
    }
    None
}

/// AI 的说法在前、本地判定在后。用户需要同时知道「它为什么建议删」和
/// 「我们凭什么确认可以删」。
fn compose_reason(ai: &str, verdict: &Verdict) -> String {
    let ai = ai.trim();
    let local = verdict.explanation();
    if ai.is_empty() {
        local
    } else {
        format!("{ai}（本地判定：{local}）")
    }
}

fn display(home: &str, path: &Path) -> String {
    match path.strip_prefix(Path::new(home)) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// 把一条方案项转成执行器需要的候选条目。
///
/// 方案项本身已经过核对，这里重建 `Candidate` 是为了复用 `molt-act` 的执行路径，
/// 而不是为 AI 另写一套移动逻辑——那正是「另起一套」最容易出事的地方。
pub fn to_candidate(item: &PlanItem, original: &Candidate) -> Candidate {
    let mut c = original.clone();
    c.path = item.source.clone();
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{Proposal, ProposalAction};
    use molt_core::{CostKind, ItemKind, ProtectReason, RebuildCost};

    const HOME: &str = "/Users/me";

    fn roots() -> PlanRoots {
        PlanRoots {
            quarantine_root: PathBuf::from("/app/quarantine"),
            archive_root: PathBuf::from("/app/ai-archive"),
            batch: "2026-10-22T10-00-00".into(),
        }
    }

    fn cand(path: &str, bytes: u64, v: Verdict) -> Candidate {
        Candidate::new(path, ItemKind::Dir, v).with_size(bytes, bytes, 4)
    }

    fn report(items: Vec<(&str, ProposalAction, &str)>) -> AiReport {
        AiReport {
            summary: "测试".into(),
            proposals: items
                .into_iter()
                .map(|(p, a, r)| Proposal {
                    path: p.into(),
                    action: Some(a),
                    reason: r.into(),
                    est_bytes: 999_999,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn builds_items_with_local_paths_and_targets() {
        let c = vec![cand(
            "/Users/me/project/x/target",
            1 << 30,
            Verdict::Reclaimable,
        )];
        let r = report(vec![(
            "~/project/x/target",
            ProposalAction::Quarantine,
            "构建产物",
        )]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "deepseek-chat");

        assert_eq!(plan.items.len(), 1);
        let it = &plan.items[0];
        assert_eq!(it.source, PathBuf::from("/Users/me/project/x/target"));
        assert!(it.target.starts_with("/app/quarantine/2026-10-22T10-00-00"));
        assert_eq!(it.bytes, 1 << 30, "体积必须按本地实测");
        assert!(it.reason.contains("本地判定"), "原因里要有本地判定");
        assert!(it.checked, "可回收项默认勾选");
    }

    #[test]
    fn protected_items_are_never_in_the_plan() {
        let c = vec![cand(
            "/Users/me/.claude/projects",
            1 << 20,
            Verdict::Protected {
                reason: ProtectReason::UserRecord,
            },
        )];
        let r = report(vec![(
            "~/.claude/projects",
            ProposalAction::Quarantine,
            "占空间",
        )]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert!(plan.items.is_empty(), "受保护项不能进方案");
        assert_eq!(plan.rejected.len(), 1);
        assert!(plan.rejected[0].reason.contains("受保护"));
    }

    #[test]
    fn hallucinated_paths_are_rejected() {
        let c = vec![cand("/Users/me/a", 1, Verdict::Reclaimable)];
        let r = report(vec![("~/not-in-scan", ProposalAction::Quarantine, "猜的")]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert!(plan.items.is_empty());
        assert!(plan.rejected[0].reason.contains("不在本次扫描结果里"));
    }

    #[test]
    fn system_paths_are_rejected_even_if_scanned() {
        let c = vec![cand("/usr/local/lib", 1 << 30, Verdict::Reclaimable)];
        let r = report(vec![("/usr/local/lib", ProposalAction::Quarantine, "很大")]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert!(plan.items.is_empty());
        assert!(plan.rejected[0].reason.contains("家目录"));
    }

    #[test]
    fn photo_and_document_files_are_not_auto_handled() {
        let c = vec![cand(
            "/Users/me/backup/contract.pdf",
            4096,
            Verdict::Costly {
                rebuild: RebuildCost::new(CostKind::Redownload, "重新下载"),
            },
        )];
        let r = report(vec![(
            "~/backup/contract.pdf",
            ProposalAction::Quarantine,
            "重复文件",
        )]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert!(plan.items.is_empty(), "唯一副本风险的文件不自动处理");
        assert!(plan.rejected[0].reason.contains("合同"));
    }

    #[test]
    fn compress_targets_the_archive_root() {
        let c = vec![cand("/Users/me/logs", 100, Verdict::Reclaimable)];
        let r = report(vec![("~/logs", ProposalAction::Compress, "很少用")]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert_eq!(plan.items[0].action, PlanAction::Compress);
        assert!(plan.items[0]
            .target
            .to_string_lossy()
            .starts_with("/app/ai-archive/"));
        assert!(plan.items[0].target.to_string_lossy().ends_with(".tar.gz"));
    }

    #[test]
    fn duplicate_proposals_collapse_into_one_item() {
        let c = vec![cand("/Users/me/a", 10, Verdict::Reclaimable)];
        let r = report(vec![
            ("~/a", ProposalAction::Quarantine, "一"),
            ("~/a", ProposalAction::Quarantine, "二"),
        ]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert_eq!(plan.items.len(), 1);
        assert!(plan.rejected.iter().any(|r| r.reason.contains("重复")));
    }

    #[test]
    fn costly_items_are_not_checked_by_default() {
        let c = vec![cand(
            "/Users/me/project/target",
            1 << 30,
            Verdict::Costly {
                rebuild: RebuildCost::new(CostKind::Recompile, "重编约 3 分钟"),
            },
        )];
        let r = report(vec![(
            "~/project/target",
            ProposalAction::Quarantine,
            "构建产物",
        )]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert_eq!(plan.items.len(), 1);
        assert!(!plan.items[0].checked, "有代价项不默认勾选");
        assert_eq!(plan.checked_count(), 0);
    }

    #[test]
    fn ai_estimated_bytes_never_reach_the_plan() {
        let c = vec![cand("/Users/me/a", 1234, Verdict::Reclaimable)];
        let r = report(vec![("~/a", ProposalAction::Quarantine, "x")]);
        let plan = Plan::build(&r, &c, HOME, &roots(), "m");
        assert_eq!(plan.items[0].bytes, 1234);
        assert_eq!(plan.total_bytes(), 1234);
    }
}
