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
use std::path::PathBuf;

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

    // ---- 卸载 ----
    /// 重新枚举已装应用与残留
    LoadApps,
    /// 选中某个应用：网格里单击它，右侧浮窗列出它的全部文件。
    /// 不卸载、不启动，只是「看它有哪些文件」。
    OpenApp(String),
    /// 关掉浮窗（浮窗右上角那个叉，或换了页面）
    CloseApp,
    /// 真的启动这个应用（双击图标）——与 `OpenApp` 的「选中」完全是两件事，
    /// 所以那条的语义在注释里写明。
    LaunchApp(String),
    /// 在访达里定位到这个应用（选中它，而不是打开它）
    RevealApp(String),
    /// 打开 / 关闭首页条目的详情抽屉（只读的简介面板）
    OpenDetail(String),
    CloseDetail,
    /// 右键菜单里的「卸载」：把这个应用的本体与残留一起装进待确认批次。
    /// 残留还没扫过时先扫，扫完才弹确认框——否则「卸干净」只卸掉一个 `.app`。
    PrepareUninstall(String),
    CancelUninstall,
    /// 确认框按了确定
    RunUninstall,

    // ---- 工具 ----
    /// 在访达里打开临时空间
    RevealQuarantine,

    // ---- 新建文件夹 / 文件 ----
    OpenCreate,
    CloseCreate,
    SetCreateRoot(String),
    SetCreateLines(String),
    SetCreateTab(crate::state::CreateTab),
    /// 切页签时把预设填进去（只填空的，不覆盖用户已经写的）
    ApplyCreatePreset(crate::state::CreateTab),
    RunCreate,
    /// 撤销最近一次创建
    UndoCreate,
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
    /// 枚举已装应用与残留（读 /Applications 与 ~/Library，纯只读但要遍历目录）
    ScanApps,
    /// 盘点一个应用的残留（只在点开它时才启动）
    ScanLeftovers(String),
    /// 把一个应用（本体 + 残留）移入临时空间
    Uninstall(Box<crate::state::PendingClean>),
    /// 启动一个已装应用（`open`，不动任何数据）
    LaunchApp(String),
    /// 在访达里选中一个已装应用（`open -R`）
    RevealApp(String),
    /// 打开临时空间的目录（`open` 是只读的，不动任何内容）
    RevealQuarantine,
    /// 落盘一批新建
    Create(Box<molt_act::CreatePlan>),
    UndoCreate(String),
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
        Action::ToggleCheck(path) => {
            if state.can_select() {
                // `remove` 的返回值就是「原本在不在」：原来在就已经删掉了，
                // 原来不在就补上。不能先删再查，那样每次都会补上。
                if !state.checked.remove(&path) {
                    state.checked.insert(path);
                }
                state.pending_clean = None;
            }
        }
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
            state.quarantine_checked.clear();
            state.quarantine_checked.insert(id);
            state.pending_purge = state.pending_purge_from_checked();
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
                state.ai.config.set_provider(&id);
                return vec![Effect::SaveAiConfig];
            }
        }
        Action::SetAiModel(id) => {
            if !state.is_busy() {
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

        Action::ReloadRules => return vec![],
        Action::Analyze => {
            if state.can_start() {
                return vec![Effect::Analyze];
            }
        }

        // ---- 卸载 ----
        Action::LoadApps => {
            // 扫目录要遍历几百个 .app 与 ~/Library，放在 UI 线程里会卡住窗口。
            state.uninstall.loading = true;
            return vec![Effect::ScanApps];
        }
        Action::OpenApp(path) => {
            // 单击 = 选中。浮窗里要能看见「这个应用占了哪些文件、各占多少」，
            // 而那份清单要残留扫过才有。所以在选中时就按需扫一次，不等点卸载。
            let same = state.uninstall.open.as_deref() == Some(path.as_str());
            state.uninstall.open = if same { None } else { Some(path.clone()) };
            state.uninstall.note = None;
            if same {
                // 关掉浮窗：正在为它扫的残留没用了，停掉
                state.leftover_job = None;
                state.uninstall.leftovers_loading = None;
                state.uninstall.pending_uninstall_after_scan = None;
                return Vec::new();
            }
            let loaded = state
                .uninstall
                .current()
                .map(|a| a.leftovers_loaded)
                .unwrap_or(false);
            if loaded {
                state.uninstall.leftovers_loading = None;
                return Vec::new();
            }
            state.uninstall.leftovers_loading = Some(path.clone());
            // 残留盘点在个别应用上能慢到十分钟，所以它是后台跑的：
            // 纯函数只表态「需要扫」，怎么扫由 `app.rs` 决定。
            if state.leftover_job.is_none() {
                return vec![Effect::ScanLeftovers(path)];
            }
        }
        Action::CloseApp => {
            state.uninstall.open = None;
            state.uninstall.note = None;
            state.uninstall.leftovers_loading = None;
            state.uninstall.pending_uninstall_after_scan = None;
            // 这一轮扫描的结果已经没人要了，别让它回来时改写一个没人看的行
            state.leftover_job = None;
        }
        Action::PrepareUninstall(path) => {
            // 右键菜单里的卸载。卸载永远覆盖本体 + 全部残留（默认全勾的语义），
            // 用户没地方逐项取消——所以残留必须先知道齐不齐。
            let loaded = state
                .uninstall
                .apps
                .iter()
                .find(|a| a.path == path)
                .map(|a| a.leftovers_loaded)
                .unwrap_or(false);
            if !loaded {
                // 还没扫过残留：先去扫，扫完再回来装批次。
                // 否则用户以为卸干净了，而实际只卸掉了一个 `.app`。
                state.uninstall.pending_uninstall_after_scan = Some(path.clone());
                if state.leftover_job.is_none() {
                    state.uninstall.leftovers_loading = Some(path.clone());
                    return vec![Effect::ScanLeftovers(path)];
                }
                // 别的应用的残留正在扫：等它回来，这里不再起第二个任务
                return Vec::new();
            }
            state.pending_clean = pending_uninstall(&state.uninstall, &path);
        }
        Action::CancelUninstall => state.pending_clean = None,
        Action::RunUninstall => {
            // 卸载**不能**走 `start_clean`：它会拿首页扫描结果里的勾选再过滤一遍批次，
            // 而应用本体与残留根本不在扫描清单里——那样得到的永远是空批次，
            // 盘上一个字节都不动，界面却像点过了。这里直接把批次交给卸载执行路径。
            let Some(pending) = state.pending_clean.take() else {
                return Vec::new();
            };
            if !state.can_start() {
                state.status = Some(crate::state::StatusMessage::warn(
                    state.activity().blocking_hint().to_string(),
                ));
                return Vec::new();
            }
            state.uninstall.removing = pending.items.first().map(|i| i.path.display().to_string());
            return vec![Effect::Uninstall(Box::new(pending))];
        }

        // ---- 工具 ----
        Action::RevealQuarantine => return vec![Effect::RevealQuarantine],
        Action::LaunchApp(path) => return vec![Effect::LaunchApp(path)],
        Action::RevealApp(path) => return vec![Effect::RevealApp(path)],

        // ---- 详情抽屉 ----
        Action::OpenDetail(path) => state.detail_path = Some(path),
        Action::CloseDetail => state.detail_path = None,

        // ---- 新建 ----
        Action::OpenCreate => {
            if state.create.root.is_empty() {
                // 默认落在家目录：新建一个空文件夹不会毁掉什么，
                // 而让用户从零手打一条绝对路径是最容易出错的起点。
                state.create.root = home_with_slash(&state.home);
            }
            state.create.open = true;
            state.create.error = None;
        }
        Action::CloseCreate => {
            state.create.open = false;
            state.create.error = None;
        }
        Action::SetCreateRoot(s) => state.create.root = s,
        Action::SetCreateLines(s) => state.create.lines = s,
        Action::SetCreateTab(tab) => {
            state.create.tab = tab;
            // 切页签时若内容还是空的，就把该页签的预设填上；
            // 已有内容一律不动——用户写的东西不能被切换页签抹掉。
            if state.create.lines.trim().is_empty() {
                state.create.lines = create_preset(tab);
            }
        }
        Action::ApplyCreatePreset(tab) => {
            state.create.tab = tab;
            state.create.lines = create_preset(tab);
        }
        Action::RunCreate => {
            if state.is_busy() {
                state.status = Some(crate::state::StatusMessage::warn(
                    state.activity().blocking_hint().to_string(),
                ));
            } else {
                let plan = state.create.plan(&state.home);
                if plan.will_create() == 0 {
                    state.create.error = Some(if plan.items.is_empty() {
                        "还没有要创建的东西：每行写一条路径，行尾加 / 表示文件夹".to_string()
                    } else {
                        "这一批里没有可创建的项：都已存在，或都被拒绝了".to_string()
                    });
                } else {
                    state.create.error = None;
                    return vec![Effect::Create(Box::new(plan))];
                }
            }
        }
        Action::UndoCreate => {
            if let Some(batch) = state.create.last.as_ref().map(|r| r.batch.clone()) {
                if state
                    .create
                    .last
                    .as_ref()
                    .is_some_and(|r| !r.created.is_empty())
                {
                    return vec![Effect::UndoCreate(batch)];
                }
            }
        }
    }
    Vec::new()
}

/// 家目录 + 结尾斜杠，给输入框一个能直接接相对路径的起点。
fn home_with_slash(home: &str) -> String {
    if home.ends_with('/') {
        home.to_string()
    } else {
        format!("{home}/")
    }
}

/// 三个页签各自的起点内容。
///
/// 预设只写「最少够用」的那几项：一次塞二十个目录，用户要先删掉一半才敢建。
fn create_preset(tab: crate::state::CreateTab) -> String {
    use crate::state::CreateTab;
    match tab {
        CreateTab::Skeleton => "src/\ntests/\nREADME.md\n".to_string(),
        CreateTab::Dirs => "".to_string(),
        CreateTab::Files => "".to_string(),
    }
}

/// 把卸载页勾掉的项装成一次待确认批次。
///
/// 判定统一写成「有代价」而不是「可回收」：卸载掉的东西不会自动重建。
/// 这个档位只影响展示口径，不影响隔离与还原。
fn pending_uninstall(
    panel: &crate::state::UninstallPanel,
    path: &str,
) -> Option<crate::state::PendingClean> {
    let app = panel.apps.iter().find(|a| a.path == path)?;
    // 本体 + 全部残留。不再看勾选：右键菜单只有「卸载」一项，
    // 它承诺的就是「卸干净」。想留配置的用户没有按钮可点，所以这里必须全带。
    let mut items: Vec<crate::state::PendingItem> = Vec::with_capacity(app.leftovers.len() + 1);
    items.push(crate::state::PendingItem {
        path: PathBuf::from(&app.path),
        bytes: app.bytes,
        note: Some(format!("{} 的应用本体", app.name)),
    });
    for l in &app.leftovers {
        items.push(crate::state::PendingItem {
            path: PathBuf::from(&l.path),
            bytes: l.bytes,
            note: Some(l.category.to_string()),
        });
    }
    let bytes = items.iter().map(|i| i.bytes).sum();
    Some(crate::state::PendingClean {
        mode: molt_act::CleanMode::Quarantine,
        items,
        bytes,
    })
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
        Page::Uninstall => {
            // 这一页只在**第一次**进入时列一遍应用。
            //
            // 以前每次切回来都重新扫：遍历 /Applications、再逐个 bundle 递归量体积，
            // 用户只是从别的页签点回来却要等几秒，看到的是「正在列出应用…」。
            // 清单不会自己变——真变了（装了/卸了应用）由「重新扫描」负责。
            if state.uninstall.listed_at.is_some() || !state.uninstall.apps.is_empty() {
                Vec::new()
            } else {
                state.uninstall.loading = true;
                vec![Effect::ScanApps]
            }
        }
        Page::History => vec![Effect::ReloadHistory],
        Page::Automation => {
            state.auto.loaded = false;
            state.auto.draft = None;
            vec![Effect::EnsureAutoLoaded]
        }
        Page::Settings => vec![Effect::ReloadAiConfig],
        // 工具页只是一组入口，进页不读任何盘：每个工具按下去时各自发起动作。
        Page::Tools => Vec::new(),
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

/// 受保护项与「建议保留」永远不进勾选。这是整套界面最硬的一条约束，
/// 所以给它一个具名函数，测试直接对着它断言。
pub fn is_selectable(state: &AppState, path: &str) -> bool {
    let Some(r) = &state.result else {
        return false;
    };
    r.items
        .iter()
        .find(|c| c.path.display().to_string() == path)
        .map(|c| state.priority_of(c).selectable())
        .unwrap_or(false)
}

/// 给测试用：某一档的项数，口径与列表分节一致。
pub fn count_of(state: &AppState, p: Priority) -> usize {
    state.count_of(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CreateTab;
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

    #[test]
    fn returning_to_uninstall_reuses_the_list_instead_of_rescanning() {
        // 卸载页每次切回来都重新遍历 /Applications、重测每个 bundle 的体积，
        // 用户只是从别的页签点回来却要等几秒。清单列过一次就该复用。
        let mut s = AppState::new("/Users/me".into());

        // 第一次进入：该扫
        let fx = apply(&mut s, Action::GoPage(Page::Uninstall));
        assert!(
            matches!(fx.as_slice(), [Effect::ScanApps]),
            "第一次进入要列出应用，实际 {fx:?}"
        );
        assert!(s.uninstall.loading, "扫描期间要显示「正在列出应用…」");

        // 扫描结束（清单 + 时间戳都落下了）
        s.uninstall.loading = false;
        s.uninstall.apps.push(crate::state::AppRow {
            name: "Demo".into(),
            path: "/Applications/Demo.app".into(),
            bundle_id: None,
            bytes: 1,
            bytes_known: true,
            leftovers: Vec::new(),
            leftover_bytes: 0,
            icon_file: None,
            leftovers_loaded: false,
        });
        s.uninstall.listed_at = Some(std::time::SystemTime::now());

        // 离开再回来：不该再扫一次
        apply(&mut s, Action::GoPage(Page::Workbench));
        let fx = apply(&mut s, Action::GoPage(Page::Uninstall));
        assert!(fx.is_empty(), "切回卸载页不该重扫，实际 {fx:?}");
        assert!(!s.uninstall.loading, "复用清单时不该进 loading 态");
        assert_eq!(s.uninstall.apps.len(), 1, "已有清单要留着");
    }

    #[test]
    fn explicit_rescan_still_works_after_the_list_is_cached() {
        // 缓存不能把「重新扫描」也一起挡掉——装了新应用就是这个入口。
        let mut s = AppState::new("/Users/me".into());
        s.uninstall.listed_at = Some(std::time::SystemTime::now());
        let fx = apply(&mut s, Action::LoadApps);
        assert!(matches!(fx.as_slice(), [Effect::ScanApps]), "实际 {fx:?}");
        assert!(s.uninstall.loading);
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
    fn selection_is_frozen_while_cleaning() {
        // 执行期间改勾选会让用户以为这一批变了，而实际执行的是按下确认那一刻的清单。
        let mut s = state_with(vec![cand("/a", 1, Verdict::Reclaimable)]);
        s.restoring = Some(1); // 占住活动，走 is_busy 那条路
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

        s.restoring = Some(1);
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

        s.restoring = Some(1);
        assert!(
            apply(&mut s, Action::SaveApiKey).is_empty(),
            "忙碌时不该落盘"
        );
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
        s.restoring = Some(9);

        let fx = apply(&mut s, Action::RestoreSelected);
        assert!(fx.is_empty(), "忙碌时不该产生还原批次");
        assert!(s.status.is_some(), "要告诉用户为什么没动");
    }

    // ---- 卸载 ----

    fn uninstall_state() -> AppState {
        let mut s = AppState::new("/Users/me".into());
        s.uninstall.apps = vec![crate::state::AppRow {
            name: "Demo".into(),
            path: "/Applications/Demo.app".into(),
            bundle_id: Some("com.example.demo".into()),
            bytes: 1_000,
            bytes_known: true,
            leftovers: vec![crate::state::LeftoverRow {
                path: "/Users/me/Library/Caches/com.example.demo".into(),
                display: "~/Library/Caches/com.example.demo".into(),
                category: "缓存",
                bytes: 200,
                needs_admin: false,
                unmeasured: false,
            }],
            leftover_bytes: 200,
            icon_file: None,
            // 夹具的残留是现成的，等同于「已经扫过」
            leftovers_loaded: true,
        }];
        s
    }

    #[test]
    fn opening_an_app_only_selects_it() {
        // 单击只负责选中：浮窗要看的是「有哪些文件」，不是预先勾好一个批次。
        let mut s = uninstall_state();
        apply(&mut s, Action::OpenApp("/Applications/Demo.app".into()));
        assert_eq!(s.uninstall.open.as_deref(), Some("/Applications/Demo.app"));
        assert!(s.pending_clean.is_none(), "选中不该顺手造出待确认批次");
        assert_eq!(s.uninstall.file_count(), 2);
        assert_eq!(s.uninstall.total_bytes(), 1_200);
    }

    #[test]
    fn closing_the_window_drops_the_selection_and_the_scan() {
        let mut s = uninstall_state();
        apply(&mut s, Action::OpenApp("/Applications/Demo.app".into()));
        apply(&mut s, Action::CloseApp);
        assert!(s.uninstall.open.is_none());
        assert!(s.uninstall.leftovers_loading.is_none());
    }

    #[test]
    fn uninstall_right_click_carries_body_and_every_leftover() {
        // 右键「卸载」承诺的是「卸干净」：本体 + 全部残留一起进批次，
        // 没有逐项勾选这一步，所以这里断言的是「一条都不落」。
        let mut s = uninstall_state();
        apply(
            &mut s,
            Action::PrepareUninstall("/Applications/Demo.app".into()),
        );
        let p = s.pending_clean.as_ref().expect("应当有确认批次");
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].path.to_string_lossy(), "/Applications/Demo.app");
        assert_eq!(
            p.items[1].path.to_string_lossy(),
            "/Users/me/Library/Caches/com.example.demo"
        );
        assert_eq!(p.bytes, 1_200);
        assert_eq!(p.mode, molt_act::CleanMode::Quarantine, "卸载必须可还原");
    }

    #[test]
    fn uninstall_waits_for_leftovers_before_building_the_batch() {
        // 残留还没扫过就装批次，会只卸掉一个 .app 而用户以为卸干净了。
        // 这里应当先派一次扫描，并记下「扫完再卸」。
        let mut s = uninstall_state();
        s.uninstall.apps[0].leftovers_loaded = false;
        s.uninstall.apps[0].leftovers.clear();
        s.uninstall.apps[0].leftover_bytes = 0;

        let fx = apply(
            &mut s,
            Action::PrepareUninstall("/Applications/Demo.app".into()),
        );
        assert!(
            matches!(fx.as_slice(), [Effect::ScanLeftovers(p)] if p == "/Applications/Demo.app"),
            "应当先扫残留，实际：{fx:?}"
        );
        assert!(s.pending_clean.is_none(), "扫完之前不该弹确认框");
        assert_eq!(
            s.uninstall.pending_uninstall_after_scan.as_deref(),
            Some("/Applications/Demo.app")
        );
    }

    #[test]
    fn uninstall_with_no_such_app_asks_for_nothing() {
        let mut s = uninstall_state();
        apply(
            &mut s,
            Action::PrepareUninstall("/Applications/Nope.app".into()),
        );
        assert!(s.pending_clean.is_none());
    }

    #[test]
    fn running_the_uninstall_hands_the_batch_to_its_own_effect() {
        // 卸载不能走 `Effect::Clean`：那条路会拿首页扫描结果的勾选再过滤一遍，
        // 而应用本体与残留都不在那份清单里，批次会被清空成「没有选中任何项」。
        let mut s = uninstall_state();
        apply(
            &mut s,
            Action::PrepareUninstall("/Applications/Demo.app".into()),
        );
        let fx = apply(&mut s, Action::RunUninstall);
        assert!(
            matches!(fx.as_slice(), [Effect::Uninstall(p)] if p.items.len() == 2),
            "应当派出卸载专用效果，实际：{fx:?}"
        );
        assert!(s.pending_clean.is_none(), "确认过就不再留着批次");
        assert_eq!(
            s.uninstall.removing.as_deref(),
            Some("/Applications/Demo.app"),
            "在途标记要记本体路径，完成检测靠它"
        );
    }

    // ---- 新建 ----

    #[test]
    fn opening_create_fills_a_starting_directory() {
        // 默认落在家目录：让用户从零手打绝对路径是最容易出错的起点。
        let mut s = AppState::new("/Users/me".into());
        apply(&mut s, Action::OpenCreate);
        assert!(s.create.open);
        assert_eq!(s.create.root, "/Users/me/");
    }

    #[test]
    fn switching_tabs_does_not_clobber_what_the_user_wrote() {
        let mut s = AppState::new("/Users/me".into());
        apply(&mut s, Action::SetCreateLines("mine/\n".into()));
        apply(&mut s, Action::SetCreateTab(CreateTab::Files));
        assert_eq!(s.create.lines, "mine/\n", "已有内容不能被切换页签抹掉");
        assert_eq!(s.create.tab, CreateTab::Files);
    }

    #[test]
    fn switching_tabs_fills_the_preset_when_the_box_is_empty() {
        let mut s = AppState::new("/Users/me".into());
        apply(&mut s, Action::SetCreateTab(CreateTab::Skeleton));
        assert!(s.create.lines.contains("src/"), "空输入时要填上骨架预设");
    }

    #[test]
    fn running_create_with_nothing_to_do_reports_instead_of_executing() {
        // 空输入不该起一次写盘：执行层跑一遍空批次只会开一次库。
        let mut s = AppState::new("/Users/me".into());
        apply(&mut s, Action::OpenCreate);
        let fx = apply(&mut s, Action::RunCreate);
        assert!(fx.is_empty());
        assert!(s.create.error.is_some(), "要说明为什么没建");
    }

    #[test]
    fn running_create_with_a_plan_emits_the_effect() {
        let mut s = AppState::new("/Users/me".into());
        apply(&mut s, Action::OpenCreate);
        apply(&mut s, Action::SetCreateLines("a/\n".into()));
        let fx = apply(&mut s, Action::RunCreate);
        assert!(
            matches!(fx.as_slice(), [Effect::Create(_)]),
            "有可建项时要发出写盘意图，实际 {fx:?}"
        );
    }

    #[test]
    fn undo_with_nothing_created_does_nothing() {
        let mut s = AppState::new("/Users/me".into());
        let fx = apply(&mut s, Action::UndoCreate);
        assert!(fx.is_empty());
    }
    #[test]
    fn purge_batch_keeps_the_confirmed_ids() {
        let row = |id: i64, path: &str| crate::state::QuarantineRow {
            id,
            original: path.to_string(),
            bytes: 10,
            at: "2026-10-20T10:00:00Z".into(),
            purge_after: "2026-10-27T10:00:00Z".into(),
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
