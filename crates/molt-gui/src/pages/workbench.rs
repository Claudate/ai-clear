//! 工作台：整个产品只有一条主线。
//!
//! 打开这一页要能立刻回答三件事，顺序就是版面的顺序：
//! 1. **现在该做什么**——一句结论 + 一个主按钮；
//! 2. **哪些建议清理、哪些留着**——按优先级分节，不是一长串按体积排的清单；
//! 3. **每一条到底是什么**——本地判定 + AI 解释同时挂在那一行上。
//!
//! AI 不再是独立的一页。它在扫描之后由用户点一次「让 AI 解释这次扫描」触发，
//! 结果直接落到每一条数据上；它失败也不会挡住本地扫描与手动清理。
//!
//! 互斥在这里体现成两件事：执行期间「勾选」与「动作」全部置灰并给出理由，
//! 而不是点了没反应。

use std::sync::Arc;

use eframe::egui;

use crate::action::Action;
use crate::state::{AppState, Filter, PendingItem, Priority, StatusKind, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{
    badge, busy_bar, card, danger_text_button, page_header, primary_button, progress_bar,
    secondary_button, section_header, state_block,
};
use molt_act::CleanMode;
use molt_core::Verdict;

/// 工作台页面。只读 `state`，改动一律作为 `Action` 追加到 `acts`。
///
/// 这条签名就是这次重构的契约：页面拿不到 `&mut AppState`，所以它**不可能**
/// 绕过 `action::apply` 去改状态。于是「点了这一下应该变成什么样」只在
/// `apply` 里有一份定义，也就能被单测直接断言。
pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    // 底栏先占位：主操作必须始终看得见，绝不能随清单滚出屏幕。
    //
    // 清单现在默认展开，底栏就跟常显——「选中 → 执行」是这一页最常走的一步，
    // 用户不该先把它找出来。
    let has_items = state
        .result
        .as_ref()
        .map(|r| !r.items.is_empty())
        .unwrap_or(false);
    if has_items {
        footer_bar(ui, state, p, acts);
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(ui, p);
            ui.add_space(12.0);

            // 摘要条取代了那只占半屏的圆环：盘还剩多少、能清多少、要不要重扫，
            // 三个问题一行答完。它在所有分支之前画——磁盘读数每帧都有，
            // 扫描没跑完也不该是一片空白。
            summary(ui, state, p, acts);
            // 清完之后「这次一共腾出多少」要有持久证据：底部结果条会消失，
            // 而这是用户唯一能回头确认的数字。
            if state.session_freed > 0 {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "本次运行已腾出 {}",
                            molt_scan::human_bytes(state.session_freed)
                        ))
                        .size(p.sz(12.0))
                        .strong()
                        .color(p.reclaimable),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            }
            ui.add_space(12.0);

            // 出错优先于「空」：读失败与「盘上很干净」在版面上长得像，
            // 但含义完全相反，绝不能画成一个。
            if let Some(err) = state.error.clone() {
                if state.result.is_none() {
                    failed_card(ui, state, p, &err, acts);
                    return;
                }
                // 旧结果还在：不丢掉它，但必须先说明它是旧的。
                stale_banner(ui, state, p, &err, acts);
            }

            let Some(result) = state.result.clone() else {
                if state.is_scanning() {
                    scanning_card(ui, state, p);
                } else {
                    hint_line(
                        ui,
                        p,
                        "还没有扫描结果。点上面的「重新扫描」开始——只读取文件信息，不会改动任何东西。",
                    );
                }
                return;
            };

            if result.items.is_empty() {
                hint_line(
                    ui,
                    p,
                    "没有找到可清理的内容：规则覆盖的目录目前都是空的，\
                     或者这些工具还没在这台 Mac 上产生缓存。",
                );
                return;
            }

            cached_note(ui, state, p);
            report_banner(ui, state, p, acts);
            // 清单不再折叠：这一页的主体就是它。
            list(ui, state, p, acts);
        });

    // 详情抽屉画在所有内容之上（Area，不占布局），Esc 或点暗处关闭。
    detail_panel(ui, state, p, acts);
}

fn header(ui: &mut egui::Ui, p: &Palette) {
    // 页头不放「重新扫描」：它属于摘要条那一段（「这一块盘现在还剩多少、要不要重查」），
    // 悬在标题行右上角离它要回答的问题太远。
    page_header(
        ui,
        p,
        "清理",
        "扫描只读；清理先放进临时空间，7 天内可还原。",
        |_ui| {},
    );
}

/// 顶部的摘要条：一条容量条 + 「可用 / 可清理」两个数字 + 重新扫描 + 状态。
///
/// 它取代了原来的圆环仪表盘。环好看，但它把「盘还剩多少、能清多少」这两件
/// 小事占掉一整屏，清单被挤到折叠状态——而用户来这里的目的正是看清清单。
/// 现在这些数字缩成一行，下面直接就是条目。
fn summary(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let (used_ratio, available) = match &state.disk {
        Some(Ok(d)) => (
            d.used as f32 / ((d.used + d.available).max(1) as f32),
            molt_scan::human_bytes(d.available),
        ),
        _ => (0.0, "—".to_string()),
    };
    let (clean_n, clean_bytes) = state.cleanable_summary();
    let clean_bytes_text = molt_scan::human_bytes(clean_bytes);
    let cleanable =
        (state.result.is_some() && clean_n > 0).then_some((clean_bytes_text.as_str(), clean_n));

    // 活动状态与上次扫描二选一：正在跑的时候，上一次的结果不再是重点。
    let act = state.activity();
    let activity = if let Some(label) = act.label() {
        let detail = match act {
            crate::state::Activity::Scanning => state.job.as_ref().and_then(|j| match j.peek() {
                crate::worker::Progress::Judging { done, total } if *total > 0 => {
                    Some(format!(" {done}/{total}"))
                }
                crate::worker::Progress::Walking { found } if *found > 0 => {
                    Some(format!(" 已找到 {found} 项"))
                }
                _ => None,
            }),
            crate::state::Activity::Cleaning => {
                Some(format!(" {:.0}%", (state.clean_anim * 100.0).min(99.0)))
            }
            _ => None,
        };
        format!("{label}{}", detail.unwrap_or_default())
    } else if state.clean_anim > 0.0 && state.clean_done_secs <= 0.0 {
        // 清完那一刻要留一小段收尾：`is_cleaning()` 已经翻成 false，
        // 但屏幕上的进度不该在 92% 处突然消失。
        format!("正在清理 {:.0}%", (state.clean_anim * 100.0).min(99.0))
    } else if state.clean_done_secs > 0.0 {
        // 清完那一刻在这一行交代结果：腾出了多少、东西去了哪。
        format!(
            "清理完成 +{} · {}",
            molt_scan::human_bytes(state.clean_total_bytes),
            if state.clean_was_delete {
                "已腾出空间"
            } else {
                "已放进临时空间"
            }
        )
    } else {
        String::new()
    };

    let scan_note = state
        .result
        .as_ref()
        .filter(|_| !state.is_scanning())
        .map(|r| format!("上次扫描 {} 项 · {:.0} 秒", r.items.len(), r.elapsed_secs))
        .unwrap_or_default();

    if crate::widgets::summary_strip(
        ui,
        p,
        used_ratio,
        &available,
        cleanable,
        state.is_scanning(),
        state.can_start(),
        &activity,
        &scan_note,
    ) {
        acts.push(Action::FollowNextStep(crate::state::NextStep::Retry));
    }
}

/// 一行可点的文字链：给筛选留一条「显示全部」的出口。
///
/// 筛选切到某一档后，用户得有一条回到全局的路，而不是再去分段控件里找。
fn link(ui: &mut egui::Ui, p: &Palette, text: &str, color: egui::Color32) -> bool {
    let resp = ui.add(
        egui::Label::new(egui::RichText::new(text).size(p.sz(11.5)).color(color))
            .wrap_mode(egui::TextWrapMode::Extend)
            .sense(egui::Sense::click()),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let r = resp.rect;
        ui.painter().hline(
            r.left()..=r.right(),
            r.bottom() + 1.0,
            egui::Stroke::new(1.0, color),
        );
    }
    resp.clicked()
}

fn report_banner(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let action = crate::widgets::op_report(ui, p, state.last_op.as_deref());
    match action {
        crate::widgets::ReportAction::None => {}
        crate::widgets::ReportAction::Dismiss => acts.push(Action::DismissReport),
        crate::widgets::ReportAction::Next(next) => acts.push(Action::FollowNextStep(next)),
    }
    if state.last_op.is_some() {
        ui.add_space(10.0);
    }
}

/// 显示的是上次留下的结果时，说明一句。
///
/// 不声不响地摆一堆数字，用户会默认它们是最新的——而这些数字直接引出
/// 删除动作。一行灰字就够了，不用横幅：它不阻断任何操作。
fn cached_note(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    // 只有「已经有结果、但这份结果不是这次扫出来的」才需要说明。
    if state.result.is_none() || state.scanned_now {
        return;
    }
    let at = state.last_scan_at.as_deref().unwrap_or("上次");
    hint_line(
        ui,
        p,
        &format!("上面是{at}那次扫描的结果，要更新就点「重新扫描」。"),
    );
    ui.add_space(6.0);
}

/// 重扫失败但旧结果还在时，先说清「下面这些是什么时候的」。
///
/// 不这么做的话，用户看到的是上一次成功的数字，却以为刚扫过——而清理
/// 是按这些数字做的决定，代价可能是一次不必要的重建。
fn stale_banner(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    err: &str,
    acts: &mut Vec<Action>,
) {
    // 框体交给 `components::banner`，这里只决定说什么。
    let detail = match &state.last_scan_at {
        Some(at) => format!("下面仍是 {at} 那次的结果"),
        None => "下面是一份较早的结果".to_string(),
    };
    if crate::components::banner::banner(
        ui,
        p,
        &format!("这次扫描失败：{err}"),
        &detail,
        Some("重试"),
        state.can_start(),
    ) {
        acts.push(Action::FollowNextStep(crate::state::NextStep::Retry));
    }
    ui.add_space(10.0);
}

/// 仪表盘下方的一行说明。
///
/// 空态不该再用一整块居中提示占掉半屏：仪表盘已经把「还剩多少」讲清楚了，
/// 这里只需要补一句「为什么清单是空的」。
fn hint_line(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.add(
        egui::Label::new(egui::RichText::new(text).size(p.sz(12.0)).color(p.text_dim))
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
}

/// 分档清单：首页的主体，默认展开。
///
/// 首页要回答的是「哪些能清、哪些要确认」——答案就是这张清单本身，
/// 不该再点一次才出现。顶部一行分段筛选，想只看某一档时点一下就换；
/// AI 解释收进这一行右侧的一个按钮，因为它是可选动作。
fn list(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(result) = state.result.clone() else {
        return;
    };
    let busy = state.activity();

    toolbar(ui, state, p, &busy, acts);
    ui.add_space(10.0);

    // hero 里的「要重建 / 是你的记录」两个入口只设了筛选，用户点了如果没滚动，
    // 会觉得「什么都没发生」——清单在屏幕外。这里把视图移到清单开头。
    if state.scroll_to_list {
        acts.push(Action::ConsumeScrollTo);
        ui.scroll_to_rect(ui.max_rect(), Some(egui::Align::TOP));
    }

    let visible = state.visible_items();
    if visible.is_empty() {
        empty_list(ui, state, p, &result, acts);
        return;
    }

    let mut toggle: Option<String> = None;

    let groups = [
        (Priority::Suggested, p.costly),
        (Priority::Safe, p.reclaimable),
        (Priority::Costly, p.costly),
        (Priority::Keep, p.protected),
    ];

    for (pr, tint) in groups {
        let rows: Vec<usize> = visible
            .iter()
            .copied()
            .filter(|&i| state.priority_of(&result.items[i]) == pr)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let bytes: u64 = rows
            .iter()
            .map(|&i| result.items[i].alloc_bytes.max(result.items[i].bytes))
            .sum();

        section_header(ui, p, pr.label(), pr.hint(), rows.len(), bytes, tint);

        // 清单是用户主动点开的，就不在每节里再藏几行——
        // 连续两次「展开」会让人以为后面还有更隐蔽的东西。
        for i in &rows {
            match row(ui, state, p, &result, *i) {
                Some(RowOutcome::Toggle) => {
                    toggle = Some(result.items[*i].path.display().to_string())
                }
                Some(RowOutcome::Detail) => acts.push(Action::OpenDetail(
                    result.items[*i].path.display().to_string(),
                )),
                None => {}
            }
        }
        ui.add_space(12.0);
    }

    if let Some(path) = toggle {
        acts.push(Action::ToggleCheck(path));
    }
}

/// 清单上方的工具条：筛选分段 + 搜索 + AI 解释入口。
fn toolbar(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    busy: &crate::state::Activity,
    acts: &mut Vec<Action>,
) {
    let (clean_n, clean_bytes) = state.cleanable_summary();
    let (costly_n, _) = state.summary_of(Priority::Costly);
    let (keep_n, _) = state.summary_of(Priority::Keep);
    // 「久未使用」与「能清」是同一批条目按时间再筛一次：30 天没动过的
    // 东西更可能是真的忘了，而不是正在用的缓存。
    let (recent_n, _) = state.summary_of(Priority::Suggested);
    let total_n = clean_n + costly_n + keep_n;

    ui.horizontal_wrapped(|ui| {
        let opts = [
            (
                format!(
                    "能清 {clean_n} 项 · {}",
                    molt_scan::human_bytes(clean_bytes)
                ),
                state.filter == Filter::Cleanable,
            ),
            (format!("要确认 {costly_n}"), state.filter == Filter::Costly),
            (format!("保留 {keep_n}"), state.filter == Filter::Protected),
            (
                format!("久未使用 {recent_n}"),
                state.filter == Filter::Recent,
            ),
            (format!("全部 {total_n}"), state.filter == Filter::All),
        ];
        let refs: Vec<(&str, bool)> = opts.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        if let Some(i) = crate::widgets::segmented(ui, p, &refs) {
            acts.push(Action::SetFilter(match i {
                0 => Filter::Cleanable,
                1 => Filter::Costly,
                2 => Filter::Protected,
                3 => Filter::Recent,
                _ => Filter::All,
            }));
        }

        // 筛选一旦不是「全部」，就必须永远有一条回得去的路。
        // hero 里的两个链接会切筛选，用户点完只看到列表变了、却找不到开关。
        if state.filter != Filter::All && link(ui, p, "显示全部", p.reclaimable) {
            acts.push(Action::SetFilter(Filter::All));
        }

        ui.add_space(8.0);
        // 搜索框用**草稿**而不是 `state.query`：视图只有 `&AppState`，
        // 拿不到 `&mut String`。输入先进草稿，回车或失焦时再作为动作提交。
        let mut draft = state.query_draft.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut draft)
                .hint_text(
                    egui::RichText::new("搜索名字或路径")
                        .size(p.sz(12.0))
                        .color(p.text_faint),
                )
                .desired_width(200.0 * p.scale)
                .margin(egui::Margin::symmetric(8, 6)),
        );
        if resp.changed() {
            acts.push(Action::SetQueryDraft(draft.clone()));
        }
        if resp.lost_focus() {
            acts.push(Action::CommitQuery);
        }
        if !state.query.is_empty() && secondary_button(ui, p, "清空", true) {
            acts.push(Action::ClearQuery);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ai_action(ui, state, p, busy, acts);
        });
    });

    if busy.is_busy() {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(busy.blocking_hint())
                    .size(p.sz(10.5))
                    .color(p.costly),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
    }
}

/// AI 解释：一个按钮 + 一行状态，不再是一张独立的卡片。
fn ai_action(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    busy: &crate::state::Activity,
    acts: &mut Vec<Action>,
) {
    if state.ai.is_running() {
        badge(ui, p, "AI 正在解释…", p.costly, p.costly_soft);
        busy_bar(ui, p, p.sz(90.0));
        return;
    }
    if state.ai.has_notes() {
        badge(
            ui,
            p,
            &format!("AI 已解释 {} 条", state.ai.notes.len()),
            p.reclaimable,
            p.accent_soft,
        );
        ui.add_space(4.0);
    }
    let resolved = state.ai.resolve();
    let label = if state.ai.has_notes() {
        "重新解释"
    } else {
        "让 AI 解释"
    };
    let ready = state.can_start() && resolved.is_ok();
    if secondary_button(ui, p, label, ready) {
        acts.push(Action::Analyze);
    }
    // 置灰必须说明为什么：一个点不动又不解释的按钮，用户只会以为软件坏了。
    // 这一行只在真的点不了时出现，正常状态下不占版面。
    //
    // 文案分两种：已经解释过、但配置现在读不出来（改过设置、密钥删了）时，
    // 不能写「未配置」——那和左边「已解释 24 条」直接打架，用户会以为
    // 那些解释是假的。说清是「现在重新解释不了」才准确。
    if !resolved.is_ok() && !state.ai.is_running() {
        let (hint, fix) = if state.ai.has_notes() {
            ("当前配置不可用，无法重新解释", "去设置")
        } else {
            ("AI 未配置，需要先填模型与密钥", "去设置")
        };
        if secondary_button(ui, p, fix, true) {
            acts.push(Action::GoSettingsForError);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(hint)
                    .size(p.sz(10.5))
                    .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Truncate),
        );
    }
    if let Some(f) = &state.ai.failure {
        if !state.ai.is_running() {
            // 顺序：原因在左、动作在右。这段画在 right_to_left 布局里，
            // 所以先加的那个反而靠右——先加按钮，用户才会先读到原因。
            if f.fixable_by_config && secondary_button(ui, p, "去设置", true) {
                acts.push(Action::GoSettingsForError);
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("上次失败：{}", crate::app::short(&f.message, 34)))
                        .size(p.sz(10.5))
                        .color(p.danger),
                )
                .wrap_mode(egui::TextWrapMode::Truncate),
            );
        }
    }
    // 未配置时什么都不画：首页要回答的是「还剩多少、能清多少、点哪个」，
    // 「AI 未配置」是设置页的事。这里挂一句尾巴，只会让人以为清理被挡住了。
    let _ = busy;
}

/// 清单为空时的三种不同情况，各自说清是哪一种。
fn empty_list(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    result: &crate::worker::ScanResult,
    acts: &mut Vec<Action>,
) {
    if !state.query.is_empty() {
        state_block(
            ui,
            p,
            "○",
            "没有匹配的内容",
            "换个关键词，或者清掉搜索框看全部。",
            p.text_faint,
        );
        if secondary_button(ui, p, "清空搜索", true) {
            acts.push(Action::ClearQuery);
        }
        return;
    }

    // 「全部受保护」是一种真实且常见的结果，不能画成「什么都没找到」
    let kept = state.count_of(Priority::Keep);
    if kept > 0 {
        let bytes: u64 = result
            .items
            .iter()
            .filter(|c| state.priority_of(c) == Priority::Keep)
            .map(|c| c.alloc_bytes.max(c.bytes))
            .sum();
        card(ui, p, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("这一档里没有内容，扫到的都在「保留」里")
                        .size(p.sz(14.0))
                        .strong(),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{} 项、共 {}：都是你的记录、密钥或正在使用的内容，不会被移动。",
                        kept,
                        molt_scan::human_bytes(bytes)
                    ))
                    .size(p.sz(11.5))
                    .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(8.0);
            if secondary_button(ui, p, "看看这些是什么", true) {
                acts.push(Action::SetFilter(Filter::Protected));
            }
        });
        return;
    }

    state_block(
        ui,
        p,
        "○",
        "这一档里没有内容",
        "换一个分段看，或者重新扫描一次。",
        p.text_faint,
    );
}

/// AI 解释最多两行。
///
/// 超过两行就裁掉：第三行开始通常是重复的修饰，而一屏多滚半屏的代价
/// 是用户看不到下面那些条目。行高按「两行字 + 内边距」固定，不做单行截断——
/// 两行正好够说清「这是什么」。
fn ai_note_clamped(ui: &mut egui::Ui, p: &Palette, text: &str) {
    let w = ui.available_width();
    let h = p.sz(36.0);
    // 先占位，再在**这块矩形**里画。
    //
    // 不能用 `allocate_ui_with_layout`：它给子 ui 的 `max_rect` 会被布局扩展，
    // 裁剪矩形跟着变大，第三行就漏到行的下面去了（清单滚到底时能看见它
    // 越过底栏）。这里把 max_rect 钉死成占位矩形，多出来的文字直接被裁掉。
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    child.set_clip_rect(rect);
    badge(&mut child, p, "AI", p.costly, p.costly_soft);
    child.add_space(p.sz(4.0));
    child.add(
        egui::Label::new(egui::RichText::new(text).size(p.sz(10.5)).color(p.text_dim))
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
}

/// 一行上可能发生的两种点击。
///
/// 勾选与「详情」是两个不同的动作，行本身只能返回一个值，所以用一个枚举
/// 把「点了哪里」带出去，由调用方统一翻成 `Action`。
enum RowOutcome {
    Toggle,
    Detail,
}

/// 一行数据。返回这一下点在了哪里（由调用方统一改状态，避免同帧多次借用）。
///
/// 行内三层信息，从上到下：
/// - 名字 + 判定徽章
/// - 路径（缩写成 `~/...`）
/// - **本地判定为什么这样** + **AI 说它是什么**——这两句是这次重做的重点，
///   用户不该再为了「这是什么」去另一个页面
fn row(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    result: &crate::worker::ScanResult,
    i: usize,
) -> Option<RowOutcome> {
    let c = &result.items[i];
    let path = c.path.display().to_string();
    let pr = state.priority_of(c);
    let color = p.verdict(&c.verdict);
    let soft = p.verdict_soft(&c.verdict);
    let actionable = pr.selectable();
    let checked = state.is_checked(&path);
    let note = state.ai_note(&path);
    let selectable = state.can_select() && actionable;

    let full_w = ui.available_width();
    // 逐项说明只在「这一项自己的理由和分节标题不同」时才画。
    // 可回收档那句固定文案已经由分节标题统一讲过，十一行重复同一句是噪声；
    // 「有代价」那些要重下什么、重编多久是逐项不同的，省掉等于让用户盲选。
    let why = c.verdict.explanation();
    let has_why = why != Verdict::Reclaimable.explanation();
    // 行高只有两档：一行字与「一行字 + 两行解释」。清单默认展开后，
    // 一屏能多读几行比每行都长得像卡片更重要。
    let min_h = if note.is_some() {
        p.sz(62.0)
    } else {
        p.sz(44.0)
    };

    let frame = egui::Frame::NONE
        // 选中只改底色：再叠一圈蓝描边，十一项连起来就是一整片蓝，
        // 蓝色也就不再表示「这里可交互」了。
        //
        // 不再给每行画圆角边框：一行一张卡片会把清单读成一叠卡片，行与行
        // 之间全是缝；列表靠一条 1 点分隔线连成一张表就够了。
        .fill(if checked {
            p.accent_soft
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(14, 4));

    // 「详情」按钮是否被点中。它要跨出 Frame 的闭包，所以先声明在外。
    let mut detail_clicked = false;
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(full_w - 30.0);
            ui.set_min_height(min_h - p.sz(8.0));
            ui.horizontal(|ui| {
                crate::components::checkbox::checkbox(
                    ui,
                    p,
                    checked,
                    actionable,
                    state.can_select(),
                );
                ui.add_space(4.0);

                let size_w = p.sz(92.0);
                let left_w = (ui.available_width() - size_w - 12.0).max(p.sz(160.0));
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, min_h - p.sz(8.0)),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        // 名字、路径、逐项说明挤在同一行：这是「一屏能读几条」
                        // 的关键。分两行写时，大字号（1.4 倍）下光是两行文字
                        // 加内边距就有 104 px，十一项要滚三屏。
                        // 说明只在「这一项自己的理由和分节标题不同」时才画：
                        // 可回收档那句固定文案已经由分节标题统一讲过了。
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 2.0);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(crate::paths::friendly_name(
                                        &state.home,
                                        &c.path,
                                    ))
                                    .size(p.sz(12.5))
                                    .strong()
                                    .color(p.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                            badge(ui, p, pr.label(), color, soft);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(crate::paths::display_in(
                                        &state.home,
                                        &c.path,
                                    ))
                                    .size(p.sz(10.0))
                                    .monospace()
                                    .color(p.text_faint),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                            if has_why {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&why).size(p.sz(10.5)).color(color),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Truncate),
                                );
                            }
                        });
                        // AI 解释：它是什么。直接挂在这一行上，最多两行。
                        if let Some(n) = note {
                            ai_note_clamped(ui, p, n);
                        }
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(size_w, min_h - p.sz(8.0)),
                        egui::Layout::top_down(egui::Align::Max),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(molt_scan::human_bytes(
                                        c.alloc_bytes.max(c.bytes),
                                    ))
                                    .size(p.sz(13.5))
                                    .strong()
                                    .color(color),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                            if c.suggest_clean {
                                if let Some(d) = idle_days(c) {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("{d} 天未用"))
                                                .size(p.sz(10.0))
                                                .color(p.text_faint),
                                        )
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                    );
                                }
                            }
                            // 详情入口：一行放不下的完整路径、判定理由与 AI 解释
                            // 都在抽屉里。小字放在体积下面，不跟勾选抢注意力。
                            //
                            // 用无框按钮而不是带 Sense 的 Label：Label 的命中矩形
                            // 跟着文字走，在大字号下窄得几乎点不到。
                            let detail = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("详情")
                                        .size(p.sz(10.5))
                                        .color(p.text_faint),
                                )
                                .frame(false)
                                .min_size(egui::vec2(p.sz(48.0), p.sz(20.0))),
                            );
                            if detail.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            detail_clicked = detail.clicked();
                        },
                    );
                });
            });
        })
        .response;
    // 行与行之间的 1 点分隔线：列表靠它连成一张表，而不是靠每行自己的边框。
    ui.painter().hline(
        resp.rect.left()..=resp.rect.right(),
        resp.rect.bottom(),
        egui::Stroke::new(1.0, p.row_separator),
    );
    if actionable {
        // 行末那一列留给「详情」：整行点击若把它也盖住，后注册的交互会
        // 吃掉按钮的 click，详情就永远打不开。
        let mut hit_rect = resp.rect;
        hit_rect.max.x -= p.sz(52.0);
        let hit = ui.interact(
            hit_rect,
            ui.scope_id().with(("wb-row", i)),
            egui::Sense::click(),
        );
        if selectable && hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if detail_clicked {
            return Some(RowOutcome::Detail);
        }
        if selectable && hit.clicked() {
            return Some(RowOutcome::Toggle);
        }
        return None;
    }
    // 保留项不能勾选，但它的详情仍然值得看。
    detail_clicked.then_some(RowOutcome::Detail)
}

/// 首页的详情抽屉：一行放不下的信息（完整路径、大小、判定理由、AI 解释）。
///
/// 只读。抽屉打开时清单仍然可见、可勾——用户常常一边看某一项的简介，
/// 一边对照着勾别的行。动作入口留在清单和底栏，这里不放执行按钮。
fn detail_panel(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(path) = state.detail_path.clone() else {
        return;
    };
    let Some(c) = state.result.as_ref().and_then(|r| {
        r.items
            .iter()
            .find(|c| c.path.display().to_string() == path)
    }) else {
        // 重扫描之后这一项可能不在了：抽屉自己关上，别停在一份过期数据上。
        acts.push(Action::CloseDetail);
        return;
    };

    let pr = state.priority_of(c);
    let mut rows: Vec<(String, String)> = vec![
        ("路径".to_string(), path.clone()),
        (
            "大小".to_string(),
            molt_scan::human_bytes(c.alloc_bytes.max(c.bytes)),
        ),
        ("判定".to_string(), c.verdict.explanation().to_string()),
    ];
    if let Some(d) = idle_days(c) {
        rows.push(("最近使用".to_string(), format!("{d} 天前")));
    }
    rows.push((
        "本次勾选".to_string(),
        if state.is_checked(&path) {
            "已勾选".to_string()
        } else {
            "未勾选".to_string()
        },
    ));

    let title = crate::paths::friendly_name(&state.home, &c.path);
    let note = state.ai_note(&path);
    let outcome = crate::widgets::detail_drawer(
        ui.ctx(),
        p,
        &title,
        (
            pr.label(),
            p.verdict(&c.verdict),
            p.verdict_soft(&c.verdict),
        ),
        &rows,
        note,
        true,
    );
    match outcome {
        crate::widgets::DetailDrawerOutcome::None => {}
        crate::widgets::DetailDrawerOutcome::Close => acts.push(Action::CloseDetail),
        crate::widgets::DetailDrawerOutcome::Reveal => acts.push(Action::RevealApp(path)),
    }
}

/// 窗口底部固定操作条：**主操作不随清单滚动**。
///
/// 旧版把「清理这些」埋在首页那张大卡片中间，下面还接着 AI 条与几百行清单；
/// 用户往下翻一眼清单，那个按钮就再也看不到了，只能往回滚。
/// 清理工具里「选中 → 执行」是最常走的一步，它必须一直在手边。
fn footer_bar(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    egui::Panel::bottom("workbench-actions")
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .inner_margin(egui::Margin::symmetric(28, 10)),
        )
        .show(ui, |ui| {
            let n = state.checked_count();
            let can = state.can_start();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = p.sz(1.0);
                    if n == 0 {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("还没有选中任何项")
                                    .size(p.sz(12.5))
                                    .color(p.text_dim),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("在下面清单里点一行即可勾选")
                                    .size(p.sz(10.5))
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "已选 {n} 项 · {}",
                                    molt_scan::human_bytes(state.checked_bytes())
                                ))
                                .size(p.sz(14.0))
                                .strong(),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        // 筛选只影响「看到哪些行」，不影响勾选。当前筛选下看不见
                        // 但已被勾中的那些必须说清，否则用户会以为自己只选了 3 个。
                        let hidden = state.checked_outside_filter();
                        if hidden > 0 {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "其中 {hidden} 项在当前筛选之外（筛选不会取消勾选）"
                                    ))
                                    .size(p.sz(10.5))
                                    .color(p.costly),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new("放入临时空间，7 天内可还原")
                                        .size(p.sz(10.5))
                                        .color(p.text_faint),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, "清理这些（可还原）", n > 0 && can) {
                        acts.push(Action::PrepareClean(CleanMode::Quarantine));
                    }
                    // 不可恢复的路留在主按钮左边，降成一行红字：它需要用户
                    // 主动去找，而不是和主按钮并排抢眼。10 月 23 日那次误删
                    // 两个真实目录，就是因为它当时和主按钮长得一样大。
                    if danger_text_button(ui, p, "直接清理…", n > 0 && can) {
                        acts.push(Action::PrepareClean(CleanMode::Delete));
                    }
                    if secondary_button(ui, p, "全不选", n > 0 && state.can_select()) {
                        acts.push(Action::ClearSelection);
                    }
                });
            });

            if state.activity().is_busy() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(state.activity().blocking_hint())
                            .size(p.sz(10.5))
                            .color(p.costly),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
}

fn scanning_card(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    card(ui, p, |ui| {
        ui.label(egui::RichText::new("正在检查…").size(p.sz(15.0)).strong());
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new("第一次扫描要遍历各工具的缓存目录，通常需要几十秒。")
                .size(p.sz(11.5))
                .color(p.text_dim),
        );
        ui.add_space(12.0);

        match state.job.as_ref().map(|j| j.peek().clone()) {
            Some(crate::worker::Progress::Judging { done, total }) if total > 0 => {
                progress_bar(
                    ui,
                    p,
                    done as f32 / total as f32,
                    p.sz(320.0).min(ui.available_width()),
                    &format!("已检查 {done} / {total} 项"),
                );
            }
            Some(crate::worker::Progress::Walking { found }) if found > 0 => {
                busy_bar(ui, p, p.sz(320.0).min(ui.available_width()));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("已找到 {found} 项，正在测量大小"))
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                );
            }
            _ => {
                busy_bar(ui, p, p.sz(320.0).min(ui.available_width()));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("正在遍历目录…")
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                );
            }
        }
    });
}

fn failed_card(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    err: &str,
    acts: &mut Vec<Action>,
) {
    card(ui, p, |ui| {
        ui.label(
            egui::RichText::new("扫描没有完成")
                .size(p.sz(15.0))
                .strong()
                .color(p.danger),
        );
        ui.add_space(3.0);
        ui.add(
            egui::Label::new(egui::RichText::new(err).size(p.sz(12.0)).color(p.text_dim))
                .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("扫描是只读的，你的文件没有被改动过。")
                .size(p.sz(11.5))
                .color(p.reclaimable),
        );
        ui.add_space(10.0);
        if primary_button(ui, p, "重试扫描", state.can_start()) {
            acts.push(Action::FollowNextStep(crate::state::NextStep::Retry));
        }
    });
}

/// 确认框。清理与隔离区彻底删除共用同一套视觉语言，文案只讲这一次的后果。
pub fn confirm_modal(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(pending) = state.pending_clean.clone() else {
        return;
    };
    let is_delete = pending.mode == CleanMode::Delete;
    let title = if is_delete {
        "直接清理这些内容？"
    } else {
        "把这些内容放入临时空间？"
    };

    // 骨架来自 `components::confirm_dialog`，与临时空间页的「彻底删除」共用：
    // 标题层级、量化数字、红底警示、按钮顺序都只有一份实现。
    // 这一页只负责填词，以及中间那份**带 AI 解释**的对象清单。
    let mut dialog = crate::components::ConfirmDialog::new(
        "molt-confirm-clean",
        if is_delete {
            crate::components::ConfirmKind::Danger
        } else {
            crate::components::ConfirmKind::Normal
        },
        title,
    )
    .summary(format!(
        "{} 项 · 共 {}",
        pending.count(),
        molt_scan::human_bytes(pending.bytes)
    ))
    .confirm_label(if is_delete {
        "确认直接清理"
    } else {
        "确认放入临时空间"
    })
    .width(480.0);

    dialog = if is_delete {
        dialog.warning(
            "这是永久删除，不可恢复。",
            "内容不会被放进临时空间，删掉之后没有任何找回的途径。",
        )
    } else {
        dialog.note(format!(
            "临时空间会保留 {} 天，期间随时可以还原。可用空间不会立刻变多——文件只是被移到了我们自己的临时空间里。",
            crate::paths::DEFAULT_RETAIN_DAYS
        ))
    };

    let outcome = dialog.show(ui, p, |ui| {
        object_list(ui, p, &pending.items, state, is_delete);
    });

    acts.extend(confirm_actions(outcome));
}

/// 确认框给出结果后要发的动作。
///
/// 确认只发 `RunPendingClean` 这一条：它经 `clean_from_pending` 取走批次的同时
/// 就把 `pending_clean` 清空了。曾经这里先推 `CancelPendingClean` 再推
/// `RunPendingClean`，两条按序执行时前一条已经把批次清空，后一条拿不到东西
/// 就静默返回——弹窗关了、盘上没动、也没有任何提示，用户看到的正是「点了没用」。
///
/// 抽成函数是为了让测试能直接拿这份动作去 `apply`，验证它真的换出了
/// `Effect::Clean`，而不是只断言「这里写了两个 push」。
fn confirm_actions(outcome: crate::components::ConfirmOutcome) -> Vec<Action> {
    match outcome {
        crate::components::ConfirmOutcome::Confirmed => vec![Action::RunPendingClean],
        crate::components::ConfirmOutcome::Cancelled => vec![Action::CancelPendingClean],
        crate::components::ConfirmOutcome::Open => Vec::new(),
    }
}

/// 确认框里的对象清单。只给数字是不够的——用户有权在不可撤销之前看见选了什么，
/// 以及 AI 对每一条的解释。
///
/// 框体与行布局交给 `components::confirm_dialog::item_list`，这里只负责
/// 把每条转成「显示用的路径 + AI 说明」两行。
fn object_list(
    ui: &mut egui::Ui,
    p: &Palette,
    items: &[PendingItem],
    state: &AppState,
    danger: bool,
) {
    const SHOWN: usize = 5;
    let rows: Vec<(String, String)> = items
        .iter()
        .take(SHOWN)
        .map(|item| {
            (
                crate::paths::display_in(&state.home, &item.path),
                item.note.clone().unwrap_or_default(),
            )
        })
        .collect();
    let footer = (items.len() > SHOWN).then(|| {
        format!(
            "还有 {} 项，共 {}",
            items.len() - SHOWN,
            molt_scan::human_bytes(items.iter().skip(SHOWN).map(|i| i.bytes).sum())
        )
    });
    crate::components::confirm_dialog::item_list(ui, p, danger, &rows, footer);
}

fn idle_days(c: &molt_core::Candidate) -> Option<u64> {
    if let Some(d) = c.evidence.iter().find_map(|e| match e {
        molt_core::Evidence::LastUsed { days_ago } => Some(*days_ago),
        _ => None,
    }) {
        return Some(d);
    }
    let then = c.last_used?;
    std::time::SystemTime::now()
        .duration_since(then)
        .ok()
        .map(|d| d.as_secs() / 86_400)
}

/// 起一次扫描。互斥由 `can_start` 在按钮层挡，这里再确认一次。
pub fn start_scan(state: &mut AppState) {
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    let Some(rules) = state.rules.clone() else {
        state.error = Some("规则库没有加载成功".into());
        return;
    };
    state.refresh_disk();
    state.error = None;
    state.status = Some(StatusMessage::info("正在扫描…"));
    state.job = Some(crate::worker::ScanJob::start(
        rules,
        state.home.clone(),
        true,
    ));
}

/// 起一次 AI 解释。所有前置条件在这里集中判断，
/// 这样按钮的可用状态与点击后的行为不会分叉。
pub fn start_analysis(state: &mut AppState) {
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    let resolved = match state.ai.resolve() {
        Ok(r) => r,
        Err(e) => {
            state.ai.failure = Some(crate::ai_worker::AiFailure {
                message: e,
                fixable_by_config: true,
                raw: None,
            });
            return;
        }
    };
    let Some(result) = state.result.clone() else {
        state.status = Some(StatusMessage::warn("还没有扫描结果，先扫描一次"));
        return;
    };

    let disk = match &state.disk {
        Some(Ok(d)) => Some(*d),
        _ => None,
    };
    state.ai.failure = None;
    state.ai.job = Some(crate::ai_worker::AiJob::start(
        resolved,
        state.ai.config.clone(),
        result.items.clone(),
        state.home.clone(),
        disk,
    ));
    state.status = Some(StatusMessage::info("正在让 AI 解释这次扫描…"));
}

/// 执行清理。与 CLI 走同一个 `molt_act::Executor`。
///
/// 批次由调用方传入（来自 `Effect::Clean`），不再从 `state.pending_clean` 里取：
/// 两者在确认框按下确定的那一刻已经是同一个东西，再读一次状态等于给
/// 「实际执行的」与「用户确认的」留下分叉的可能。
pub fn start_clean(state: &mut AppState, pending: &crate::state::PendingClean) {
    let Some(result) = state.result.clone() else {
        state.pending_clean = None;
        return;
    };
    if !state.can_start() {
        state.pending_clean = None;
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }

    let selected: Vec<molt_core::Candidate> = result
        .items
        .iter()
        .filter(|c| state.is_checked(&c.path.display().to_string()))
        .filter(|c| pending.items.iter().any(|i| i.path == c.path))
        .cloned()
        .collect();

    if selected.is_empty() {
        state.pending_clean = None;
        state.status = Some(StatusMessage::warn("没有选中任何项，未执行清理"));
        return;
    }

    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.pending_clean = None;
            state.status = Some(StatusMessage::error(format!(
                "打开审计库失败，未执行清理：{e}"
            )));
            return;
        }
    };
    let batch = molt_act::now_iso8601().replace(':', "-");
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), batch);

    state.pending_clean = None;
    // 不再往状态里塞「正在清理…」：进度已经由首页圆环自己表达，
    // 再弹一个窗只会挡住那块正在动的环。
    // 圆环要拿这两个数去写圆心（项数与体积），不能等结果报告回来才有数。
    // 同时把动画清零：每批清理都是从 12 点开始重新长出来。
    state.clean_total_items = selected.len();
    state.clean_total_bytes = selected.iter().map(|c| c.alloc_bytes.max(c.bytes)).sum();
    state.clean_anim = 0.0;
    state.clean_done_secs = 0.0;
    state.clean_job = Some(crate::worker::CleanJob::start(
        selected,
        store,
        q,
        pending.mode,
        crate::paths::DEFAULT_RETAIN_DAYS,
    ));
}

/// 保留旧名给测试与调用方：语义等价于「按当前 `pending_clean` 执行」。
pub fn do_clean(state: &mut AppState) {
    if let Some(pending) = state.pending_clean.clone() {
        start_clean(state, &pending);
    }
}

/// 执行一次卸载：把应用本体与它的残留一起移入临时空间。
///
/// 为什么不复用 [`start_clean`]：那一份的候选来自「上一次扫描结果 ∩ 用户勾选」，
/// 而卸载的对象（`/Applications/*.app` 与 `~/Library` 里的残留）根本不在扫描清单里。
/// 走它只会得到空批次——这正是早先「点了卸载什么都没发生」的原因。
///
/// 复用的是什么：`CleanJob` 与隔离模式本身。所以卸错了同样能在临时空间里还原，
/// 审计库里记的也是同一种记录。
pub fn start_uninstall(state: &mut AppState, pending: &crate::state::PendingClean) {
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }

    // 先确认本体还在。应用在界面列出之后被拖进废纸篓是常事，
    // 这时候还要跑一遍只会让整批残留去陪绑失败。
    let mut items = pending.items.clone();
    let missing: Vec<String> = items
        .iter()
        .filter(|i| !i.path.exists())
        .map(|i| i.path.display().to_string())
        .collect();
    items.retain(|i| i.path.exists());
    if items.is_empty() {
        state.uninstall.removing = None;
        state.status = Some(StatusMessage::warn(
            "这些文件已经不在原处了，没有可卸载的内容",
        ));
        return;
    }

    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.uninstall.removing = None;
            state.status = Some(StatusMessage::error(format!(
                "打开审计库失败，未执行卸载：{e}"
            )));
            return;
        }
    };

    // 卸载的东西一律判「有代价」：它们不会自动重建。
    // 这个档位只影响展示口径，隔离与还原不看它。
    let candidates: Vec<molt_core::Candidate> = items
        .iter()
        .map(|i| {
            let path = i.path.clone();
            let kind = if path.is_dir() {
                molt_core::ItemKind::Dir
            } else {
                molt_core::ItemKind::File
            };
            molt_core::Candidate::new(
                &path,
                kind,
                molt_core::Verdict::Costly {
                    rebuild: molt_core::RebuildCost::new(
                        molt_core::CostKind::Reinstall,
                        "卸载后需要重新安装",
                    ),
                },
            )
            .with_size(i.bytes, i.bytes, 0)
        })
        .collect();

    let batch = molt_act::now_iso8601().replace(':', "-");
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), batch);

    state.clean_total_items = candidates.len();
    state.clean_total_bytes = candidates.iter().map(|c| c.alloc_bytes.max(c.bytes)).sum();
    state.clean_anim = 0.0;
    state.clean_done_secs = 0.0;
    // 卸载永远走隔离模式：移进临时空间才谈得上「卸错了能放回去」。
    state.clean_job = Some(crate::worker::CleanJob::start(
        candidates,
        store,
        q,
        molt_act::CleanMode::Quarantine,
        crate::paths::DEFAULT_RETAIN_DAYS,
    ));
    let total = molt_scan::human_bytes(state.clean_total_bytes);
    state.status = Some(StatusMessage::info(if missing.is_empty() {
        format!("正在卸载，共 {total}；完成前请别关窗口")
    } else {
        format!("正在卸载，共 {total}；{} 项已不在原处，跳过", missing.len())
    }));
}

/// 取回后台清理的结果。
pub fn pump_clean(state: &mut AppState) {
    let Some(job) = state.clean_job.take() else {
        return;
    };
    let mut job = job;
    match job.poll().clone() {
        crate::worker::CleanProgress::Done(report) => {
            state.session_freed += report.deleted_bytes;
            let op = report_to_op(&report);
            state.status = Some(StatusMessage {
                kind: op.kind,
                text: op.summary.clone(),
            });
            state.last_op = Some(Arc::new(op));
            // 圆环跑满、满蓝停 1.8 秒再回到磁盘读数：清完那一下要看得见。
            state.clean_anim = 1.0;
            state.clean_done_secs = 1.8;
            // 「放入临时空间」不释放空间（只是 rename），圆心不能写成「已腾出」。
            // 只有直接清理才是真的腾出来了；临时空间那批只报「移走了多少」。
            state.clean_was_delete = report.mode == CleanMode::Delete;
            state.clean_total_bytes = if state.clean_was_delete {
                report.deleted_bytes
            } else {
                report.moved_bytes
            };
            // 清完立刻重扫：清单里现在应该是清理后的真实状态
            crate::pages::workbench::start_scan(state);
            crate::pages::quarantine::reload(state);
            crate::pages::history::reload(state);
            state.refresh_disk();
            // 卸载页的清单也过期了：那一批里可能就有刚卸掉的应用。
            // 丢掉缓存，下次进这一页会重新列（否则会列出已经不存在的应用）。
            state.uninstall.listed_at = None;
        }
        _ => {
            state.clean_job = Some(job);
        }
    }
}

fn report_to_op(report: &crate::worker::CleanReport) -> crate::state::OpReport {
    let failed = report.failures.len();
    let (done, done_bytes) = match report.mode {
        CleanMode::Delete => (report.deleted, report.deleted_bytes),
        CleanMode::Quarantine => (report.moved, report.moved_bytes),
    };

    let summary = match report.mode {
        CleanMode::Delete => format!(
            "已永久删除 {} 项，释放 {}",
            report.deleted,
            molt_scan::human_bytes(report.deleted_bytes)
        ),
        CleanMode::Quarantine => format!(
            "已放入临时空间 {} 项，共 {}（{} 天内可还原）",
            report.moved,
            molt_scan::human_bytes(report.moved_bytes),
            crate::paths::DEFAULT_RETAIN_DAYS
        ),
    };

    if done == 0 {
        let first = report
            .failures
            .first()
            .map(|(path, reason)| format!("{}：{reason}", crate::paths::display_path_str(path)))
            .unwrap_or_else(|| "没有可处理的内容".to_string());
        return crate::state::OpReport {
            summary: format!("清理没有生效：{first}"),
            done: 0,
            done_bytes: 0,
            failures: report.failures.clone(),
            kind: StatusKind::Error,
            next: Some(crate::state::NextStep::Retry),
        };
    }

    if failed > 0 {
        crate::state::OpReport {
            summary: format!("{summary}；{failed} 项未处理，原因见下方"),
            done,
            done_bytes,
            failures: report.failures.clone(),
            kind: StatusKind::Warn,
            next: Some(crate::state::NextStep::Retry),
        }
    } else {
        // 做成之后把用户送到「刚移走的东西去哪了」——
        // 移入隔离区的去隔离区，直接删的只能去历史看记录。
        let next = match report.mode {
            CleanMode::Quarantine => crate::state::NextStep::Quarantine,
            CleanMode::Delete => crate::state::NextStep::History,
        };
        crate::state::OpReport {
            summary,
            done,
            done_bytes,
            failures: Vec::new(),
            kind: StatusKind::Info,
            next: Some(next),
        }
    }
}

/// 每帧推进 AI 任务：把解释落进状态，供清单逐行读取。
pub fn pump_ai(state: &mut AppState) {
    let Some(job) = state.ai.job.as_mut() else {
        return;
    };
    match job.poll().clone() {
        crate::ai_worker::AiProgress::Done(outcome) => {
            let home = state.home.clone();
            state.ai.notes = outcome
                .notes
                .iter()
                .map(|(p, n)| (p.display().to_string(), n.clone()))
                .collect();
            // 「AI 认为不能动」的路径单独存一份，用于排序时把它们挪进「建议保留」。
            // 只认模型显式表态过的条目；没表态的按本地判定走。
            state.ai.safe = outcome
                .report
                .explanations
                .iter()
                .filter_map(|e| {
                    let abs = if let Some(rest) = e.path.strip_prefix("~/") {
                        std::path::Path::new(&home).join(rest)
                    } else {
                        std::path::PathBuf::from(&e.path)
                    };
                    e.safe_to_clean.map(|v| (abs.display().to_string(), v))
                })
                .collect();
            state.ai.last_model = Some(outcome.model.clone());
            state.ai.last_sent = outcome.sent_items;
            state.ai.last_at = Some(molt_act::now_iso8601());
            state.ai.failure = None;
            state.ai.job = None;
            state.status = Some(StatusMessage::info(format!(
                "AI 已解释 {} 条，说明显示在对应条目上",
                state.ai.notes.len()
            )));
            // 解释可能把某些行挪进「建议保留」，重新按默认勾选那批对齐
            if let Some(r) = state.result.clone() {
                let keep: std::collections::HashSet<String> = r
                    .items
                    .iter()
                    .filter(|c| {
                        matches!(state.priority_of(c), Priority::Safe | Priority::Suggested)
                    })
                    .map(|c| c.path.display().to_string())
                    .collect();
                state.checked = keep;
            }
        }
        crate::ai_worker::AiProgress::Failed(f) => {
            state.ai.failure = Some(f.clone());
            state.ai.job = None;
            state.status = Some(StatusMessage::error(format!(
                "AI 解释失败：{}（本地清理不受影响）",
                f.message
            )));
        }
        crate::ai_worker::AiProgress::Stage(_) => {}
    }
}

/// 取后台扫描的进展。主循环与端到端测试都调它——
/// 测试必须走用户真会走的那条路径（包括「清理后自动重扫」），
/// 否则验的是另一套时序。
pub fn pump_scan(state: &mut AppState) {
    let Some(job) = state.job.take() else {
        return;
    };
    let mut job = job;
    match job.poll().clone() {
        crate::worker::Progress::Done(result) => {
            // 落一份缓存：下次打开窗口先用它铺页面，不必再等十几秒。
            result.save_cache(&crate::paths::scan_cache());
            state.apply_result(result);
            state.refresh_disk();
            // 记下这份结果的时刻：下次扫描失败时要用它说明「旧数据是哪来的」。
            state.last_scan_at = Some(crate::paths::now_hhmm());
            state.scanned_now = true;
            if state.error.is_none() {
                state.status = None;
            }
        }
        crate::worker::Progress::Failed(msg) => {
            state.error = Some(msg.clone());
            state.status = Some(StatusMessage::error(msg));
            state.refresh_disk();
        }
        _ => {
            state.job = Some(job);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use std::sync::Arc;

    fn painted_text(ctx: &egui::Context, state: &mut AppState, p: &Palette) -> Vec<String> {
        let mut dump = Vec::new();
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760.0, 2600.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut acts = Vec::new();
                    show(ui, state, p, &mut acts);
                });
            });
            out.textures_delta.clear();
            dump.clear();
            for cs in &out.shapes {
                collect_text(&cs.shape, &mut dump);
            }
        }
        dump
    }

    /// 首页是「摘要在上、清单在下」的一条竖线。
    ///
    /// 旧版这条守的是「圆环在图例上面」——环和图例都删掉之后，要守的是
    /// 新的版面顺序：先看到盘的数字，再看到条目。
    #[test]
    fn summary_sits_above_the_list() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.disk = Some(Ok(molt_scan::disk::DiskUsage {
            total: 500 << 30,
            used: 320 << 30,
            available: 180 << 30,
        }));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 2200.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut acts = Vec::new();
                show(ui, &state, &Palette::light(), &mut acts);
            });
        });
        out.textures_delta.clear();

        let mut texts: Vec<(String, f32)> = Vec::new();
        collect_text_pos(&out.shapes, &mut texts);
        let y = |needle: &str| -> Option<f32> {
            texts
                .iter()
                .find(|(t, _)| t.contains(needle))
                .map(|(_, y)| *y)
        };
        let avail_y = y("可用").expect("摘要条要有「可用」");
        let item_y = y("cargo build 的输出目录").expect("清单要默认展开");
        assert!(
            avail_y < item_y,
            "摘要要在清单上面：摘要 {avail_y}，条目 {item_y}"
        );
    }

    fn collect_text_pos(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<(String, f32)>) {
        for cs in shapes {
            walk_text_pos(&cs.shape, out);
        }
    }

    fn walk_text_pos(shape: &egui::epaint::Shape, out: &mut Vec<(String, f32)>) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                out.push((t.galley.text().to_string(), t.pos.y));
            }
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    walk_text_pos(s, out);
                }
            }
            _ => {}
        }
    }

    /// 清理进行中：摘要条要写「正在清理」与百分比。
    #[test]
    fn cleaning_shows_progress_in_the_summary() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.clean_anim = 0.46;
        state.clean_total_items = 24;
        state.clean_total_bytes = 3_600_000_000;

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("正在清理")),
            "清理中摘要条要写「正在清理」：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains('%')),
            "清理中要给百分比，让用户看到它在动：{dump:#?}"
        );
        // 旧版那只圆环已经不在，摘要条不该继承它的居中大字。
        assert!(
            !dump.iter().any(|l| l.contains("还剩")),
            "摘要条不该再出现旧版的「还剩」：{dump:#?}"
        );
    }

    /// 清完那一刻：环心换成「清理完成」与腾出的量。
    #[test]
    fn finished_clean_announces_the_result() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.clean_anim = 1.0;
        state.clean_done_secs = 1.5;
        state.clean_was_delete = true;
        state.clean_total_bytes = 3_600_000_000;

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("清理完成")),
            "清完要明说完成：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("已腾出空间")),
            "直接清理要说明腾出了空间：{dump:#?}"
        );
    }

    /// 放进临时空间只是换了个位置，磁盘可用空间没变——不能写「已腾出空间」。
    #[test]
    fn quarantine_run_does_not_claim_freed_space() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.clean_anim = 1.0;
        state.clean_done_secs = 1.5;
        state.clean_was_delete = false;
        state.clean_total_bytes = 3_600_000_000;

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            !dump.iter().any(|l| l.contains("已腾出空间")),
            "放入临时空间没有释放空间，不能说腾出了空间：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("已放进临时空间")),
            "要说明东西去了临时空间：{dump:#?}"
        );
    }

    fn collect_text(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    collect_text(s, out);
                }
            }
            _ => {}
        }
    }

    fn state_with_notes() -> AppState {
        let mut state = AppState::new("/Users/me".to_string());
        let mut stale = molt_core::Candidate::new(
            "/Users/me/project/mac/write-T/target",
            molt_core::ItemKind::Dir,
            molt_core::Verdict::Costly {
                rebuild: molt_core::RebuildCost::new(
                    molt_core::CostKind::Recompile,
                    "这些都会重新编译出来",
                ),
            },
        )
        .with_size(26 << 30, 26 << 30, 100);
        stale.suggest_clean = true;
        let items = vec![
            stale,
            molt_core::Candidate::new(
                "/Users/me/Library/Caches/com.example.tool",
                molt_core::ItemKind::Dir,
                molt_core::Verdict::Reclaimable,
            )
            .with_size(1536 << 20, 1536 << 20, 10),
        ];
        state.apply_result(Arc::new(crate::worker::ScanResult {
            items,
            rule_ids: vec![],
            denied_roots: vec![],
            elapsed_secs: 12.0,
            from_cache: false,
        }));
        state.ai.notes.insert(
            "/Users/me/project/mac/write-T/target".into(),
            "cargo build 的输出目录，删掉后下次构建会重新生成".into(),
        );
        state
    }

    /// 确认框按下「确认直接清理」必须真的开始清理。
    ///
    /// 回归点：确认分支曾经推 `[CancelPendingClean, RunPendingClean]` 两条动作，
    /// 前一条把批次清空，后一条拿不到东西就静默返回——弹窗关掉、盘上没动、
    /// 界面也没有任何提示。这里不检查「push 了哪几个 Action」，
    /// 而是把确认产物喂回 `apply`，验证它真的换出了 `Effect::Clean`。
    #[test]
    fn confirming_the_clean_dialog_yields_a_real_clean_effect() {
        let mut s = state_with_notes();
        // 走真路径装批次，模拟用户按的是「直接清理…」
        crate::action::apply(&mut s, Action::PrepareClean(molt_act::CleanMode::Delete));
        let wanted = s.pending_clean.as_ref().expect("勾选项应装成批次").count();
        assert!(wanted > 0);

        let mut effects = Vec::new();
        for act in confirm_actions(crate::components::ConfirmOutcome::Confirmed) {
            effects.extend(crate::action::apply(&mut s, act));
        }

        assert_eq!(effects.len(), 1, "确认应当且只应当产生一个执行意图");
        match effects.pop().unwrap() {
            crate::action::Effect::Clean(pending) => {
                assert_eq!(pending.count(), wanted, "交给后台的应是用户确认的那批");
                assert_eq!(pending.mode, molt_act::CleanMode::Delete);
            }
            other => panic!("确认后应产生 Effect::Clean，实际是 {other:?}"),
        }
        assert!(s.pending_clean.is_none(), "批次交给后台后不该留在状态里");
    }

    /// 取消则什么都不启动，勾选也保留。
    #[test]
    fn cancelling_the_dialog_starts_nothing() {
        let mut s = state_with_notes();
        crate::action::apply(&mut s, Action::PrepareClean(molt_act::CleanMode::Delete));

        let mut effects = Vec::new();
        for act in confirm_actions(crate::components::ConfirmOutcome::Cancelled) {
            effects.extend(crate::action::apply(&mut s, act));
        }
        assert!(effects.is_empty(), "取消不该产生任何执行意图");
        assert!(s.pending_clean.is_none());
        assert!(s.checked_count() > 0, "取消确认不等于放弃勾选");
    }

    #[test]
    fn workbench_puts_the_priority_sections_on_screen() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let dump = painted_text(&ctx, &mut state, &Palette::light());

        // 摘要条回答「盘还剩多少、能清多少」——一行说清，不再占一整屏。
        assert!(
            dump.iter().any(|l| l.contains("可用")),
            "摘要条要有可用空间：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可清理")),
            "摘要条要有可清理那一档：{dump:#?}"
        );

        // 清单默认展开：分节与条目第一屏就在，不需要再点「看看都清什么」。
        assert!(
            dump.iter().any(|l| l.contains("建议清掉")),
            "必须有一档写清「建议清掉」：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可以直接清")),
            "必须有可回收那一段：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("cargo build 的输出目录")),
            "AI 解释要直接显示在条目上：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l == "AI"),
            "AI 说的话要有标记与本地判定区分：{dump:#?}"
        );

        // 主操作在底栏，且说清可还原。
        assert!(
            dump.iter().any(|l| l.contains("清理这些")),
            "底栏要有主操作：{dump:#?}"
        );
    }

    #[test]
    fn rows_show_both_the_local_verdict_and_why() {
        // 判定理由从「每行重复一遍」改成「分节标题说一次」——十一行写同一句话
        // 是噪声，不是信息。但理由本身必须还在页面上，否则用户又得靠猜。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("删了会自动重建")),
            "可回收那一段的标题要给出理由：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("重新编译")),
            "有代价那一段的标题要给出代价：{dump:#?}"
        );
    }

    #[test]
    fn summary_count_matches_what_is_actually_checked() {
        // 回归：顶部曾把「建议清理」算进「可回收」再加一次，
        // 于是写着「可以先清 3 项」而列表里只有 2 项勾着。
        let state = state_with_notes();
        let (n, _) = state.reclaimable_summary();
        let (sug, _) = state.suggested_summary();
        assert_eq!(n + sug, state.checked_count(), "顶部数字必须与勾选数一致");
    }

    #[test]
    fn stale_result_is_labelled_as_stale() {
        // 重扫失败但旧结果还在：数字保留，但必须先说清它是旧的。
        // 不说的话，用户会把 10:42 那份当成刚刚扫出来的。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.last_scan_at = Some("10:42".into());
        state.error = Some("遍历 ~/Library/Caches 时被系统拒绝".into());

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("这次扫描失败")),
            "失败要写在最上面：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("10:42")),
            "要说清下面这份是什么时候的结果：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("重试")),
            "失败必须给一个可点的下一步：{dump:#?}"
        );
    }

    #[test]
    fn freed_bytes_are_shown_once_something_was_cleaned() {
        // session_freed 一直在累加，但从没显示过；清完用户没有任何持久证据。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.session_freed = 1_240_000_000;
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("本次运行已腾出")),
            "清完要说清腾出了多少：{dump:#?}"
        );
    }

    #[test]
    fn filtered_view_always_offers_a_way_back() {
        // hero 里的两个链接会切筛选；切过去之后必须有一条回到「全部」的路。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.filter = Filter::Costly;
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("显示全部")),
            "筛到某一档后要有出口：{dump:#?}"
        );
    }

    #[test]
    fn unconfigured_ai_offers_a_way_to_configure_it() {
        // 未配置时以前只显示 resolve() 报错的最后 30 个字——
        // 用户看到的是「…API_KEY，或在配置里填写 api_key」这种半句，
        // 既不知道怎么读，也没有一条去配置页的路。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.ai.config.active_provider.clear();
        state.ai.config.active_model.clear();

        // 首页不再挂「AI 未配置」的尾巴：那不是清理的必要条件，
        // 挂在首页只会让人以为主流程被挡住了。首页必须照常给出清理入口。
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("放入临时空间")),
            "没配 AI 也要能清理：{dump:#?}"
        );
        assert!(
            !dump.iter().any(|l| l.contains("未配置")),
            "首页不该提 AI 配置：{dump:#?}"
        );

        // 但「让 AI 解释」这个入口仍然在清单里，只是点不动——
        // 用户想知道为什么点不了时，设置页有密钥输入框。
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        // 这份 state 里已经有解释，按钮因此写「重新解释」——两种文案都算入口在。
        assert!(
            dump.iter()
                .any(|l| l.contains("解释") && l.contains("AI") || l == "重新解释"),
            "清单里要保留 AI 解释入口：{dump:#?}"
        );
    }

    #[test]
    fn explained_rows_never_claim_ai_is_unconfigured() {
        // 回归：已经解释过 24 条、配置此刻读不出来时，工具条上同时显示
        // 「AI 已解释 24 条」和「AI 未配置」。两句话互相打架，
        // 用户会以为那些解释是假的。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        // 解释还在，但配置读不出来
        state.ai.config.active_provider.clear();
        state.ai.config.active_model.clear();

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("解释")),
            "已解释的条数要显示：{dump:#?}"
        );
        assert!(
            !dump.iter().any(|l| l.contains("AI 未配置")),
            "已经解释过就不能说「未配置」，两者矛盾：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("去设置")),
            "点不了的时候要给一条去配置的路：{dump:#?}"
        );
    }

    /// 详情抽屉展示一行放不下的信息：完整路径、判定理由、勾选状态。
    ///
    /// 只读：抽屉打开不该改动勾选，也不该出现执行按钮。
    #[test]
    fn detail_drawer_shows_the_full_record() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let path = "/Users/me/project/mac/write-T/target";
        state.detail_path = Some(path.into());
        let checked_before = state.is_checked(path);

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        for needle in ["路径", "大小", "判定", "本次勾选", "在访达中显示"] {
            assert!(
                dump.iter().any(|l| l.contains(needle)),
                "抽屉要写「{needle}」：{dump:#?}"
            );
        }
        assert!(
            dump.iter().any(|l| l == path),
            "抽屉要写完整路径：{dump:#?}"
        );
        assert_eq!(state.is_checked(path), checked_before, "看详情不该改变勾选");
    }

    #[test]
    fn empty_state_is_not_drawn_as_a_clean_disk() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = AppState::new("/Users/me".to_string());
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("还没有扫描结果")),
            "没扫过要说「还没有」，不能说「很干净」：{dump:#?}"
        );
        assert!(!dump.iter().any(|l| l.contains("很干净")));
    }
}
