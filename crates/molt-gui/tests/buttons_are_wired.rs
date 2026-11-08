//! 按钮级验收：把每一页真的画出来，逐颗按钮发真实点击，断言每颗都产生了动作。
//!
//! 这一份测试存在的理由：单测能证明「`Action::X` 落到 `Effect::Y`」，但它证不了
//! 「界面上那颗按钮真的会 push `Action::X`」。两者的接缝正是「点了没反应」的地方。
//!
//! 点击夹具在 `tests/common/mod.rs`，与 `clean_page_buttons.rs` 共用同一份——
//! 两处各写一套的话，同一个按钮会在一边显示「点得动」、另一边显示「点不动」。
//!
//! 不碰磁盘、不设 `MOLT_HOME`：这里全部是纯界面层，`AppState` 里的数据由夹具填。

mod common;

use molt_gui::action::Action;
use molt_gui::state::{AppState, HistoryAction, HistoryFilter, HistoryRow, Page, QuarantineRow};

/// 默认画 1240×2600：整页一次性画完，按钮不会因为超出可视区而画不出来。
fn harness() -> common::Ui {
    common::Ui::new(1240.0, 2600.0)
}

// ---------- 夹具 ----------

fn qrow(id: i64, restorable: bool) -> QuarantineRow {
    QuarantineRow {
        id,
        original: format!("~/Library/Caches/x{id}"),
        bytes: 1_048_576,
        at: "2026-10-20T10:00:00Z".into(),
        purge_after: "2026-10-27T10:00:00Z".into(),
        batch: Some("2026-10-20-clean".into()),
        restorable,
    }
}

fn hrow(id: i64, action: HistoryAction) -> HistoryRow {
    HistoryRow {
        id,
        path: format!("~/Library/Caches/h{id}"),
        bytes: 2_048_576,
        at: "2026-10-22T21:14:00Z".into(),
        source: molt_act::CleanSource::Manual,
        action,
    }
}

fn quarantine_state(restorable: bool) -> AppState {
    let mut s = AppState::new("/Users/me".into());
    s.page = Page::Quarantine;
    s.quarantine = vec![qrow(1, restorable), qrow(2, restorable)];
    s
}

fn history_state() -> AppState {
    let mut s = AppState::new("/Users/me".into());
    s.page = Page::History;
    s.history = vec![
        hrow(1, HistoryAction::Quarantined),
        hrow(2, HistoryAction::Purged),
        hrow(3, HistoryAction::Restored),
    ];
    s
}

// ---------- 临时空间页 ----------

/// 页头「刷新」必须重新读隔离区。
#[test]
fn quarantine_refresh_button_reloads_the_list() {
    let h = harness();
    let s = quarantine_state(true);
    let (acts, labels) = h.click(&s, molt_gui::pages::quarantine::show, "刷新");
    assert!(
        acts.iter().any(|a| matches!(a, Action::LoadQuarantine)),
        "点「刷新」应重新读隔离区，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 页脚「全选」在有可还原条目时必须能选中全部。
#[test]
fn quarantine_select_all_button_selects_everything() {
    let h = harness();
    let s = quarantine_state(true);
    let (acts, labels) = h.click(&s, molt_gui::pages::quarantine::show, "全选");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::SelectAllQuarantine)),
        "点「全选」应请求全选，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 已经勾了两条时，「全不选」必须能清掉。
#[test]
fn quarantine_clear_selection_button_clears_checks() {
    let h = harness();
    let mut s = quarantine_state(true);
    s.quarantine_checked = [1, 2].into_iter().collect();
    let (acts, labels) = h.click(&s, molt_gui::pages::quarantine::show, "全不选");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::ClearQuarantineSelection)),
        "点「全不选」应清空勾选，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 勾上之后「彻底删除…」必须进入确认流程，而且**不能**当场就删。
///
/// 这是整个应用里最危险的一颗按钮：它背后是不可恢复的删除。
/// 断言的是「只产生一个待确认的意图」，而不是直接开始删。
#[test]
fn quarantine_purge_button_asks_for_confirmation_first() {
    let h = harness();
    let mut s = quarantine_state(true);
    s.quarantine_checked = [1].into_iter().collect();
    let (acts, labels) = h.click(&s, molt_gui::pages::quarantine::show, "彻底删除…");
    assert!(
        acts.iter().any(|a| matches!(a, Action::PreparePurge)),
        "点「彻底删除…」应先请求确认，实际动作：{acts:?}；画面文字：{labels:?}"
    );
    assert!(
        !acts.iter().any(|a| matches!(a, Action::RunPendingPurge)),
        "确认框都还没出来，绝不能直接执行删除：{acts:?}"
    );
}

/// 勾上之后「还原所选」必须走批量还原。
#[test]
fn quarantine_restore_selected_button_restores() {
    let h = harness();
    let mut s = quarantine_state(true);
    s.quarantine_checked = [1].into_iter().collect();
    let (acts, labels) = h.click(&s, molt_gui::pages::quarantine::show, "还原所选");
    assert!(
        acts.iter().any(|a| matches!(a, Action::RestoreSelected)),
        "点「还原所选」应请求还原，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 全部条目都已失效时，「全选」点下去不该有任何反应。
///
/// 这一档里每一行都写着「内容已不在」，全选它们唯一的作用是让用户
/// 一路点到「还原所选」然后全部失败。按钮留着占位可以，但必须是灰的。
#[test]
fn quarantine_select_all_is_a_dead_button_when_nothing_is_restorable() {
    let h = harness();
    let s = quarantine_state(false);
    let (clicked, labels) = h.click(&s, molt_gui::pages::quarantine::show, "全选");
    assert!(
        !clicked
            .iter()
            .any(|a| matches!(a, Action::SelectAllQuarantine)),
        "没有可还原条目时「全选」必须点不动，实际动作：{clicked:?}；画面文字：{labels:?}"
    );
}

// ---------- 历史页 ----------

/// 页头「刷新」重新读历史。
#[test]
fn history_refresh_button_reloads() {
    let h = harness();
    let s = history_state();
    let (acts, labels) = h.click(&s, molt_gui::pages::history::show, "刷新");
    assert!(
        acts.iter().any(|a| matches!(a, Action::LoadHistory)),
        "点「刷新」应重新读历史，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 「去临时空间」必须真的切页，不能只是一句话。
#[test]
fn history_go_to_quarantine_button_navigates() {
    let h = harness();
    let s = history_state();
    let (acts, labels) = h.click(&s, molt_gui::pages::history::show, "去临时空间");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::GoPage(Page::Quarantine))),
        "点「去临时空间」应切到临时空间页，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 筛选分段上点「已还原 N」要切到那一档。
#[test]
fn history_filter_segment_switches_the_list() {
    let h = harness();
    let s = history_state();
    let (acts, labels) = h.click(&s, molt_gui::pages::history::show, "已还原 1");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::SetHistoryFilter(HistoryFilter::Restored))),
        "点「已还原」分段应切到该档，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 读失败时页面给出「重试」，而且底下**不会**再画出一片「空」。
#[test]
fn history_error_shows_retry_and_not_an_empty_state() {
    let h = harness();
    let mut s = history_state();
    s.history.clear();
    s.history_error = Some("打不开审计库：权限不足".into());
    let mut acts = Vec::new();
    let labels = h.paint(&s, molt_gui::pages::history::show, &mut acts);
    let joined: String = labels
        .iter()
        .map(|(t, _)| t.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("读不到历史记录"),
        "读失败要说「读不到」，实际画面：{labels:?}"
    );
    assert!(
        !joined.contains("还没有操作记录"),
        "读失败绝不能被画成「空」——这两件事对用户含义相反：{labels:?}"
    );
    let (clicked, _) = h.click(&s, molt_gui::pages::history::show, "重试");
    assert!(
        clicked.iter().any(|a| matches!(a, Action::LoadHistory)),
        "点「重试」应重新读一次"
    );
}

// ---------- 自动清理页 ----------

fn automation_state() -> AppState {
    let mut s = AppState::new("/Users/me".into());
    s.page = Page::Automation;
    s.auto.loaded = true;
    s
}

/// 定时页的「立即执行一次」必须真的触发一次清理。
#[test]
fn automation_run_now_button_triggers_a_run() {
    let h = harness();
    let s = automation_state();
    let (acts, labels) = h.click(&s, molt_gui::pages::automation::show, "立即执行一次");
    assert!(
        acts.iter().any(|a| matches!(a, Action::RunScheduledNow)),
        "点「立即执行一次」应触发一次执行，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 「刷新状态」要重新查询 launchd 的真实状态。
#[test]
fn automation_refresh_status_button_requeries_launchd() {
    let h = harness();
    let s = automation_state();
    let (acts, labels) = h.click(&s, molt_gui::pages::automation::show, "刷新状态");
    assert!(
        acts.iter().any(|a| matches!(a, Action::RefreshAutoStatus)),
        "点「刷新状态」应重新查询，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

// ---------- 设置页 ----------

/// 「重新读取配置」要重新读一遍，而不是只在界面上装作读了。
///
/// 这里必须**两份都读**：按钮文案说的是「配置」，而这一页的配置有 `ai.toml`
/// 与规则库两份。只重读前者时，用户加完规则点它，规则卡上的统计一动不动。
#[test]
fn settings_reload_button_rereads_both_config_and_rules() {
    let h = harness();
    let s = AppState::new("/Users/me".into());
    let (acts, labels) = h.click(&s, molt_gui::pages::settings::show, "重新读取配置");
    assert!(
        acts.iter().any(|a| matches!(a, Action::ReloadAiConfig)),
        "点「重新读取配置」应重读 AI 配置，实际动作：{acts:?}；画面文字：{labels:?}"
    );
    assert!(
        acts.iter().any(|a| matches!(a, Action::ReloadRules)),
        "同一个按钮也要重读规则库，否则规则卡的统计不会变：{acts:?}"
    );
}

/// 当前服务不需要密钥时，整块密钥卡片不该画出来。
///
/// 本地 ollama / jev 本来就不要密钥。给它们画一个「请输入 API 密钥」的框，
/// 用户会以为不填就不完整——那块界面不该存在。
#[test]
fn settings_hides_the_key_card_for_a_provider_without_keys() {
    let h = harness();
    let mut s = AppState::new("/Users/me".into());
    s.ai.config.set_provider("jev");
    s.key_draft.clear();
    let mut acts = Vec::new();
    let labels = h.paint(&s, molt_gui::pages::settings::show, &mut acts);
    let joined: String = labels
        .iter()
        .map(|(t, _)| t.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !joined.contains("请输入 API 密钥"),
        "不需要密钥的服务不该出现密钥输入框：{labels:?}"
    );
}

/// 换成需要密钥的服务之后：空着点不动「保存」，填了才能保存。
///
/// 空密钥被写进配置文件，会让「已配置」变成一句谎话：下次分析才报错，
/// 而那时用户早忘了自己点过什么。
#[test]
fn settings_save_key_needs_a_non_empty_draft() {
    let h = harness();
    let mut s = AppState::new("/Users/me".into());
    s.ai.config.set_provider("deepseek");

    s.key_draft.clear();
    let (acts, labels) = h.click(&s, molt_gui::pages::settings::show, "保存");
    assert!(
        !acts.iter().any(|a| matches!(a, Action::SaveApiKey)),
        "空密钥不该能保存，实际动作：{acts:?}；画面文字：{labels:?}"
    );

    s.key_draft = "sk-draft".into();
    let (acts, labels) = h.click(&s, molt_gui::pages::settings::show, "保存");
    assert!(
        acts.iter().any(|a| matches!(a, Action::SaveApiKey)),
        "填了密钥之后应能保存，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

// ---------- 清理页：扫描控制 ----------

fn scanning_state(paused: bool) -> AppState {
    let mut s = AppState::new("/Users/me".into());
    s.page = Page::Workbench;
    // 真的起一个扫描任务，但把 home 指到一个空目录：遍历会很快走完，
    // 这里只借它的 control 句柄来构造「运行中 / 已暂停」这两种界面状态。
    let rules = molt_rules::load("/Users/me", None).unwrap();
    let job = molt_gui::worker::ScanJob::start(rules, "/Users/me".into(), false);
    if paused {
        job.control.pause();
    }
    s.job = Some(job);
    s
}

/// 扫描进行中，「暂停」必须能按下去。
#[test]
fn scan_pause_button_works_while_running() {
    let h = harness();
    let s = scanning_state(false);
    let (acts, labels) = h.click(&s, molt_gui::pages::workbench::show, "暂停");
    assert!(
        acts.iter().any(|a| matches!(a, Action::PauseScan)),
        "点「暂停」应请求暂停，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 扫描**没有暂停**的时候，「取消」也必须在场并且能用。
///
/// 以前取消只在暂停后才画出来，于是「扫到一半不想扫了」要分两步：
/// 先暂停、再取消。用户在跑着的扫描上找这颗按钮是找不到的。
#[test]
fn scan_cancel_is_reachable_without_pausing_first() {
    let h = harness();
    let s = scanning_state(false);
    let (acts, labels) = h.click(&s, molt_gui::pages::workbench::show, "取消");
    assert!(
        acts.iter().any(|a| matches!(a, Action::CancelScan)),
        "运行中的扫描上也必须有可用的「取消」，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 暂停之后，「继续」与「取消」两颗都得在。
#[test]
fn scan_paused_state_offers_resume_and_cancel() {
    let h = harness();
    let s = scanning_state(true);
    let (acts, labels) = h.click(&s, molt_gui::pages::workbench::show, "继续");
    assert!(
        acts.iter().any(|a| matches!(a, Action::ResumeScan)),
        "暂停后「继续」应能按，实际动作：{acts:?}；画面文字：{labels:?}"
    );
    let (acts, labels) = h.click(&s, molt_gui::pages::workbench::show, "取消");
    assert!(
        acts.iter().any(|a| matches!(a, Action::CancelScan)),
        "暂停后「取消」应能按，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 扫描结束之后，那一整条控制栏不能再画出来。
///
/// 留一颗点了没用的「暂停」比不画更糟：用户会以为扫描还没停。
#[test]
fn scan_control_bar_disappears_once_the_job_is_cancelled() {
    let h = harness();
    let s = {
        let s = scanning_state(false);
        s.job.as_ref().unwrap().control.cancel();
        s
    };
    let mut acts = Vec::new();
    let labels = h.paint(&s, molt_gui::pages::workbench::show, &mut acts);
    let joined: String = labels
        .iter()
        .map(|(t, _)| t.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !joined.contains("扫描已暂停") && !joined.contains("正在扫描…"),
        "已取消的扫描不该再画控制条：{labels:?}"
    );
}
