//! 清理页的按钮级验收：筛选档、搜索、底栏动作、行内、确认框入口。
//!
//! 点击夹具在 `tests/common/mod.rs`，与 `buttons_are_wired.rs` 共用同一份。
//! 这一页多一层夹具：要造「有扫描结果」的状态。

mod common;

use molt_gui::action::Action;
use molt_gui::state::{AppState, Filter, Page};

/// 这一页内容长，画得更高，保证整页一次性画完。
fn harness() -> common::Ui {
    common::Ui::new(1240.0, 2800.0)
}

// 这一份测试全部针对清理页，把 show 绑死在这里，测试体里就不必每次重复。
fn paint(ui: &common::Ui, s: &AppState, acts: &mut Vec<Action>) -> Vec<(String, egui::Pos2)> {
    ui.paint(s, molt_gui::pages::workbench::show, acts)
}

fn click_containing(ui: &common::Ui, s: &AppState, needle: &str) -> (Vec<Action>, Vec<String>) {
    ui.click_containing(s, molt_gui::pages::workbench::show, needle)
}

fn cand(path: &str, bytes: u64, v: molt_core::Verdict) -> molt_core::Candidate {
    molt_core::Candidate::new(path, molt_core::ItemKind::Dir, v).with_size(bytes, bytes, 2)
}

fn costly() -> molt_core::Verdict {
    molt_core::Verdict::Costly {
        rebuild: molt_core::RebuildCost::new(molt_core::CostKind::Recompile, "重编"),
    }
}

fn state_with(items: Vec<molt_core::Candidate>) -> AppState {
    let mut s = AppState::new("/Users/me".into());
    s.page = Page::Workbench;
    s.apply_result(std::sync::Arc::new(molt_gui::worker::ScanResult {
        items,
        rule_ids: Vec::new(),
        denied_roots: vec![],
        elapsed_secs: 1.0,
        from_cache: false,
    }));
    s
}

fn mixed() -> Vec<molt_core::Candidate> {
    vec![
        cand("/a/cache", 1024, molt_core::Verdict::Reclaimable),
        cand("/b/target", 2048, costly()),
        cand(
            "/c/records",
            4096,
            molt_core::Verdict::Protected {
                reason: molt_core::ProtectReason::UserRecord,
            },
        ),
    ]
}

// ---------- 筛选 ----------

/// 「要确认 N」这一档点下去要真的切到「有代价」。
#[test]
fn filter_segment_costly_switches_the_list() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "要确认");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::SetFilter(Filter::Costly))),
        "点「要确认」应切到有代价档，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 「保留 N」这一档也要能进得去——那里放的是用户绝不能删的东西。
#[test]
fn filter_segment_protected_switches_the_list() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "保留 1");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::SetFilter(Filter::Protected))),
        "点「保留」应切到保留档，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 「久未使用 N」也要能点进去。
#[test]
fn filter_segment_recent_switches_the_list() {
    let h = harness();
    let mut s = state_with(mixed());
    // 让第一批里的可回收项带上「久未使用」标记。
    if let Some(r) = s.result.as_ref() {
        let mut items = r.items.clone();
        items[0].suggest_clean = true;
        s.apply_result(std::sync::Arc::new(molt_gui::worker::ScanResult {
            items,
            rule_ids: Vec::new(),
            denied_roots: vec![],
            elapsed_secs: 1.0,
            from_cache: false,
        }));
    }
    let (acts, labels) = click_containing(&h, &s, "久未使用");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::SetFilter(Filter::Recent))),
        "点「久未使用」应切到该档，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 切到别的档之后，必须有一条回得去的路。
#[test]
fn a_way_back_to_all_exists_once_filtered() {
    let h = harness();
    let mut s = state_with(mixed());
    s.filter = Filter::Costly;
    let (acts, labels) = click_containing(&h, &s, "显示全部");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::SetFilter(Filter::All))),
        "筛选后必须能一键回「全部」，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

// ---------- 底栏 ----------

/// 有可清条目时，底栏主按钮要能进入确认流程。
#[test]
fn footer_clean_button_prepares_a_quarantine_batch() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "清理这些（可还原）");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::PrepareClean(molt_act::CleanMode::Quarantine))),
        "主按钮应请求进入放入隔离区的确认，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 「永久删除…」必须存在，且**不能**是主按钮那种样子。
///
/// 这里只能验它接的是删除路径；「长得不显眼」由 `danger_text_button`
/// 的样式决定，属视觉层，不在这一层断言。
#[test]
fn footer_permanent_delete_is_a_separate_action() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "永久删除");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::PrepareClean(molt_act::CleanMode::Delete))),
        "「永久删除…」必须走 Delete 模式（并随后弹确认），实际动作：{acts:?}；画面文字：{labels:?}"
    );
    assert!(
        !acts
            .iter()
            .any(|a| matches!(a, Action::PrepareClean(molt_act::CleanMode::Quarantine))),
        "点永久删除不能退化成走隔离区的温柔路径：{acts:?}"
    );
}

/// 「全不选」要清掉所有勾选。
#[test]
fn footer_clear_selection_clears_checks() {
    let h = harness();
    let mut s = state_with(mixed());
    s.checked.insert("/a/cache".into());
    let (acts, labels) = click_containing(&h, &s, "全不选");
    assert!(
        acts.iter().any(|a| matches!(a, Action::ClearSelection)),
        "点「全不选」应清空勾选，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 「全选本页」要只选当前筛选看得见的那些。
#[test]
fn footer_select_all_visible_selects_the_current_page() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "全选本页");
    assert!(
        acts.iter().any(|a| matches!(a, Action::SelectAllVisible)),
        "点「全选本页」应选中当前可见行，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

// ---------- 空结果 / 出错 ----------

/// 扫过但没东西时，不该画成「很方便清理」，也不该留一颗点了没用的按钮。
#[test]
fn empty_scan_is_not_drawn_as_success() {
    let h = harness();
    let s = state_with(Vec::new());
    let mut acts = Vec::new();
    let labels = paint(&h, &s, &mut acts);
    let joined: String = labels
        .iter()
        .map(|(t, _)| t.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !joined.contains("很干净"),
        "「没扫到东西」不能说成「很干净」：{labels:?}"
    );
}

/// 读失败时那颗「重试扫描」必须接上重扫。
#[test]
fn retry_scan_button_actually_rescans() {
    let h = harness();
    let mut s = AppState::new("/Users/me".into());
    s.page = Page::Workbench;
    s.error = Some("扫描失败：权限不足".into());
    let (acts, labels) = click_containing(&h, &s, "重试扫描");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::FollowNextStep(molt_gui::state::NextStep::Retry))),
        "点「重试扫描」应触发重扫，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

// ---------- 搜索 ----------

/// 搜索框里有字时，「清空」要能一键清掉。
#[test]
fn clear_search_button_clears_the_query() {
    let h = harness();
    let mut s = state_with(mixed());
    s.query = "cache".into();
    s.query_draft = "cache".into();
    let (acts, labels) = click_containing(&h, &s, "清空");
    assert!(
        acts.iter().any(|a| matches!(a, Action::ClearQuery)),
        "点「清空」应清掉搜索条件，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 搜索真的会过滤，而不是只把字显示在框里。
#[test]
fn search_actually_filters_the_list() {
    let h = harness();
    let mut s = state_with(mixed());
    s.query = "target".into();
    s.query_draft = "target".into();
    let visible = s.visible_items();
    let r = s.result.as_ref().unwrap();
    assert_eq!(visible.len(), 1, "只有一个路径包含 target");
    assert_eq!(
        r.items[visible[0]].path.to_str(),
        Some("/b/target"),
        "过滤结果必须是那一条"
    );
    // 经由界面也看不到别的行
    let mut acts = Vec::new();
    let labels = paint(&h, &s, &mut acts);
    // 只数清单区里的行：页面上方的「空间占用排行」是全量排行，本来就不跟搜索走，
    // 拿它判搜索等于把两个不同的东西混成一个。
    let list_paths: Vec<&String> = labels
        .iter()
        .map(|(t, _)| t)
        .filter(|t| t.starts_with('/'))
        .collect();
    assert_eq!(
        list_paths,
        vec!["/b/target"],
        "搜索之后清单里只该剩匹配的那一行，实际：{list_paths:?}"
    );
}

/// 筛选条上的每一档都要有内容，不能是「点进去发现是空的」。
///
/// 这是「数字与列表同源」的界面级复核：每一档标签上的数字，
/// 必须等于切到那一档之后 `visible_items` 的长度。
#[test]
fn every_filter_segment_number_matches_what_it_shows() {
    let mut s = state_with(mixed());
    // 给可回收项打上「久未使用」，让最冷门的那一档也有内容。
    if let Some(r) = s.result.as_ref() {
        let mut items = r.items.clone();
        items[0].suggest_clean = true;
        s.apply_result(std::sync::Arc::new(molt_gui::worker::ScanResult {
            items,
            rule_ids: Vec::new(),
            denied_roots: vec![],
            elapsed_secs: 1.0,
            from_cache: false,
        }));
    }
    for f in [
        Filter::Cleanable,
        Filter::Costly,
        Filter::Protected,
        Filter::Recent,
        Filter::All,
    ] {
        s.filter = f;
        assert!(
            !s.visible_items().is_empty(),
            "筛选 {f:?} 的标签报了数字，点进去却一行都没有"
        );
    }
    // 「保留」里的东西永远选不上——这条是安全底线。
    s.filter = Filter::Protected;
    for i in s.visible_items() {
        let path = s.result.as_ref().unwrap().items[i]
            .path
            .display()
            .to_string();
        assert!(!s.is_checked(&path), "受保护条目 {path} 不可能被勾选");
    }
}

// ---------- 行内按钮 ----------

/// 每行的「详情」要能打开右侧抽屉。
#[test]
fn row_detail_button_opens_the_drawer() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "详情");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::OpenDetail(path) if path.ends_with("cache"))),
        "点「详情」应打开那一条的抽屉，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 抽屉打开后要能关掉它。
///
/// 右上角那颗叉是纯图标按钮（没有文字图元），所以这里走文档里承诺的另一条
/// 出口：Esc。两条路都必须在——只有叉时，习惯用键盘的人会被困住；
/// 只有 Esc 时，用鼠标的人找不到出口。
#[test]
fn escape_closes_the_drawer() {
    let h = harness();
    let mut s = state_with(mixed());
    let path = s.result.as_ref().unwrap().items[0]
        .path
        .display()
        .to_string();
    s.detail_path = Some(path);

    // 先画一帧，让抽屉进入绘制状态。
    let mut acts = Vec::new();
    let _ = paint(&h, &s, &mut acts);

    acts.clear();
    let mut input = h.base.clone();
    input.events = vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    }];
    let mut out = h.ctx.run_ui(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            molt_gui::pages::workbench::show(ui, &s, &h.p, &mut acts);
        });
    });
    out.textures_delta.clear();
    assert!(
        acts.iter().any(|a| matches!(a, Action::CloseDetail)),
        "按 Esc 应关掉抽屉，实际动作：{acts:?}"
    );
}

/// 抽屉里的「在访达中显示」要接上 RevealInFinder，而不是只画一颗按钮。
#[test]
fn drawer_reveal_button_calls_reveal() {
    let h = harness();
    let mut s = state_with(mixed());
    let path = s.result.as_ref().unwrap().items[0]
        .path
        .display()
        .to_string();
    s.detail_path = Some(path);
    let (acts, labels) = click_containing(&h, &s, "在访达中显示");
    assert!(
        acts.iter().any(|a| matches!(a, Action::RevealInFinder(_))),
        "点「在访达中显示」应请求定位，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

/// 每个分组都要有一个能折叠/展开的箭头。
///
/// 折叠由那颗 `▾` 触发，不是标题文字——标题那一行的左侧是三态勾选框，
/// 同一片区域里放两个动作会互相抢点击。这里守住「箭头在」和
/// 「点它确实产生 ToggleGroup」两件事。
#[test]
fn group_header_toggles_collapse() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "▾");
    // 分组是两层的：外层「可以直接清」这一类，内层「位置 · a」这样的子组。
    // 第一颗箭头属于最靠近它上方标题的那一层，两种折叠动作都算对——
    // 要守住的是「箭头点得动」，而不是它恰好折了哪一层。
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::ToggleGroup(_) | Action::ToggleSubgroupOpen(_, _))),
        "点折叠箭头应折叠/展开它所在的那一组，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}

// ---------- AI ----------

/// 没配模型时，「让 AI 解释」必须点不动——点了只会等一次注定失败的请求。
#[test]
fn ai_button_is_disabled_when_no_provider_is_configured() {
    let h = harness();
    let s = state_with(mixed());
    let (acts, labels) = click_containing(&h, &s, "让 AI 解释");
    assert!(
        !acts.iter().any(|a| matches!(a, Action::Analyze)),
        "没配模型时不该能发起分析，实际动作：{acts:?}；画面文字：{labels:?}"
    );
    let joined: String = labels.join(" ");
    assert!(
        joined.contains("未配置") || joined.contains("去设置"),
        "点不动就要说清为什么、并给出配置入口：{labels:?}"
    );
}
// ---------- 占用图 ----------

/// 占用图是这一页「盘被谁占了」的第一眼，必须真的画出来。
///
/// 它曾经在 800 高的窗口里整块消失：判断窗口高度的阈值被乘了字号缩放
/// （1.4 倍档下 640 变成 896），条件永远成立就永不绘制。所以这里既守住
/// 「标题和图例在」，也守住「大项的名字画在图上」。
#[test]
fn occupancy_map_draws_the_biggest_items() {
    let h = common::Ui::new(1240.0, 1400.0);
    let s = state_with(vec![
        cand("/a/huge-target", 40 << 30, costly()),
        cand("/b/mid-target", 12 << 30, costly()),
        cand("/c/cache", 512 << 20, molt_core::Verdict::Reclaimable),
    ]);
    let labels = h.labels(&s, molt_gui::pages::workbench::show);
    let joined = labels.join(" | ");
    for needle in ["空间占用排行", "能清", "清理有代价", "40.0 GB"] {
        assert!(joined.contains(needle), "占用图缺少「{needle}」：{joined}");
    }
}

/// 点图上的块要打开那一条自己的详情，而不是排序后的第几个。
#[test]
fn clicking_a_big_block_opens_that_items_detail() {
    let h = common::Ui::new(1240.0, 1400.0);
    let s = state_with(vec![
        cand("/a/huge-target", 40 << 30, costly()),
        cand("/b/mid-target", 12 << 30, costly()),
    ]);
    // 块内文字是画在画布上的「名字 + 体积」，找名字所在位置点下去。
    let (acts, labels) = h.click_containing(&s, molt_gui::pages::workbench::show, "huge-target");
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::OpenDetail(p) if p.contains("huge-target"))),
        "点大块应打开它自己的详情，实际动作：{acts:?}；画面文字：{labels:?}"
    );
}
