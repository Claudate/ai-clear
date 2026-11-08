//! 单向数据流：界面只读状态、只发 `Action`，改状态的地方只有 `apply` 一个。
//!
//! 为什么要有这一层：在此之前界面代码直接伸手改 `AppState`，一个 2287 行的
//! 页面里有 82 处写入。后果是「点了某一行之后状态应该是什么样」只能靠截图验，
//! 而截图看不出 `checked` 里到底多了哪条路径；也拦不住界面绕过约束——比如
//! 「受保护项永不进勾选」这件事，原先只能靠每个调用点自觉。
//!
//! 现在分成两半：
//! - [`apply`] 是**纯函数**，不碰磁盘、不起线程、不发网络，只改 `AppState`。
//!   因此它可以被单测直接调用，断言「点了这一下，状态变成什么」。
//! - 需要真的跑线程或碰盘的动作，`apply` 只把意图追加到 [`Effect`] 列表里，
//!   由 [`crate::app`] 在这一帧结束前执行——那是唯一有副作用的地方。
//!
//! 这条分界是整套改动的价值所在：新加一个界面动作时，只要它的状态变化写进
//! `apply`，就会自动获得可测性；而绕过它直接改 `AppState` 会写不出来，
//! 因为页面函数拿到的是 `&AppState`。

use crate::state::{AppState, Filter, NextStep, Page, Priority, StatusMessage};

/// 界面能发出的全部动作。
///
/// 一个 `Action` 描述「用户做了什么」，不是一个具体字段的新值——这样将来给
/// 同一个动作补额外后果（例如「切到设置页时顺带重读配置」）只改 `apply` 一处。
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // ---- 导航 ----
    GoPage(Page),
    /// 结果条上的「下一步」按钮
    FollowNextStep(NextStep),
    DismissStatus,
    /// 出错提示条上的「去设置」
    GoSettingsForError,

    // ---- 工作台的筛选与列表 ----
    SetFilter(Filter),
    SetQuery(String),
    /// 输入过程中的草稿：只暂存，不过滤。真正过滤发生在 `CommitQuery`。
    SetQueryDraft(String),
    /// 把草稿提交成生效的搜索词。
    CommitQuery,
    ClearQuery,
    /// 滚动到清单开头的那一次请求已经用掉
    ConsumeScrollTo,
    DismissReport,

    // ---- 勾选 ----
    ToggleCheck(String),
    /// 只选「可回收」那一档
    SelectReclaimable,
    /// 只选「建议清理」那一档
    SelectSuggested,
    /// 当前筛选下能动手的全部选中
    SelectAllVisible,
    /// 整组勾上或清掉某一档（分组标题上的三态框）
    ToggleGroup(Priority),
    /// 展开或收起项目/工具子组。
    ToggleSubgroupOpen(Priority, String),
    /// 子组范围内全选或清空；状态层会再次核对路径的判定与筛选。
    ToggleSubgroupSelection(Priority, Vec<String>),
    ClearSelection,

    // ---- 清理与确认 ----
    /// 把当前勾选装进待确认批次。`mode` 决定隔离区还是直接清理。
    PrepareClean(molt_act::CleanMode),
    CancelPendingClean,
    /// 确认框按下了确定：把已装好的批次交给后台去跑。
    /// 与 `PrepareClean` 分开，是因为两者之间隔着用户的一次确认——
    /// 合成一个动作就无法表达「装了但还没确认」这个状态。
    RunPendingClean,
    RunPendingPurge,

    // ---- 隔离区 ----
    ToggleQuarantineCheck(i64),
    /// 重新读一遍临时空间的内容
    LoadQuarantine,
    /// 还原单独一项（行上的「还原」按钮）
    RestoreOne(i64),
    /// 还原所勾选的每一项。逐条还原，不是一次性 rename ——
    /// 所以它是一个 Effect，由执行层逐条跑并汇总成败。
    RestoreSelected,
    SelectAllQuarantine,
    ClearQuarantineSelection,
    PreparePurge,
    /// 只彻底删除某一条。勾选先对齐成「只有它」，再走与批量同一个确认框——
    /// 用户看到的、确认的就是实际会删的，不会顺手带上别处勾着的条目。
    PreparePurgeOne(i64),
    CancelPendingPurge,

    // ---- 历史 ----
    LoadHistory,
    SetHistoryFilter(crate::state::HistoryFilter),

    // ---- 设置 ----
    /// 大/中/小字号；非法档位忽略，默认仍为大字号。
    SetFontScale(u8),
    ToggleSettingsAdvanced,
    SetKeyDraft(String),
    ClearKeyDraft,
    /// 落盘当前密钥（写盘 + 收权限都在执行层做）
    SaveApiKey,
    /// 切换 AI 服务
    SetAiProvider(String),
    /// 切换 AI 模型
    SetAiModel(String),
    /// 改用本地判定引擎 jev
    UseJev,
    /// 重新读一遍 ai.toml
    ReloadAiConfig,

    // ---- 自动化 ----
    SetAutoEnabled(bool),
    SetAutoSchedule(crate::schedule::ScheduleConfig),
    /// 往前看：把草稿丢弃，回到已生效的那份配置
    DiscardAutoDraft,
    /// 重新读一遍定时配置（读失败卡片上的那个按钮）
    ReloadAutoConfig,
    /// 重新问一次 launchd：任务装了没、上次什么时候跑过
    RefreshAutoStatus,
    /// 用已生效的配置跑一次
    RunScheduledNow,
    /// 按草稿落盘（启用与否取自草稿本身）
    SaveDraft,
    /// 保存并关闭定时任务（同时把 launchd 里的那个卸掉）
    SaveAutoDisabled,

    // ---- 会被 apply 直接执行、不产生 Effect 的读取请求 ----
    ReloadRules,
    /// 让 AI 解释这次扫描
    Analyze,

    /// 打开 / 关闭首页条目的详情抽屉（只读的简介面板）
    OpenDetail(String),
    CloseDetail,
    /// 在访达里定位这个路径（选中它，而不是打开它）
    RevealInFinder(String),
    /// 在访达里打开临时空间
    RevealQuarantine,

    // ---- 扫描控制 ----
    /// 暂停当前扫描：后台在下一个检查点停下，已完成的进度保留。
    PauseScan,
    /// 从暂停处继续扫描。
    ResumeScan,
    /// 取消当前扫描：任务结束，不能继续。
    CancelScan,

    // ---- 重复文件 ----
    /// 启动重复文件检测（后台任务，只读）。
    StartDuplicateScan,
    /// 把路径复制到剪贴板。
    CopyPath(String),
}

/// `apply` 判断出「这件事要真的去跑」时追加的意图。
///
/// 它之所以不在 `apply` 里直接执行：起线程、读数据库、碰磁盘都不是纯操作，
/// 一旦混进去，`apply` 就不能再被单测直接调用，这一层重构的意义也就没了。
///
/// 没有派生 `PartialEq`：它描述的是一次要发生的副作用，不是可比较的值。
/// 测试要断言的是「产生了哪几类意图」，对着 `matches!` 或 `Effect::kind` 判即可。
#[derive(Debug, Clone)]
pub enum Effect {
    Scan,
    Analyze,
    Clean(Box<crate::state::PendingClean>),
    Purge(Box<crate::state::PendingPurge>),
    RestoreQuarantine(i64),
    /// 批量还原：执行层逐条跑，汇总成一份报告
    RestoreQuarantineBatch(Vec<i64>),
    ReloadQuarantine,
    ReloadHistory,
    ReloadAiConfig,
    /// 重读规则库（用户可能往 rules 目录里加了规则）。
    ///
    /// 与 `ReloadAiConfig` 分开：那个只读 ai.toml，规则库是另一份文件、
    /// 另一套解析，合成一个 Effect 会让「重新读取配置」做什么变得含糊。
    ReloadRules,
    /// AI 配置变了，需要写回 ai.toml
    SaveAiConfig,
    /// 把界面上的密钥落盘
    SaveApiKey,
    EnsureAutoLoaded,
    SaveAuto,
    /// 保存但关闭：写盘后要从 launchd 卸掉任务
    SaveAutoDisabled,
    /// 重新问 launchd 要一次状态（读 plist / launchctl）
    RefreshAutoStatus,
    /// 按已生效配置跑一次定时清理
    RunScheduledNow,
    RefreshDisk,
    /// 回收已到期的隔离项
    CollectExpired,
    /// 在访达里选中一个路径（`open -R`）
    RevealInFinder(String),
    /// 打开临时空间的目录（`open` 是只读的，不动任何内容）
    RevealQuarantine,
    /// 启动重复文件检测。
    ScanDuplicates,
    /// 把路径写入系统剪贴板。
    CopyPath(String),
}

/// 唯一改状态的地方。纯函数：只读 `state`、只写 `state`，返回要执行的动作。
///
/// 注意这里**不检查** `can_select()` 之类的前置条件就够用了：那些守卫原本散在
/// 每个界面调用点，现在收敛到这里，语义反而更强——界面画成什么样不影响
/// 「即使点了也不该生效」这条规则。
pub fn apply(state: &mut AppState, action: Action) -> Vec<Effect> {
    match action {
        // ---- 导航 ----
        Action::GoPage(page) => return go_page(state, page),
        Action::GoSettingsForError => {
            state.status = None;
            return go_page(state, Page::Settings);
        }
        Action::FollowNextStep(next) => {
            match next {
                NextStep::Quarantine => return go_page(state, Page::Quarantine),
                NextStep::History => return go_page(state, Page::History),
                NextStep::Retry => {
                    if state.can_start() {
                        return vec![Effect::Scan];
                    }
                }
            }
            state.last_op = None;
        }
        Action::DismissStatus => state.status = None,

        // ---- 筛选与列表 ----
        Action::SetFilter(f) => {
            state.filter = f;
            // 清单常显，切筛选只需要把视图滚到清单开头。
            state.scroll_to_list = true;
        }
        Action::SetQuery(q) => {
            state.query = q;
            state.scroll_to_list = true;
        }
        Action::SetQueryDraft(q) => state.query_draft = q,
        Action::CommitQuery => {
            // 先写回草稿再提交：用户可能正在输入框里按回车，此时 `query_draft`
            // 已经是最新的，但 `query` 还是上一份。
            state.query = state.query_draft.clone();
            state.scroll_to_list = true;
        }
        Action::ClearQuery => {
            state.query.clear();
            state.query_draft.clear();
            state.scroll_to_list = true;
        }
        Action::ConsumeScrollTo => state.scroll_to_list = false,
        Action::DismissReport => state.last_op = None,

        // ---- 勾选 ----
        //
        // 执行期间改勾选会让用户以为这一批变了，而实际执行的是按下确认那一刻
        // 的清单。守卫放这里，所有入口自动都守住了。
        Action::ToggleCheck(path) => state.toggle(&path),
        Action::SelectReclaimable => {
            if state.can_select() {
                state.select_reclaimable();
            }
        }
        Action::SelectSuggested => {
            if state.can_select() {
                state.select_suggested();
            }
        }
        Action::SelectAllVisible => {
            if state.can_select() {
                state.select_all_visible();
            }
        }
        Action::ToggleGroup(pr) => {
            if state.can_select() {
                state.toggle_group(pr);
            }
        }
        Action::ToggleSubgroupOpen(pr, key) => state.toggle_subgroup_open(pr, key),
        Action::ToggleSubgroupSelection(pr, paths) => state.toggle_subgroup_selection(pr, &paths),
        Action::ClearSelection => {
            if state.can_select() {
                state.clear_selection();
            }
        }

        // ---- 清理 ----
        Action::PrepareClean(mode) => {
            // 先把勾选变成批次，再让 Effect 去跑——判据留在这一层，
            // 页面就只负责「用户按了哪个按钮」。
            state.pending_clean = state.pending_from_checked(mode);
        }
        Action::CancelPendingClean => state.pending_clean = None,
        Action::RunPendingClean => {
            if let Some(effect) = clean_from_pending(state) {
                return vec![effect];
            }
        }

        // ---- 隔离区 ----
        Action::ToggleQuarantineCheck(id) => {
            if state.can_select() {
                state.toggle_quarantine(id);
            }
        }
        Action::LoadQuarantine => return vec![Effect::ReloadQuarantine],
        Action::RestoreOne(id) => return vec![Effect::RestoreQuarantine(id)],
        Action::RestoreSelected => {
            if !state.can_start() {
                state.status = Some(crate::state::StatusMessage::warn(
                    state.activity().blocking_hint().to_string(),
                ));
            } else {
                // 只还原「可还原」的那些：已过期的条目在库里也还在，
                // 但内容已经被彻底删掉了，把它们塞进批次只会在执行时失败。
                let ids: Vec<i64> = state
                    .quarantine
                    .iter()
                    .filter(|r| r.restorable && state.quarantine_checked.contains(&r.id))
                    .map(|r| r.id)
                    .collect();
                if !ids.is_empty() {
                    return vec![Effect::RestoreQuarantineBatch(ids)];
                }
            }
        }
        Action::SelectAllQuarantine => {
            if state.can_select() {
                state.select_all_quarantine();
            }
        }
        Action::ClearQuarantineSelection => {
            if state.can_select() {
                state.clear_quarantine_selection();
            }
        }
        Action::PreparePurge => {
            state.pending_purge = state.pending_purge_from_checked();
        }
        Action::PreparePurgeOne(id) => {
            // 只构造一条独立的待确认批次，**不动** `quarantine_checked`。
            //
            // 以前这里把整份勾选换成这一条：用户勾了 6 项、顺手点了某行的
            // 「彻底删除…」，结果确认框里只有 1 项，之前勾的 5 个悄悄没了，
            // 连取消确认框也换不回来。
            state.pending_purge = state
                .quarantine
                .iter()
                .find(|r| r.id == id && r.restorable)
                .map(|r| crate::state::PendingPurge {
                    bytes: r.bytes,
                    items: vec![crate::state::PendingPurgeItem {
                        id: r.id,
                        original: std::path::PathBuf::from(&r.original),
                        bytes: r.bytes,
                    }],
                });
        }
        Action::CancelPendingPurge => state.pending_purge = None,
        Action::RunPendingPurge => {
            if let Some(effect) = purge_from_pending(state) {
                return vec![effect];
            }
        }

        Action::LoadHistory => return vec![Effect::ReloadHistory],
        Action::SetHistoryFilter(f) => state.history_filter = f,

        // ---- 设置 ----
        Action::SetFontScale(index) => {
            if let Some(scale) = crate::theme::Theme::scale_for_index(index) {
                state.display_scale = scale;
            }
        }
        Action::ToggleSettingsAdvanced => state.settings_advanced = !state.settings_advanced,
        // 密钥只在内存里，保存后立即清空——它绝不能跟着窗口状态被持久化。
        Action::SetKeyDraft(s) => state.key_draft = s,
        Action::ClearKeyDraft => state.key_draft.clear(),
        Action::SaveApiKey => {
            if !state.key_draft.trim().is_empty() && !state.is_busy() {
                return vec![Effect::SaveApiKey];
            }
        }
        Action::SetAiProvider(id) => {
            if !state.is_busy() {
                // 换服务时把那半截密钥清掉：它是上一家服务填的，留着
                // 会让「保存密钥」把 A 家的 key 写进 B 家的配置里。
                if state.ai.config.active_provider != id {
                    state.key_draft.clear();
                }
                state.ai.config.set_provider(&id);
                return vec![Effect::SaveAiConfig];
            }
        }
        Action::SetAiModel(id) => {
            if !state.is_busy() {
                // 同一家服务下换模型，密钥仍属于这家，草稿要留着。
                state.ai.config.active_model = id;
                return vec![Effect::SaveAiConfig];
            }
        }
        Action::UseJev => {
            if !state.is_busy() {
                let pid = state
                    .ai
                    .config
                    .providers
                    .iter()
                    .find(|x| {
                        x.models
                            .iter()
                            .any(|m| m.id == molt_ai::config::JEV_MODEL_ID)
                    })
                    .map(|x| x.id.clone());
                if let Some(pid) = pid {
                    state.ai.config.set_provider(&pid);
                    state.ai.config.active_model = molt_ai::config::JEV_MODEL_ID.to_string();
                    return vec![Effect::SaveAiConfig];
                }
            }
        }
        Action::ReloadAiConfig => {
            if !state.is_busy() {
                state.ai.reload_config();
                state.status = Some(crate::state::StatusMessage::info("已重新读取 AI 配置"));
            }
        }

        // ---- 自动化 ----
        Action::SetAutoEnabled(on) => {
            if let Some(d) = state.auto.draft.as_mut() {
                d.enabled = on;
            } else {
                let mut d = state.auto.config.clone();
                d.enabled = on;
                state.auto.draft = Some(d);
            }
        }
        Action::SetAutoSchedule(cfg) => state.auto.draft = Some(cfg),
        Action::DiscardAutoDraft => state.auto.draft = Some(state.auto.config.clone()),
        Action::ReloadAutoConfig => {
            state.auto.loaded = false;
            return vec![Effect::EnsureAutoLoaded];
        }
        Action::SaveDraft => {
            state.auto.saving = true;
            return vec![Effect::SaveAuto];
        }
        Action::SaveAutoDisabled => {
            state.auto.saving = true;
            return vec![Effect::SaveAutoDisabled];
        }
        Action::RefreshAutoStatus => return vec![Effect::RefreshAutoStatus],
        Action::RunScheduledNow => {
            if !state.can_start() {
                state.status = Some(crate::state::StatusMessage::warn(
                    state.activity().blocking_hint().to_string(),
                ));
            } else if state.rules.is_none() {
                state.status = Some(crate::state::StatusMessage::error(
                    "规则库没有加载成功，无法试跑",
                ));
            } else {
                // 草稿还在没保存时试跑，用的是已生效的那份 —— 与「保存后才生效」一致。
                return vec![Effect::RunScheduledNow];
            }
        }

        Action::ReloadRules => return vec![Effect::ReloadRules],
        Action::Analyze => {
            if state.can_start() {
                return vec![Effect::Analyze];
            }
        }

        // ---- 扫描控制 ----
        Action::PauseScan => {
            if let Some(job) = state.job.as_ref() {
                job.control.pause();
            }
        }
        Action::ResumeScan => {
            if let Some(job) = state.job.as_ref() {
                job.control.resume();
            }
        }
        Action::CancelScan => {
            if let Some(job) = state.job.as_ref() {
                job.control.cancel();
            }
        }
        Action::StartDuplicateScan => {
            if state.duplicate_job.is_none() {
                return vec![Effect::ScanDuplicates];
            }
        }
        Action::CopyPath(path) => return vec![Effect::CopyPath(path)],

        // ---- 工具 ----
        Action::RevealInFinder(path) => return vec![Effect::RevealInFinder(path)],
        Action::RevealQuarantine => return vec![Effect::RevealQuarantine],

        // ---- 详情抽屉 ----
        Action::OpenDetail(path) => state.detail_path = Some(path),
        Action::CloseDetail => state.detail_path = None,
    }
    Vec::new()
}

/// 切页并回报该页需要的加载。
///
/// 抽成一处是为了让「切到临时空间要重读列表」这类副作用只有一份定义：
/// 界面与测试走同一条路径，就不会各理解一遍。
fn go_page(state: &mut AppState, page: Page) -> Vec<Effect> {
    if state.page == page {
        return Vec::new();
    }
    state.page = page;
    match page {
        Page::Quarantine => vec![Effect::ReloadQuarantine],
        Page::History => vec![Effect::ReloadHistory],
        Page::Automation => {
            state.auto.loaded = false;
            state.auto.draft = None;
            vec![Effect::EnsureAutoLoaded]
        }
        Page::Settings => vec![Effect::ReloadAiConfig],
        Page::Workbench => Vec::new(),
    }
}

/// 把一个待确认批次换成「真的开始清理」这一意图。
///
/// 从 `apply` 里单独拎出来，是因为它发生在用户按下确认框的确定键之后：
/// 那一刻 `pending_clean` 已经被 `apply` 装好，这里只负责把它交给后台。
pub fn clean_from_pending(state: &mut AppState) -> Option<Effect> {
    let pending = state.pending_clean.clone()?;
    state.pending_clean = None;
    Some(Effect::Clean(Box::new(pending)))
}

/// 同理，隔离区的彻底删除。
pub fn purge_from_pending(state: &mut AppState) -> Option<Effect> {
    let pending = state.pending_purge.clone()?;
    state.pending_purge = None;
    Some(Effect::Purge(Box::new(pending)))
}

/// 一条消息构成「必须看见」的警告时用的统一出口，避免各页拼不同的话。
pub fn warn(state: &mut AppState, text: impl Into<String>) {
    state.status = Some(StatusMessage::warn(text));
}

/// 受保护项永远不能勾选；AI 建议保留的本地可回收项仍能由用户决定。
pub fn is_selectable(state: &AppState, path: &str) -> bool {
    let Some(r) = &state.result else {
        return false;
    };
    r.items
        .iter()
        .find(|c| c.path.display().to_string() == path)
        .map(AppState::item_actionable)
        .unwrap_or(false)
}

/// 给测试用：某一档的项数，口径与列表分节一致。
pub fn count_of(state: &AppState, p: Priority) -> usize {
    state.count_of(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::ScanResult;
    use molt_core::{Candidate, CostKind, ItemKind, ProtectReason, RebuildCost, Verdict};
    use std::sync::Arc;

    fn cand(path: &str, bytes: u64, v: Verdict) -> Candidate {
        Candidate::new(path, ItemKind::Dir, v).with_size(bytes, bytes, 1)
    }

    fn state_with(items: Vec<Candidate>) -> AppState {
        let mut s = AppState::new("/Users/me".into());
        s.apply_result(Arc::new(ScanResult {
            items,
            rule_ids: Vec::new(),
            denied_roots: vec![],
            elapsed_secs: 0.0,
            from_cache: false,
        }));
        s
    }

    /// 占住「正在还原」这个活动位，供互斥相关的断言使用。
    ///
    /// 自己建一个临时目录里的空审计库，**不碰真实 home**：以前这里直接
    /// `AuditStore::open(paths::audit_db())`，跑测试就会在用户真正的
    /// 应用数据目录里建库。
    fn occupy_restore(s: &mut AppState) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "molt-occupy-restore-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let store = molt_act::AuditStore::open(&dir.join("audit.db")).expect("建审计库");
        let q = molt_act::Quarantine::new(dir.join("q"), "restore");
        // 空 id 列表：任务立刻跑完，但 `restore_job` 在这一帧仍是非 None，
        // 互斥闸门（`can_start` / `can_select`）照常生效。
        s.restore_job = Some(crate::worker::RestoreJob::start(Vec::new(), store, q));
    }

    fn protected() -> Verdict {
        Verdict::Protected {
            reason: ProtectReason::UserRecord,
        }
    }

    fn costly() -> Verdict {
        Verdict::Costly {
            rebuild: RebuildCost::new(CostKind::Recompile, "重编"),
        }
    }

    // ---- 导航 ----

    #[test]
    fn go_page_reports_the_load_it_owes() {
        // 切页不是只改一个字段：临时空间与历史都要重读，自动化要问配置。
        // 这些副作用原先写死在界面的点击处理里，现在由 Effect 声明。
        let mut s = AppState::new("/Users/me".into());
        let fx = apply(&mut s, Action::GoPage(Page::Quarantine));
        assert_eq!(s.page, Page::Quarantine);
        assert!(matches!(fx.as_slice(), [Effect::ReloadQuarantine]));

        let fx = apply(&mut s, Action::GoPage(Page::History));
        assert!(matches!(fx.as_slice(), [Effect::ReloadHistory]));

        let fx = apply(&mut s, Action::GoPage(Page::Automation));
        assert!(matches!(fx.as_slice(), [Effect::EnsureAutoLoaded]));
        assert!(
            !s.auto.loaded,
            "进自动化页必须把 loaded 置回 false 才会重读"
        );

        let fx = apply(&mut s, Action::GoPage(Page::Settings));
        assert!(matches!(fx.as_slice(), [Effect::ReloadAiConfig]));

        let fx = apply(&mut s, Action::GoPage(Page::Workbench));
        assert!(fx.is_empty(), "工作台不需要额外加载");
    }

    #[test]
    fn go_page_to_the_same_page_does_no_work() {
        // 点当前页的图标不该触发一次多余的重读：用户点它往往只是误触。
        let mut s = AppState::new("/Users/me".into());
        let fx = apply(&mut s, Action::GoPage(Page::Workbench));
        assert!(fx.is_empty());
    }

    // ---- 勾选：这套重构要守住的核心约束 ----

    #[test]
    fn toggle_check_flips_both_ways() {
        // 先前的写法是「先删再查」，结果每次都会重新插入，取消勾选永远不生效。
        let mut s = state_with(vec![cand("/a", 1, Verdict::Reclaimable)]);
        assert!(s.is_checked("/a"), "可回收项默认勾选");

        apply(&mut s, Action::ToggleCheck("/a".into()));
        assert!(!s.is_checked("/a"), "点一下要取消");

        apply(&mut s, Action::ToggleCheck("/a".into()));
        assert!(s.is_checked("/a"), "再点一下要重新勾上");
    }

    #[test]
    fn protected_rows_can_never_be_checked() {
        // 受保护项在界面上根本点不到，但这条约束不该只靠界面画得对不对。
        let mut s = state_with(vec![cand("/k", 99, protected())]);
        apply(&mut s, Action::SelectAllVisible);
        assert_eq!(s.checked_count(), 0, "「全选本页」不能把受保护项带进来");
    }

    #[test]
    fn subgroup_actions_keep_protected_and_other_rows_untouched() {
        let mut s = state_with(vec![
            cand("/a", 1, Verdict::Reclaimable),
            cand("/b", 2, Verdict::Reclaimable),
            cand("/k", 3, protected()),
        ]);
        s.clear_selection();
        let requested = vec!["/a".into(), "/k".into()];
        apply(
            &mut s,
            Action::ToggleSubgroupSelection(Priority::Safe, requested.clone()),
        );
        assert!(s.is_checked("/a"));
        assert!(!s.is_checked("/b"));
        assert!(!s.is_checked("/k"));
        apply(
            &mut s,
            Action::ToggleSubgroupSelection(Priority::Safe, requested),
        );
        assert_eq!(s.checked_count(), 0);

        let key = "project:p".to_string();
        apply(
            &mut s,
            Action::ToggleSubgroupOpen(Priority::Safe, key.clone()),
        );
        assert!(s.collapsed_groups.contains(&(Priority::Safe, key.clone())));
        apply(
            &mut s,
            Action::ToggleSubgroupOpen(Priority::Safe, key.clone()),
        );
        assert!(!s.collapsed_groups.contains(&(Priority::Safe, key)));

        occupy_restore(&mut s);
        apply(
            &mut s,
            Action::ToggleSubgroupSelection(Priority::Safe, vec!["/b".into()]),
        );
        assert!(!s.is_checked("/b"), "执行期间不能改变子组选择");
    }

    #[test]
    fn ai_keep_is_manual_choice_but_protected_items_stay_locked() {
        let mut s = state_with(vec![
            cand("/a", 10, Verdict::Reclaimable),
            cand("/k", 10, protected()),
        ]);
        s.ai.safe.insert("/a".into(), false);
        s.filter = Filter::Protected;
        s.clear_selection();
        assert_eq!(
            s.group_check(Priority::Keep),
            crate::components::checkbox::TriState::Unchecked
        );
        apply(&mut s, Action::ToggleGroup(Priority::Keep));
        assert!(s.is_checked("/a"));
        assert!(!s.is_checked("/k"));
        apply(&mut s, Action::ToggleGroup(Priority::Keep));
        assert_eq!(s.checked_count(), 0);
        apply(
            &mut s,
            Action::ToggleSubgroupSelection(Priority::Keep, vec!["/a".into(), "/k".into()]),
        );
        assert!(s.is_checked("/a"));
        assert!(!s.is_checked("/k"));
        apply(&mut s, Action::ToggleCheck("/k".into()));
        assert!(!s.is_checked("/k"));
    }

    #[test]
    fn selection_is_frozen_while_cleaning() {
        // 执行期间改勾选会让用户以为这一批变了，而实际执行的是按下确认那一刻的清单。
        let mut s = state_with(vec![cand("/a", 1, Verdict::Reclaimable)]);
        occupy_restore(&mut s); // 占住活动，走 is_busy 那条路
        assert!(!s.can_select());

        apply(&mut s, Action::ToggleCheck("/a".into()));
        assert!(s.is_checked("/a"), "忙碌期间勾选不该被改动");

        apply(&mut s, Action::ClearSelection);
        assert!(s.is_checked("/a"), "忙碌期间也不该被清空");
    }

    // ---- 筛选与列表 ----

    #[test]
    fn setting_a_filter_moves_the_scroll_target() {
        // 清单现在常显，切筛选的意义只剩「把视图滚到清单」。
        let mut s = state_with(vec![cand("/a", 1, Verdict::Reclaimable)]);
        s.scroll_to_list = false;
        let fx = apply(&mut s, Action::SetFilter(Filter::Reclaimable));
        assert!(fx.is_empty());
        assert_eq!(s.filter, Filter::Reclaimable);
        assert!(s.scroll_to_list, "切筛选要滚到清单位置");
    }

    #[test]
    fn query_moves_the_scroll_target() {
        let mut s = state_with(vec![cand("/a", 1, Verdict::Reclaimable)]);
        s.scroll_to_list = false;
        apply(&mut s, Action::SetQuery("abc".into()));
        assert_eq!(s.query, "abc");
        assert!(s.scroll_to_list);

        apply(&mut s, Action::ConsumeScrollTo);
        assert!(!s.scroll_to_list, "滚过去之后要消费掉，否则每帧都会跳");
    }

    // ---- 清理批次 ----

    #[test]
    fn prepare_clean_collects_the_checked_rows() {
        let mut s = state_with(vec![
            cand("/a", 100, Verdict::Reclaimable),
            cand("/b", 200, costly()),
        ]);
        // 有代价项默认不勾，这里手动勾上
        apply(&mut s, Action::ToggleCheck("/b".into()));
        apply(
            &mut s,
            Action::PrepareClean(molt_act::CleanMode::Quarantine),
        );

        let p = s.pending_clean.as_ref().expect("应装上待确认批次");
        assert_eq!(p.count(), 2);
        assert_eq!(p.bytes, 300);
    }

    #[test]
    fn prepare_clean_with_nothing_checked_yields_no_batch() {
        // 空批次不是「0 项待确认」，而是没有待确认这回事——否则会弹出一个空确认框。
        let mut s = state_with(vec![cand("/a", 100, Verdict::Reclaimable)]);
        apply(&mut s, Action::ClearSelection);
        apply(&mut s, Action::PrepareClean(molt_act::CleanMode::Delete));
        assert!(s.pending_clean.is_none());
    }

    #[test]
    fn cancelling_clears_the_batch_but_leaves_the_selection() {
        // 取消确认不等于放弃勾选：用户只是改主意了，不该让他重挑一遍。
        let mut s = state_with(vec![cand("/a", 100, Verdict::Reclaimable)]);
        apply(&mut s, Action::PrepareClean(molt_act::CleanMode::Delete));
        assert!(s.pending_clean.is_some());
        apply(&mut s, Action::CancelPendingClean);
        assert!(s.pending_clean.is_none());
        assert!(s.is_checked("/a"));
    }

    // ---- 结果条的下一步 ----

    #[test]
    fn retry_from_report_starts_a_scan_only_when_idle() {
        let mut s = AppState::new("/Users/me".into());
        let fx = apply(&mut s, Action::FollowNextStep(NextStep::Retry));
        assert!(matches!(fx.as_slice(), [Effect::Scan]));

        occupy_restore(&mut s);
        let fx = apply(&mut s, Action::FollowNextStep(NextStep::Retry));
        assert!(fx.is_empty(), "忙碌时不该再起一次扫描");
    }

    // ---- 密钥 ----

    #[test]
    fn key_draft_can_be_cleared() {
        // 密钥只存在内存里，保存后必须能立即清掉。
        let mut s = AppState::new("/Users/me".into());
        apply(&mut s, Action::SetKeyDraft("sk-secret".into()));
        assert_eq!(s.key_draft, "sk-secret");
        apply(&mut s, Action::ClearKeyDraft);
        assert!(s.key_draft.is_empty());
    }

    /// 换服务必须清掉上一家的密钥草稿，否则「保存密钥」会把 A 家的 key
    /// 写进 B 家的配置里。同一家服务下换模型则要留着。
    #[test]
    fn switching_provider_drops_the_previous_key_draft() {
        let mut s = AppState::new("/Users/me".into());
        s.ai.config.set_provider("deepseek");
        apply(&mut s, Action::SetKeyDraft("sk-deepseek".into()));

        // 同一家服务下换模型：密钥仍属于这家，不能清
        apply(&mut s, Action::SetAiModel("deepseek-chat".into()));
        assert_eq!(
            s.key_draft, "sk-deepseek",
            "同服务换模型不该清掉正在填的密钥"
        );

        apply(&mut s, Action::SetAiProvider("openrouter".into()));
        assert!(
            s.key_draft.is_empty(),
            "换到别家服务后，上一家填的密钥不能留在输入框里"
        );
    }

    /// 字号三档：点哪档就立刻改显示档位，越界的编号忽略。
    /// 用户明确要求默认落在最大档，这里一并锁住。
    #[test]
    fn font_scale_switches_all_three_choices() {
        let mut s = AppState::new("/Users/me".into());
        let large = s.display_scale;
        assert!(
            (large - 1.4).abs() < 0.01,
            "默认必须是大字号档，实际 {large}"
        );

        apply(&mut s, Action::SetFontScale(2));
        assert!((s.display_scale - 1.0).abs() < 0.01, "第三档是标准字号");
        apply(&mut s, Action::SetFontScale(1));
        assert!((s.display_scale - 1.2).abs() < 0.01, "第二档是中字号");
        apply(&mut s, Action::SetFontScale(0));
        assert!((s.display_scale - 1.4).abs() < 0.01, "第一档是大字号");

        let before = s.display_scale;
        apply(&mut s, Action::SetFontScale(9));
        assert_eq!(s.display_scale, before, "越界的编号要忽略");
    }

    // ---- 自动化草稿 ----

    #[test]
    fn enabling_auto_edits_the_draft_not_the_live_config() {
        // 界面上的改动只写草稿，点保存才落盘——否则用户拨一下时间就立刻改了
        // launchd 里的任务，而他可能只是路过看看。
        let mut s = AppState::new("/Users/me".into());
        s.auto.config.enabled = false;
        s.auto.draft = None;

        apply(&mut s, Action::SetAutoEnabled(true));

        assert!(!s.auto.config.enabled, "生效配置不能被直接改掉");
        assert_eq!(
            s.auto.draft.as_ref().map(|d| d.enabled),
            Some(true),
            "改动要落在草稿上"
        );
        assert!(s.auto.dirty(), "草稿与生效配置不同时，dirty 才算得对");
    }

    // ---- 设置：写盘动作只在空闲时发出 ----

    #[test]
    fn saving_a_key_needs_a_non_empty_draft_and_idle_state() {
        // 空草稿落盘会把服务密钥覆盖成空串；忙碌时落盘会和正在跑的读取抢文件。
        let mut s = AppState::new("/Users/me".into());
        assert!(
            apply(&mut s, Action::SaveApiKey).is_empty(),
            "空草稿不该产生落盘"
        );

        apply(&mut s, Action::SetKeyDraft("sk-abc".into()));
        assert!(matches!(
            apply(&mut s, Action::SaveApiKey).as_slice(),
            [Effect::SaveApiKey]
        ));

        occupy_restore(&mut s);
        assert!(
            apply(&mut s, Action::SaveApiKey).is_empty(),
            "忙碌时不该落盘"
        );
    }

    #[test]
    fn font_scale_defaults_to_large_and_rejects_unknown_choice() {
        let mut state = AppState::new("/Users/me".into());
        assert_eq!(state.display_scale, crate::theme::Theme::DEFAULT_SCALE);
        apply(&mut state, Action::SetFontScale(2));
        assert_eq!(state.display_scale, 1.0);
        apply(&mut state, Action::SetFontScale(9));
        assert_eq!(state.display_scale, 1.0);
        apply(&mut state, Action::SetFontScale(0));
        assert_eq!(state.display_scale, crate::theme::Theme::DEFAULT_SCALE);
    }

    #[test]
    fn switching_model_writes_back_the_config() {
        // 换服务或换模型都要立刻落盘：配置只存在内存里的话，
        // 下次启动又回到旧模型，用户会以为自己的选择没生效。
        let mut s = AppState::new("/Users/me".into());
        s.ai.config.active_model = "a".into();
        let fx = apply(&mut s, Action::SetAiModel("b".into()));
        assert_eq!(s.ai.config.active_model, "b");
        assert!(matches!(fx.as_slice(), [Effect::SaveAiConfig]));
    }

    // ---- 隔离区的批量还原 ----

    fn qrow(id: i64, restorable: bool) -> crate::state::QuarantineRow {
        crate::state::QuarantineRow {
            id,
            original: format!("/Users/me/Library/x{id}"),
            bytes: 1024,
            at: "2026-10-20T10:00:00Z".into(),
            purge_after: "2026-10-27T10:00:00Z".into(),
            batch: None,
            restorable,
        }
    }

    #[test]
    fn restore_selected_only_passes_restorable_checked_ids() {
        // 已过期的条目仍在库里，但内容早被彻底删掉了。把它们塞进批次
        // 只会在执行时逐条失败，还把真实的失败原因淹掉。
        let mut s = AppState::new("/Users/me".into());
        s.quarantine = vec![qrow(1, true), qrow(2, false), qrow(3, true)];
        s.quarantine_checked = [1, 2, 3].into_iter().collect();

        let fx = apply(&mut s, Action::RestoreSelected);

        match fx.as_slice() {
            [Effect::RestoreQuarantineBatch(ids)] => {
                assert_eq!(ids, &vec![1, 3], "只有可还原的才该进批次");
            }
            other => panic!("期望批量还原，实际 {other:?}"),
        }
    }

    #[test]
    fn restore_selected_with_nothing_checked_asks_for_nothing() {
        // 空勾选不该产生一个空 Effect：执行层不必为一个空批次跑一遍开库。
        let mut s = AppState::new("/Users/me".into());
        s.quarantine = vec![qrow(1, true)];

        let fx = apply(&mut s, Action::RestoreSelected);
        assert!(fx.is_empty());
    }

    #[test]
    fn restore_selected_is_refused_while_busy() {
        // 互斥：清理或还原正在跑的时候，不能并开第二个还原。
        let mut s = AppState::new("/Users/me".into());
        s.quarantine = vec![qrow(1, true)];
        s.quarantine_checked = [1].into_iter().collect();
        occupy_restore(&mut s);

        let fx = apply(&mut s, Action::RestoreSelected);
        assert!(fx.is_empty(), "忙碌时不该产生还原批次");
        assert!(s.status.is_some(), "要告诉用户为什么没动");
    }

    #[test]
    fn purge_batch_keeps_the_confirmed_ids() {
        let row = |id: i64, path: &str| crate::state::QuarantineRow {
            id,
            original: path.to_string(),
            bytes: 10,
            at: "2026-10-20T10:00:00Z".into(),
            purge_after: "2026-10-27T10:00:00Z".into(),
            batch: None,
            restorable: true,
        };
        let mut s = state_with(vec![]);
        s.quarantine = vec![row(1, "/Users/me/a"), row(2, "/Users/me/b")];
        s.quarantine_checked.insert(1);
        apply(&mut s, Action::PreparePurge);
        assert_eq!(
            s.pending_purge.as_ref().unwrap().ids(),
            vec![1],
            "确认框里的批次应只含当时勾上的那一项"
        );

        // 确认框还开着时用户又勾了一条：执行仍只认按下确认那一刻的那批。
        s.quarantine_checked.insert(2);
        let fx = apply(&mut s, Action::RunPendingPurge);
        assert_eq!(fx.len(), 1, "确认只产生一个执行意图");
        match &fx[0] {
            Effect::Purge(p) => assert_eq!(
                p.ids(),
                vec![1],
                "确认 1 项就该只删 1 项——不能把确认之后才勾上的带进去"
            ),
            other => panic!("应是 Effect::Purge，实际 {other:?}"),
        }
    }

    /// 行内「彻底删除…」只该给这一条一个独立批次。
    ///
    /// 以前它把整份勾选替换成这一条：用户勾了 3 项、顺手点了某行的删除，
    /// 确认框里只剩 1 项，之前勾的都没了，取消也换不回来。
    #[test]
    fn purging_one_row_leaves_the_other_checks_alone() {
        let mut s = state_with(vec![]);
        s.quarantine = vec![qrow(1, true), qrow(2, true), qrow(3, true)];
        s.quarantine_checked = [1, 2].into_iter().collect();

        apply(&mut s, Action::PreparePurgeOne(3));

        let pending = s.pending_purge.as_ref().expect("应有一个待确认批次");
        assert_eq!(pending.ids(), vec![3], "确认框里只该有被点的那一行");

        // 取消之后，原来的勾选必须还在。
        apply(&mut s, Action::CancelPendingPurge);
        assert!(s.pending_purge.is_none());
        let mut left: Vec<i64> = s.quarantine_checked.iter().copied().collect();
        left.sort();
        assert_eq!(left, vec![1, 2], "单条删除不该动用户已有的多选");
    }

    #[test]
    fn batch_restore_is_a_background_job() {
        // 还原以前是同步 for 循环：几十 GB 的解压会让窗口卡到没有一帧。
        // 现在任务在后台跑，状态层面立刻表现为「忙」——勾选与按钮因此置灰。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("molt-restore-job-{nanos}"));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let store = molt_act::AuditStore::open(&dir.join("audit.db")).expect("建审计库");
        let q = molt_act::Quarantine::new(dir.join("q"), "restore");

        let mut s = state_with(vec![]);
        s.restore_job = Some(crate::worker::RestoreJob::start(Vec::new(), store, q));
        assert_eq!(
            s.activity(),
            crate::state::Activity::Restoring,
            "任务存在时活动状态必须是「正在还原」"
        );
        assert!(!s.can_select(), "还原期间不能改勾选");
        assert!(!s.can_start(), "还原期间不能开新动作");

        // 空批次的 job 会立刻结束；轮询到 Done，别让线程在测试结束后还跑。
        let mut job = s.restore_job.take().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if matches!(job.poll(), crate::worker::RestoreProgress::Done(_)) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "还原任务没有在 5 秒内结束"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
