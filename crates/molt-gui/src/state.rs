//! 界面状态。所有「界面上能变的东西」都在这里，界面代码只读它、改它。
//!
//! 三件事分开存放，因为它们在用户眼里本来就是三件事：
//! - `result`：磁盘上现在有什么（扫描结果）
//! - `quarantine`：已经从原位置移走、还能拿回来的东西
//! - `history`：发生过什么（只读时间线）
//!
//! 另有一件事贯穿三者：**同一时间只能有一件会碰盘的事在做**。
//! 扫描、AI 分析、清理、彻底删除、定时任务、还原，全部走 `activity()` 这一个判断。
//! 界面上的按钮一律先问它，再决定画不画得动——而不是「点了没反应」。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::worker::{ScanJob, ScanResult};
use molt_ai::AiConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// 工作台：一进来就知道该做什么
    Workbench,
    /// 隔离区：固定入口，查看 / 还原 / 彻底删除
    Quarantine,
    /// 历史：发生过什么，只读
    History,
    /// 自动化：定时清理
    Automation,
    /// 设置：AI 配置（含 jev）与规则信息
    Settings,
}

impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Page::Workbench => "清理",
            Page::Quarantine => "临时空间",
            Page::History => "历史",
            Page::Automation => "自动清理",
            Page::Settings => "设置",
        }
    }

    pub const ALL: [Page; 5] = [
        Page::Workbench,
        Page::Quarantine,
        Page::History,
        Page::Automation,
        Page::Settings,
    ];
}

/// 当前正在做的事。整个界面只有一个活动，这是互斥的唯一来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Scanning,
    /// AI 正在解释这次扫描
    Analyzing,
    /// 正在移入隔离区或直接清理
    Cleaning,
    /// 正在从隔离区彻底删除
    Purging,
    /// 正在跑一次定时清理
    Schedule,
    /// 正在还原
    Restoring,
}

impl Activity {
    pub fn is_busy(self) -> bool {
        !matches!(self, Activity::Idle)
    }

    /// 顶栏那一行「现在在干什么」。空闲时没有可说的。
    pub fn label(self) -> Option<&'static str> {
        match self {
            Activity::Idle => None,
            Activity::Scanning => Some("正在扫描"),
            Activity::Analyzing => Some("正在让 AI 解释"),
            Activity::Cleaning => Some("正在清理"),
            Activity::Purging => Some("正在彻底删除"),
            Activity::Schedule => Some("正在执行定时清理"),
            Activity::Restoring => Some("正在还原"),
        }
    }

    /// 按钮置灰时要给出的理由。用户看到灰按钮必须能知道为什么。
    pub fn blocking_hint(self) -> &'static str {
        match self {
            Activity::Idle => "",
            Activity::Scanning => "扫描进行中，完成后再操作",
            Activity::Analyzing => "AI 正在解释这次扫描，完成后再操作",
            Activity::Cleaning => "清理进行中，完成后再操作",
            Activity::Purging => "彻底删除进行中，完成后再操作",
            Activity::Schedule => "定时清理正在执行，完成后再操作",
            Activity::Restoring => "正在还原，完成后再操作",
        }
    }
}

/// 清单的筛选。默认看全部。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    /// 「现在就能清」：建议清理 + 可回收。首页只露这一档时最常用的一种看法。
    Cleanable,
    /// 建议优先处理：有代价但久未使用，或 AI 判定可以动
    Suggested,
    Reclaimable,
    Costly,
    Protected,
}

/// 一条数据在「该不该动手」这条主线上的位置。
///
/// 用它排序，而不是只按体积：用户打开界面要知道的是「先清哪个」，
/// 不是「哪个最大」。最大的那几项往往是有代价的构建产物，最不该先动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// 建议清理：有代价但很久没用，或 AI 判定可以动
    Suggested = 0,
    /// 可回收：删了自动重建
    Safe = 1,
    /// 有代价：能清，但要重编 / 重下
    Costly = 2,
    /// 建议保留：受保护，或 AI 判定不要动
    Keep = 3,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::Suggested => "建议清掉",
            Priority::Safe => "可以直接清",
            Priority::Costly => "清之前先确认",
            Priority::Keep => "建议留着",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Priority::Suggested => "很久没用了，清掉代价最小",
            Priority::Safe => "删了会自动重建，不影响使用",
            Priority::Costly => "能腾出空间，但下次要重新编译 / 下载",
            Priority::Keep => "你的记录、密钥或正在被使用的内容，不会被移动",
        }
    }

    /// 这一档能不能勾选。
    pub fn selectable(self) -> bool {
        !matches!(self, Priority::Keep)
    }
}

/// 历史页的一行。
#[derive(Debug, Clone)]
pub struct HistoryRow {
    pub id: i64,
    pub path: String,
    pub bytes: u64,
    pub at: String,
    pub source: molt_act::CleanSource,
    pub action: HistoryAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAction {
    Quarantined,
    Restored,
    Purged,
    Deleted,
}

impl HistoryAction {
    pub fn label(self) -> &'static str {
        match self {
            HistoryAction::Quarantined => "放入临时空间",
            HistoryAction::Restored => "已还原",
            HistoryAction::Purged => "彻底删除",
            HistoryAction::Deleted => "直接清理",
        }
    }
}

/// 隔离区里的一条。`restorable = false` 表示内容已经不在隔离区了，
/// 界面必须显式说明，不能画成可还原。
#[derive(Debug, Clone)]
pub struct QuarantineRow {
    pub id: i64,
    pub original: String,
    pub bytes: u64,
    pub at: String,
    pub purge_after: String,
    pub restorable: bool,
}

/// 确认框里要展示的一项。
#[derive(Debug, Clone)]
pub struct PendingItem {
    pub path: PathBuf,
    pub bytes: u64,
    /// AI 对这一条的解释。确认框里要显示——用户按下不可撤销的按钮前，
    /// 有权看到「这一条是什么」而不只是一个路径。
    pub note: Option<String>,
}

/// 一次待确认的清理。
#[derive(Debug, Clone)]
pub struct PendingClean {
    pub mode: molt_act::CleanMode,
    pub items: Vec<PendingItem>,
    pub bytes: u64,
}

impl PendingClean {
    pub fn count(&self) -> usize {
        self.items.len()
    }
}

/// 一次待确认的隔离区彻底删除。
#[derive(Debug, Clone)]
pub struct PendingPurge {
    pub items: Vec<PendingItem>,
    pub bytes: u64,
}

impl PendingPurge {
    pub fn count(&self) -> usize {
        self.items.len()
    }
}

/// AI 层的全部状态。配置与运行分开：
/// 配置错了不该让界面一片空白，运行失败也不该动配置。
pub struct AiPanel {
    pub config: AiConfig,
    pub config_path: PathBuf,
    pub config_error: Option<String>,
    /// 正在跑的解释任务
    pub job: Option<crate::ai_worker::AiJob>,
    /// 最近一次失败
    pub failure: Option<crate::ai_worker::AiFailure>,
    /// 最近一次成功用的模型与发送量，界面上要说清「这份解释是谁给的」
    pub last_model: Option<String>,
    pub last_sent: usize,
    pub last_at: Option<String>,
    /// 扫描时的绝对路径 → 一句话解释
    pub notes: HashMap<String, String>,
    /// 扫描时的绝对路径 → AI 是否认为可以动手
    pub safe: HashMap<String, bool>,
}

impl Default for AiPanel {
    fn default() -> Self {
        Self {
            config: AiConfig::default(),
            config_path: molt_ai::config::default_path(),
            config_error: None,
            job: None,
            failure: None,
            last_model: None,
            last_sent: 0,
            last_at: None,
            notes: HashMap::new(),
            safe: HashMap::new(),
        }
    }
}

impl AiPanel {
    pub fn reload_config(&mut self) {
        match AiConfig::load(&self.config_path) {
            Ok(c) => {
                self.config = c;
                self.config_error = None;
            }
            Err(e) => self.config_error = Some(e.to_string()),
        }
    }

    /// 删除判断用的解析：没选模型时按约定落到 jev。
    pub fn resolve(&self) -> Result<molt_ai::Resolved, String> {
        if let Some(e) = &self.config_error {
            return Err(e.clone());
        }
        self.config
            .resolve_for_delete_judge()
            .map_err(|e| e.to_string())
    }

    pub fn is_running(&self) -> bool {
        self.job.as_ref().map(|j| !j.is_finished()).unwrap_or(false)
    }

    pub fn note(&self, path: &str) -> Option<&str> {
        self.notes.get(path).map(|s| s.as_str())
    }

    pub fn has_notes(&self) -> bool {
        !self.notes.is_empty()
    }
}

/// 一次操作的结果。所有页面共用同一份展示。
///
/// 除了「做成了什么」，还记着「接下来去哪」——见 [`NextStep`]。
/// 结果条如果只写「已移入隔离区 12 项」就断了，用户得自己想「那我去哪看」；
/// 按钮文案与目标页由产出报告的那一方决定，因为它知道这次到底做成了什么。
#[derive(Debug, Clone)]
pub struct OpReport {
    /// 一句话结论，如「已永久删除 12 项，释放 1.2 GB」
    pub summary: String,
    pub done: usize,
    pub done_bytes: u64,
    /// 逐项失败原因：(显示用路径, 原因)
    pub failures: Vec<(String, String)>,
    pub kind: StatusKind,
    /// 接着做的那一步。`None` 表示这次没有明确的后续动作。
    pub next: Option<NextStep>,
}

/// 一次操作之后，用户最可能想接着做的那一件事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    /// 去隔离区看看刚移走的东西
    Quarantine,
    /// 去历史看这次永久删除的记录
    History,
    /// 重试失败的那些（回到清单重新扫描）
    Retry,
}

impl NextStep {
    pub fn label(self) -> &'static str {
        match self {
            NextStep::Quarantine => "去临时空间",
            NextStep::History => "看历史",
            NextStep::Retry => "重新扫描",
        }
    }
}

/// 历史页的筛选：三种「这一页最常被用来查的事」。
///
/// 不按每个 `HistoryAction` 各给一档：「彻底删除」与「直接清理」对用户是
/// 同一件事（永久没了），分成两档只会让两边的数字都变小、看不出总量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryFilter {
    #[default]
    All,
    /// 移入隔离区（还可能拿回来）
    Quarantined,
    /// 永久删除（隔离区彻底删除 + 直接清理）
    Gone,
    /// 还原
    Restored,
}

impl HistoryFilter {
    pub fn keeps(self, action: HistoryAction) -> bool {
        match self {
            HistoryFilter::All => true,
            HistoryFilter::Quarantined => action == HistoryAction::Quarantined,
            HistoryFilter::Gone => {
                matches!(action, HistoryAction::Purged | HistoryAction::Deleted)
            }
            HistoryFilter::Restored => action == HistoryAction::Restored,
        }
    }
}

pub struct AppState {
    pub page: Page,
    pub ai: AiPanel,
    pub job: Option<ScanJob>,
    pub clean_job: Option<crate::worker::CleanJob>,
    pub purge_job: Option<crate::worker::PurgeJob>,
    pub schedule_job: Option<crate::schedule::ScheduleJob>,
    pub result: Option<Arc<ScanResult>>,
    /// 已勾选的路径。只有可动项会进来。
    pub checked: HashSet<String>,
    pub filter: Filter,
    pub query: String,
    pub quarantine: Vec<QuarantineRow>,
    pub quarantine_error: Option<String>,
    pub quarantine_loading: bool,
    pub quarantine_checked: HashSet<i64>,
    pub history: Vec<HistoryRow>,
    pub history_error: Option<String>,
    pub history_filter: HistoryFilter,
    pub status: Option<StatusMessage>,
    pub restoring: Option<i64>,
    pub rules: Option<molt_core::RuleSet>,
    /// 设置页的「高级」是否展开。默认收起：base_url / 环境变量名不是
    /// 每次都要看的东西，但排障时必须能拿到。
    pub settings_advanced: bool,
    /// 设置页里正在输入的密钥。**只存在内存里**，保存后立即清空——
    /// 它绝不能跟着窗口状态被持久化，也不能被回填进输入框。
    pub key_draft: String,
    pub home: String,
    pub error: Option<String>,
    /// 上一次**成功**扫描结束的时刻（`HH:MM`）。重扫失败时用它告诉用户
    /// 「下面这份结果是什么时候的」——否则旧数字会被当成刚扫出来的。
    pub last_scan_at: Option<String>,
    pub disk: Option<Result<molt_scan::disk::DiskUsage, String>>,
    pub session_freed: u64,
    pub last_op: Option<Arc<OpReport>>,
    pub pending_clean: Option<PendingClean>,
    pub pending_purge: Option<PendingPurge>,
    /// 清单是否已展开到全部。首页默认只露前几项，
    /// 否则一屏全被清单吃掉，结论区和主按钮就被挤到看不见。
    pub list_expanded: bool,
    /// 下一帧是否把滚动条移到清单开头（用户点了「看看清单」）
    pub scroll_to_list: bool,
    /// 定时任务页的状态
    pub auto: AutoPanel,
    /// 首页圆环在清理中的动画进度（0..1）。清理结束时不再推进，
    /// 下一次开始清理时清零——它只服务「一眼看到正在清」这一件事。
    pub clean_anim: f32,
    /// 本次清理的总项数与总字节，用于圆环圆心与说明文字。
    pub clean_total_items: usize,
    pub clean_total_bytes: u64,
    /// 刚清完的庆祝态剩余秒数：>0 时圆环满绿并写「清理完成」，
    /// 倒数完毕就回到磁盘读数。它让「清掉了」这件事自己看得见。
    pub clean_done_secs: f32,
    /// 上一帧是否在清理。用来区分「清理被中断」与「动画只是被别人摆好了」：
    /// 前者要把弧收回去，后者（出图工具、预览）不该被抹掉。
    pub clean_was_running: bool,
    /// 刚结束的这一批是「直接清理」还是「放入临时空间」。
    /// 两者在环心不能写同一句话：放进临时空间并没有释放空间，
    /// 写「已腾出」就是在骗人。
    pub clean_was_delete: bool,
}

/// 自动化页的状态。
#[derive(Default)]
pub struct AutoPanel {
    /// 已经落盘、正在生效的那份配置。
    pub config: crate::schedule::ScheduleConfig,
    /// 正在编辑的草稿。为空表示还没载入过。
    ///
    /// 界面上的改动只写进草稿，点「保存」才落盘——否则用户拨一下时间就
    /// 立刻改了 launchd 里的任务，而他可能只是路过看看。
    pub draft: Option<crate::schedule::ScheduleConfig>,
    pub loaded: bool,
    pub error: Option<String>,
    pub last_run: Option<crate::schedule::LastRun>,
    /// launchd 里现在是否真的加载着。`None` 表示还没查。
    pub installed: Option<bool>,
    /// 正在写配置 / 装载 launchd。期间按钮置灰，防止重复点。
    pub saving: bool,
}

impl AutoPanel {
    /// 草稿与生效配置是否不同。派生而不是另存一个 `dirty` 字段：
    /// 两个字段迟早会不一致，而这里算一次是 O(几个字段)。
    pub fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(|d| *d != self.config)
    }

    /// 草稿（没有草稿时退回生效配置），供只读展示使用。
    pub fn editing(&self) -> &crate::schedule::ScheduleConfig {
        self.draft.as_ref().unwrap_or(&self.config)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub kind: StatusKind,
    pub text: String,
}

impl StatusMessage {
    pub fn info(t: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Info,
            text: t.into(),
        }
    }
    pub fn warn(t: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Warn,
            text: t.into(),
        }
    }
    pub fn error(t: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Error,
            text: t.into(),
        }
    }
}

impl AppState {
    pub fn new(home: String) -> Self {
        let mut ai = AiPanel::default();
        ai.reload_config();
        Self {
            page: Page::Workbench,
            ai,
            job: None,
            clean_job: None,
            purge_job: None,
            schedule_job: None,
            result: None,
            checked: HashSet::new(),
            filter: Filter::All,
            query: String::new(),
            quarantine: Vec::new(),
            quarantine_error: None,
            // 标成 true：窗口一打开就会读一次，读到之前不显示「隔离区是空的」
            quarantine_loading: true,
            quarantine_checked: HashSet::new(),
            history: Vec::new(),
            history_error: None,
            history_filter: HistoryFilter::All,
            status: None,
            restoring: None,
            rules: None,
            settings_advanced: false,
            key_draft: String::new(),
            home,
            error: None,
            last_scan_at: None,
            disk: None,
            session_freed: 0,
            last_op: None,
            pending_clean: None,
            pending_purge: None,
            list_expanded: false,
            scroll_to_list: false,
            auto: AutoPanel::default(),
            clean_anim: 0.0,
            clean_total_items: 0,
            clean_total_bytes: 0,
            clean_done_secs: 0.0,
            clean_was_running: false,
            clean_was_delete: false,
        }
    }

    pub fn refresh_disk(&mut self) {
        self.disk = Some(molt_scan::disk::read(&self.home));
    }

    pub fn is_cleaning(&self) -> bool {
        self.clean_job
            .as_ref()
            .map(|j| !j.is_finished())
            .unwrap_or(false)
    }

    pub fn is_purging(&self) -> bool {
        self.purge_job
            .as_ref()
            .map(|j| !j.is_finished())
            .unwrap_or(false)
    }

    pub fn is_scanning(&self) -> bool {
        self.job.as_ref().map(|j| !j.is_finished()).unwrap_or(false)
    }

    pub fn is_scheduled(&self) -> bool {
        self.schedule_job
            .as_ref()
            .map(|j| !j.is_finished())
            .unwrap_or(false)
    }

    /// **唯一**的活动判断。互斥就是从这里来的，别处不许自己再拼一遍。
    pub fn activity(&self) -> Activity {
        if self.is_scanning() {
            Activity::Scanning
        } else if self.ai.is_running() {
            Activity::Analyzing
        } else if self.is_cleaning() {
            Activity::Cleaning
        } else if self.is_purging() {
            Activity::Purging
        } else if self.is_scheduled() {
            Activity::Schedule
        } else if self.restoring.is_some() {
            Activity::Restoring
        } else {
            Activity::Idle
        }
    }

    pub fn is_busy(&self) -> bool {
        self.activity().is_busy()
    }

    /// 能不能改勾选。执行期间改勾选会让用户以为这一批变了，而实际执行的是按下确认那一刻的清单。
    pub fn can_select(&self) -> bool {
        !self.is_busy()
    }

    /// 能不能开始一个新的、会碰盘或占网络的动作。
    pub fn can_start(&self) -> bool {
        !self.is_busy()
    }

    /// 这一条属于哪一档。
    pub fn priority_of(&self, c: &molt_core::Candidate) -> Priority {
        let path = c.path.display().to_string();
        // AI 说不要动，就按「建议保留」排——但**不改变**本地判定，
        // 也就是说它仍然可以手动勾选：AI 只是建议，判定权在规则与用户。
        if self.ai.safe.get(&path) == Some(&false) {
            return Priority::Keep;
        }
        match &c.verdict {
            molt_core::Verdict::Protected { .. } => Priority::Keep,
            molt_core::Verdict::Costly { .. } => {
                if c.suggest_clean || self.ai.safe.get(&path) == Some(&true) {
                    Priority::Suggested
                } else {
                    Priority::Costly
                }
            }
            molt_core::Verdict::Reclaimable => {
                if c.suggest_clean {
                    Priority::Suggested
                } else {
                    Priority::Safe
                }
            }
        }
    }

    pub fn ai_note(&self, path: &str) -> Option<&str> {
        self.ai.note(path)
    }

    /// 只选中「可回收」那批，并返回 (项数, 字节)。
    pub fn select_reclaimable(&mut self) -> (usize, u64) {
        let Some(r) = &self.result else {
            self.checked.clear();
            return (0, 0);
        };
        self.checked = r
            .items
            .iter()
            .filter(|c| self.priority_of(c) == Priority::Safe)
            .map(|c| c.path.display().to_string())
            .collect();
        self.pending_clean = None;
        (self.checked_count(), self.checked_bytes())
    }

    /// 选中「建议清理」那一档。
    pub fn select_suggested(&mut self) -> (usize, u64) {
        let Some(r) = &self.result else {
            self.checked.clear();
            return (0, 0);
        };
        self.checked = r
            .items
            .iter()
            .filter(|c| self.priority_of(c) == Priority::Suggested)
            .map(|c| c.path.display().to_string())
            .collect();
        self.pending_clean = None;
        (self.checked_count(), self.checked_bytes())
    }

    /// 把当前筛选下能动手的项全部选中（受保护与「建议保留」不进来）。
    pub fn select_all_visible(&mut self) -> usize {
        let visible = self.visible_items();
        let Some(r) = &self.result else {
            return 0;
        };
        for i in visible {
            let c = &r.items[i];
            if self.priority_of(c).selectable() {
                self.checked.insert(c.path.display().to_string());
            }
        }
        self.pending_clean = None;
        self.checked_count()
    }

    pub fn clear_selection(&mut self) {
        self.checked.clear();
        self.pending_clean = None;
    }

    /// 「可回收」那一档的数量与体积。**不含**「建议清理」——
    /// 顶部把两档相加来回答「一共能清多少」，这里再把它算进来就会重复计数，
    /// 用户会看到「可以先清 3 项」而列表里只有 2 项是勾着的。
    pub fn reclaimable_summary(&self) -> (usize, u64) {
        let Some(r) = &self.result else {
            return (0, 0);
        };
        let mut n = 0;
        let mut bytes = 0;
        for c in &r.items {
            if self.priority_of(c) == Priority::Safe {
                n += 1;
                bytes += c.alloc_bytes.max(c.bytes);
            }
        }
        (n, bytes)
    }

    /// 建议优先处理的那一批：数量与体积。
    pub fn suggested_summary(&self) -> (usize, u64) {
        let Some(r) = &self.result else {
            return (0, 0);
        };
        let mut n = 0;
        let mut bytes = 0;
        for c in &r.items {
            if self.priority_of(c) == Priority::Suggested {
                n += 1;
                bytes += c.alloc_bytes.max(c.bytes);
            }
        }
        (n, bytes)
    }

    /// 扫描完成后默认勾选「建议清理 + 可回收」。
    ///
    /// 这两档合起来才是「打开就能动」的那批：前者是久未使用、后者是自动重建。
    /// 有代价项一律不默认勾选——那是用户要自己决定的区域。
    pub fn apply_result(&mut self, result: Arc<ScanResult>) {
        self.result = Some(result);
        let keep: Vec<String> = self
            .result
            .as_ref()
            .map(|r| {
                r.items
                    .iter()
                    .filter(|c| matches!(self.priority_of(c), Priority::Safe | Priority::Suggested))
                    .map(|c| c.path.display().to_string())
                    .collect()
            })
            .unwrap_or_default();
        self.checked = keep.into_iter().collect();
        self.pending_clean = None;
    }

    /// 扫完了、有内容，但一项都不能动。
    pub fn only_protected(&self) -> bool {
        match &self.result {
            Some(r) if !r.items.is_empty() => {
                !r.items.iter().any(|c| self.priority_of(c).selectable())
            }
            _ => false,
        }
    }

    pub fn toggle(&mut self, path: &str) {
        if !self.can_select() {
            return;
        }
        let key = path.to_string();
        if !self.checked.remove(&key) {
            self.checked.insert(key);
        }
        self.pending_clean = None;
    }

    pub fn is_checked(&self, path: &str) -> bool {
        self.checked.contains(path)
    }

    pub fn checked_bytes(&self) -> u64 {
        let Some(r) = &self.result else { return 0 };
        r.items
            .iter()
            .filter(|c| self.is_checked(&c.path.display().to_string()))
            .map(|c| c.alloc_bytes.max(c.bytes))
            .sum()
    }

    pub fn checked_count(&self) -> usize {
        let Some(r) = &self.result else { return 0 };
        r.items
            .iter()
            .filter(|c| self.is_checked(&c.path.display().to_string()))
            .count()
    }

    /// 已勾选、但被当前筛选或搜索挡在列表外的项数。
    ///
    /// 筛选与搜索只决定「看到哪些行」，从不改动 `checked`。底栏如果只报
    /// 可见的勾选数，用户会以为只清了眼前这几行，实际却动了更多东西。
    pub fn checked_outside_filter(&self) -> usize {
        let Some(r) = &self.result else { return 0 };
        let visible: std::collections::HashSet<usize> = self.visible_items().into_iter().collect();
        r.items
            .iter()
            .enumerate()
            .filter(|(i, c)| !visible.contains(i) && self.is_checked(&c.path.display().to_string()))
            .count()
    }

    /// 把当前勾选变成一份待确认批次。AI 解释一并带上：
    /// 确认框是用户最后一次看到「这一条是什么」的地方。
    pub fn pending_from_checked(&self, mode: molt_act::CleanMode) -> Option<PendingClean> {
        let r = self.result.as_ref()?;
        let items: Vec<PendingItem> = r
            .items
            .iter()
            .filter(|c| self.is_checked(&c.path.display().to_string()))
            .map(|c| PendingItem {
                path: c.path.clone(),
                bytes: c.alloc_bytes.max(c.bytes),
                note: self
                    .ai_note(&c.path.display().to_string())
                    .map(str::to_string),
            })
            .collect();
        if items.is_empty() {
            return None;
        }
        let bytes = items.iter().map(|i| i.bytes).sum();
        Some(PendingClean { mode, items, bytes })
    }

    pub fn toggle_quarantine(&mut self, id: i64) {
        if !self.can_select() {
            return;
        }
        if !self.quarantine_checked.remove(&id) {
            self.quarantine_checked.insert(id);
        }
        self.pending_purge = None;
    }

    pub fn is_quarantine_checked(&self, id: i64) -> bool {
        self.quarantine_checked.contains(&id)
    }

    pub fn quarantine_checked_count(&self) -> usize {
        self.quarantine
            .iter()
            .filter(|r| r.restorable && self.quarantine_checked.contains(&r.id))
            .count()
    }

    pub fn quarantine_checked_bytes(&self) -> u64 {
        self.quarantine
            .iter()
            .filter(|r| r.restorable && self.quarantine_checked.contains(&r.id))
            .map(|r| r.bytes)
            .sum()
    }

    pub fn select_all_quarantine(&mut self) {
        if !self.can_select() {
            return;
        }
        self.quarantine_checked = self
            .quarantine
            .iter()
            .filter(|r| r.restorable)
            .map(|r| r.id)
            .collect();
        self.pending_purge = None;
    }

    pub fn clear_quarantine_selection(&mut self) {
        self.quarantine_checked.clear();
        self.pending_purge = None;
    }

    pub fn pending_purge_from_checked(&self) -> Option<PendingPurge> {
        let items: Vec<PendingItem> = self
            .quarantine
            .iter()
            .filter(|r| r.restorable && self.quarantine_checked.contains(&r.id))
            .map(|r| PendingItem {
                path: PathBuf::from(&r.original),
                bytes: r.bytes,
                note: None,
            })
            .collect();
        if items.is_empty() {
            return None;
        }
        let bytes = items.iter().map(|i| i.bytes).sum();
        Some(PendingPurge { items, bytes })
    }

    /// 按筛选 + 搜索 + 优先级排序后，应该显示哪些行。
    ///
    /// 排序规则是这一页的主线：建议清理在最前，然后是可回收、有代价，
    /// 最后是受保护。同一档里按体积降序——用户先看到最占地方的。
    pub fn visible_items(&self) -> Vec<usize> {
        let Some(r) = &self.result else {
            return Vec::new();
        };
        let q = self.query.trim().to_lowercase();
        let mut idx: Vec<usize> = (0..r.items.len())
            .filter(|&i| {
                let c = &r.items[i];
                let pr = self.priority_of(c);
                match self.filter {
                    // 「全部」不含保留项：那些东西不会被移动，
                    // 混在清单里只会让可清理的部分被稀释。想看它们走「保留」这一档。
                    Filter::All => pr != Priority::Keep,
                    Filter::Cleanable => {
                        matches!(pr, Priority::Suggested | Priority::Safe)
                    }
                    Filter::Suggested => pr == Priority::Suggested,
                    Filter::Reclaimable => pr == Priority::Safe,
                    Filter::Costly => pr == Priority::Costly,
                    Filter::Protected => pr == Priority::Keep,
                }
            })
            .filter(|&i| {
                if q.is_empty() {
                    return true;
                }
                r.items[i]
                    .path
                    .display()
                    .to_string()
                    .to_lowercase()
                    .contains(&q)
            })
            .collect();

        idx.sort_by_key(|&i| {
            (
                self.priority_of(&r.items[i]),
                std::cmp::Reverse(r.items[i].alloc_bytes.max(r.items[i].bytes)),
            )
        });
        idx
    }

    /// 各档的数量，给筛选标签用。
    pub fn counts(&self) -> [usize; 5] {
        let Some(r) = &self.result else {
            return [0; 5];
        };
        let mut c = [0usize; 5];
        for item in &r.items {
            match self.priority_of(item) {
                Priority::Suggested => c[1] += 1,
                Priority::Safe => c[2] += 1,
                Priority::Costly => c[3] += 1,
                Priority::Keep => c[4] += 1,
            }
            c[0] += 1;
        }
        c
    }

    /// 某一档的项数与体积。
    ///
    /// 口径与列表分节完全一致（都走 `priority_of`）：三档互不重叠，
    /// 相加就等于扫到的全部。首页每个数字都能和下面的清单对上——
    /// 旧版把「扫到的总量」和「能清的量」并排放在最显眼处，
    /// 用户会以为能腾出前者。
    pub fn summary_of(&self, pr: Priority) -> (usize, u64) {
        let Some(r) = &self.result else {
            return (0, 0);
        };
        let mut n = 0;
        let mut bytes = 0;
        for c in &r.items {
            if self.priority_of(c) == pr {
                n += 1;
                bytes += c.alloc_bytes.max(c.bytes);
            }
        }
        (n, bytes)
    }

    /// 「现在就能清」的那一批：建议清理 + 可回收。首页的大数字只讲这一个量。
    pub fn cleanable_summary(&self) -> (usize, u64) {
        let (n1, b1) = self.suggested_summary();
        let (n2, b2) = self.reclaimable_summary();
        (n1 + n2, b1 + b2)
    }

    /// 某一档里的项数。分节标题要用它，0 项的分节不画。
    pub fn count_of(&self, p: Priority) -> usize {
        let Some(r) = &self.result else { return 0 };
        r.items.iter().filter(|c| self.priority_of(c) == p).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{Candidate, CostKind, ItemKind, ProtectReason, RebuildCost, Verdict};
    use std::path::Path;

    fn cand(path: &str, bytes: u64, v: Verdict) -> Candidate {
        Candidate::new(path, ItemKind::Dir, v).with_size(bytes, bytes, 1)
    }

    fn costly() -> Verdict {
        Verdict::Costly {
            rebuild: RebuildCost::new(CostKind::Recompile, "重编"),
        }
    }

    fn state_with(items: Vec<Candidate>) -> AppState {
        let mut s = AppState::new("/Users/me".into());
        let result = ScanResult {
            items,
            rule_ids: Vec::new(),
            denied_roots: vec![],
            elapsed_secs: 0.0,
        };
        s.apply_result(Arc::new(result));
        s
    }

    #[test]
    fn only_actionable_items_are_checked_by_default() {
        let s = state_with(vec![
            cand("/a", 100, Verdict::Reclaimable),
            cand("/b", 200, costly()),
            cand(
                "/c",
                300,
                Verdict::Protected {
                    reason: ProtectReason::UserRecord,
                },
            ),
        ]);
        assert!(s.is_checked("/a"), "可回收项默认勾选");
        assert!(!s.is_checked("/b"), "有代价项绝不默认勾选");
        assert!(!s.is_checked("/c"));
        assert_eq!(s.checked_bytes(), 100);
    }

    #[test]
    fn suggested_items_come_before_safe_ones_in_the_list() {
        let mut stale = cand("/p/target", 900, costly());
        stale.suggest_clean = true;
        let s = state_with(vec![stale, cand("/tiny", 1, Verdict::Reclaimable)]);
        let vis = s.visible_items();
        let r = s.result.as_ref().unwrap();
        assert_eq!(r.items[vis[0]].path.to_str(), Some("/p/target"));
    }

    #[test]
    fn protected_items_are_hidden_by_default() {
        // 「建议保留」单独一档，默认列表里不混进来——否则用户会对着
        // 一条永远点不动的路径反复尝试
        let s = state_with(vec![
            cand("/a", 1, Verdict::Reclaimable),
            cand(
                "/k",
                99,
                Verdict::Protected {
                    reason: ProtectReason::UserRecord,
                },
            ),
        ]);
        let vis = s.visible_items();
        assert_eq!(vis.len(), 1);
        assert_eq!(
            s.result.as_ref().unwrap().items[vis[0]].path.to_str(),
            Some("/a")
        );
    }

    #[test]
    fn ai_saying_keep_moves_a_row_out_of_the_default_list() {
        let mut s = state_with(vec![cand("/a", 10, Verdict::Reclaimable)]);
        s.ai.safe.insert("/a".to_string(), false);
        assert!(
            s.visible_items().is_empty(),
            "AI 说不要动，默认列表里就不该出现"
        );
        // 但仍然可以手动勾选：建议不等于判定
        assert_eq!(
            s.priority_of(&s.result.as_ref().unwrap().items[0]),
            Priority::Keep
        );
    }

    #[test]
    fn filter_hides_other_priorities() {
        let mut s = state_with(vec![
            cand("/a", 100, Verdict::Reclaimable),
            cand("/b", 200, costly()),
        ]);
        s.filter = Filter::Costly;
        let vis = s.visible_items();
        assert_eq!(vis.len(), 1);
        assert_eq!(
            s.result.as_ref().unwrap().items[vis[0]].path.to_str(),
            Some("/b")
        );
    }

    #[test]
    fn query_matches_path_case_insensitively() {
        let mut s = state_with(vec![
            cand("/Users/me/Library/Cursor/Cache", 10, Verdict::Reclaimable),
            cand("/Users/me/.npm", 20, Verdict::Reclaimable),
        ]);
        s.query = "CURSOR".into();
        assert_eq!(s.visible_items().len(), 1);
    }

    #[test]
    fn select_reclaimable_skips_costly_items() {
        let mut s = state_with(vec![
            cand("/a", 100, Verdict::Reclaimable),
            cand("/b", 200, Verdict::Reclaimable),
            cand("/c", 999, costly()),
        ]);
        let (n, bytes) = s.select_reclaimable();
        assert_eq!(n, 2);
        assert_eq!(bytes, 300);
    }

    #[test]
    fn select_suggested_only_takes_the_suggested_rows() {
        let mut stale = cand("/p/target", 900, costly());
        stale.suggest_clean = true;
        let mut s = state_with(vec![stale, cand("/a", 100, Verdict::Reclaimable)]);
        s.clear_selection();
        let (n, bytes) = s.select_suggested();
        assert_eq!(n, 1);
        assert_eq!(bytes, 900);
        assert!(!s.is_checked("/a"));
    }

    #[test]
    fn pending_batch_carries_the_objects_and_the_ai_note() {
        let mut s = state_with(vec![cand("/a", 100, Verdict::Reclaimable)]);
        s.ai.notes.insert("/a".into(), "这是 X 工具的缓存".into());
        let p = s
            .pending_from_checked(molt_act::CleanMode::Delete)
            .expect("应有待确认批次");
        assert_eq!(p.count(), 1);
        assert_eq!(p.items[0].note.as_deref(), Some("这是 X 工具的缓存"));
        assert!(p.items.iter().any(|i| i.path == Path::new("/a")));
    }

    #[test]
    fn selection_cannot_change_while_cleaning() {
        // 执行期间改勾选会让用户以为这一批变了，而实际执行的是按下确认那一刻的清单
        let mut s = state_with(vec![cand("/a", 1, Verdict::Reclaimable)]);
        assert!(s.can_select());
        // 用一个注定不会被 poll 的 job 占住活动
        s.clean_job = Some(crate::worker::CleanJob::start(
            vec![],
            molt_act::AuditStore::in_memory().unwrap(),
            molt_act::Quarantine::new(std::env::temp_dir().join("molt-test-q"), "b"),
            molt_act::CleanMode::Quarantine,
            7,
        ));
        s.toggle("/a");
        assert!(s.is_checked("/a"), "清理期间不应该能改变勾选");
    }

    #[test]
    fn activity_is_exclusive() {
        let mut s = AppState::new("/Users/me".into());
        assert_eq!(s.activity(), Activity::Idle);
        assert!(s.can_start());
        s.restoring = Some(1);
        assert_eq!(s.activity(), Activity::Restoring);
        assert!(!s.can_start(), "有动作在跑时不能再开新动作");
    }

    #[test]
    fn counts_add_up_across_priorities() {
        let mut stale = cand("/p/target", 900, costly());
        stale.suggest_clean = true;
        let s = state_with(vec![
            stale,
            cand("/a", 100, Verdict::Reclaimable),
            cand("/c", 50, costly()),
            cand(
                "/k",
                10,
                Verdict::Protected {
                    reason: ProtectReason::UserRecord,
                },
            ),
        ]);
        let c = s.counts();
        assert_eq!(c, [4, 1, 1, 1, 1]);
        assert_eq!(s.count_of(Priority::Suggested), 1);
    }
}
