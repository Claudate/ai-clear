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

use crate::state::{AppState, PendingItem, Priority, StatusKind, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{
    ai_note, badge, busy_bar, card, danger_button, danger_ghost_button, page_header,
    primary_button, progress_bar, secondary_button, section_header, state_block,
};
use molt_act::CleanMode;

pub fn show(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(ui, state, p);
            ui.add_space(12.0);

            // 出错优先于「空」：读失败与「盘上很干净」在版面上长得像，
            // 但含义完全相反，绝不能画成一个。
            if let Some(err) = state.error.clone() {
                if state.result.is_none() {
                    failed_card(ui, state, p, &err);
                    return;
                }
            }

            let Some(result) = state.result.clone() else {
                if state.is_scanning() {
                    scanning_card(ui, state, p);
                } else {
                    state_block(
                        ui,
                        p,
                        "○",
                        "还没有扫描结果",
                        "点右上角「扫描」开始。扫描只读取文件信息，不会改动任何东西。",
                        p.text_faint,
                    );
                }
                return;
            };

            if result.items.is_empty() {
                state_block(
                    ui,
                    p,
                    "✓",
                    "没有找到可清理的内容",
                    "规则覆盖的目录目前都是空的，或者这些工具还没在这台 Mac 上产生缓存。",
                    p.reclaimable,
                );
                return;
            }

            hero(ui, state, p);
            ui.add_space(12.0);
            ai_bar(ui, state, p);
            ui.add_space(6.0);
            report_banner(ui, state, p);
            list(ui, state, p);
            ui.add_space(10.0);
            footer(ui, state, p);
        });
}

fn header(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    page_header(
        ui,
        p,
        "清理",
        "看看这台 Mac 上哪些能清、哪些该留。扫描只读，动手前一定会再问一次。",
        |ui| {
            // 扫描按钮在忙碌时置灰并说明理由：点了没反应是最糟的反馈
            let label = if state.is_scanning() {
                "扫描中…"
            } else {
                "重新扫描"
            };
            if secondary_button(ui, p, label, state.can_start()) {
                start_scan(state);
            }
        },
    );
}

/// 顶部结论：不是「有多少」，而是「现在该点哪个按钮」。
fn hero(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let (sug_n, sug_bytes) = state.suggested_summary();
    let (safe_n, safe_bytes) = state.reclaimable_summary();
    let costly = state.result.as_ref().map(|r| r.costly_bytes()).unwrap_or(0);
    let protected = state.result.as_ref().map(|r| r.protected_bytes()).unwrap_or(0);

    card(ui, p, |ui| {
        // 一行紧凑摘要，而不是三块大数字：这一页的主线是「现在点哪个按钮」，
        // 三个统计块会把结论和按钮挤到首屏之外，用户反而要多滚一屏才看到该做什么。
        ui.horizontal_wrapped(|ui| {
            let chip = |ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(value)
                                .size(p.sz(19.0))
                                .strong()
                                .color(color),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(label).size(p.sz(10.5)).color(p.text_faint),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
            };
            chip(
                ui,
                &format!("建议清理 {sug_n} + 可回收 {safe_n}"),
                molt_scan::human_bytes(sug_bytes + safe_bytes),
                p.reclaimable,
            );
            ui.add_space(22.0);
            chip(
                ui,
                "要确认的有代价项",
                molt_scan::human_bytes(costly),
                p.costly,
            );
            ui.add_space(22.0);
            chip(ui, "建议保留", molt_scan::human_bytes(protected), p.protected);
        });

        ui.add_space(10.0);
        crate::widgets::hairline(ui, p);

        // 结论 + 主按钮
        let busy = state.activity();
        if sug_n + safe_n > 0 {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "可以先清 {} 项、共 {}",
                        sug_n + safe_n,
                        molt_scan::human_bytes(sug_bytes + safe_bytes)
                    ))
                    .size(p.sz(18.0))
                    .strong(),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "这些删了会自动重建，或用处已经很小。默认已帮你勾好，点下面按钮会先让你确认。",
                    )
                    .size(p.sz(12.0))
                    .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if primary_button(ui, p, "清理这些（可还原）", state.can_start()) {
                    // 用当前默认勾选（建议清理 + 可回收）。不再在这里重新选一遍：
                    // 用户可能已经手动取消过其中几条，重选会把他的选择悄悄改回来。
                    state.pending_clean = state.pending_from_checked(CleanMode::Quarantine);
                }
                if busy.is_busy() {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(busy.blocking_hint())
                                .size(p.sz(10.5))
                                .color(p.text_faint),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
            });
        } else {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("没有可以直接清理的缓存")
                        .size(p.sz(17.0))
                        .strong(),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(if costly > 0 {
                        format!(
                            "还有 {} 属于「能腾但要重建」，需要你逐条看过再决定。",
                            molt_scan::human_bytes(costly)
                        )
                    } else {
                        "扫到的都是你自己的记录，这个工具不会动它们。".to_string()
                    })
                    .size(p.sz(12.0))
                    .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
    });
}

/// AI 那一条：不再是独立页面，就是主线上的一个按钮与一行状态。
fn ai_bar(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("AI 解释").size(p.sz(13.5)).strong());

            if state.ai.is_running() {
                let stage = match state.ai.job.as_ref().map(|j| j.peek()) {
                    Some(crate::ai_worker::AiProgress::Stage(
                        crate::ai_worker::AiStage::Calling { .. },
                    )) => "正在请求模型…",
                    Some(crate::ai_worker::AiProgress::Stage(
                        crate::ai_worker::AiStage::Building,
                    )) => "正在核对方案…",
                    _ => "正在整理摘要…",
                };
                badge(ui, p, stage, p.costly, p.costly_soft);
                ui.add_space(6.0);
                busy_bar(ui, p, p.sz(140.0));
            } else if state.ai.has_notes() {
                let n = state.ai.notes.len();
                badge(
                    ui,
                    p,
                    &format!("已解释 {n} 条"),
                    p.reclaimable,
                    p.accent_soft,
                );
                if let Some(m) = &state.ai.last_model {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("由 {m} 给出"))
                                .size(p.sz(10.5))
                                .color(p.text_faint),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                }
            } else if let Some(f) = &state.ai.failure {
                badge(ui, p, "解释失败", p.danger, p.danger_soft);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(crate::app::short(&f.message, 48))
                            .size(p.sz(10.5))
                            .color(p.danger),
                    )
                    .wrap_mode(egui::TextWrapMode::Truncate),
                );
            } else {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("还没让 AI 看过这次扫描")
                            .size(p.sz(11.0))
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resolved = state.ai.resolve();
                let label = if state.ai.has_notes() {
                    "重新解释"
                } else {
                    "让 AI 解释"
                };
                let enabled = state.can_start() && resolved.is_ok();
                if secondary_button(ui, p, label, enabled) {
                    start_analysis(state);
                }
                if let Err(reason) = &resolved {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(crate::app::short(reason, 40))
                                .size(p.sz(10.0))
                                .color(p.text_faint),
                        )
                        .wrap_mode(egui::TextWrapMode::Truncate),
                    );
                }
            });
        });

        // 失败要说全原因，并给出「去设置」的路——那一页才有密钥与模型。
        if let Some(f) = state.ai.failure.clone() {
            if !state.ai.is_running() {
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("这件事不影响本地清理：{}", f.message))
                            .size(p.sz(11.0))
                            .color(p.danger),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                if f.fixable_by_config && secondary_button(ui, p, "去设置检查 AI 配置", true) {
                    state.page = crate::state::Page::Settings;
                }
            }
        }

        if state.ai.has_notes() {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "模型只给解释与建议；能不能动由本地规则判定，受保护的内容永远不会被移动。",
                    )
                    .size(p.sz(10.5))
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
    });
}

fn report_banner(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    if crate::widgets::op_report(ui, p, state.last_op.as_deref()) {
        state.last_op = None;
    }
    if state.last_op.is_some() {
        ui.add_space(10.0);
    }
}

/// 按优先级分节列出数据。
fn list(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let Some(result) = state.result.clone() else {
        return;
    };
    let busy = state.activity();

    // 搜索框：几百项里找一条路径靠滚是不现实的
    ui.horizontal_wrapped(|ui| {
        let edit = egui::TextEdit::singleline(&mut state.query)
            .hint_text(
                egui::RichText::new("搜索名字或路径")
                    .size(p.sz(12.0))
                    .color(p.text_faint),
            )
            .desired_width(220.0 * p.scale)
            .margin(egui::Margin::symmetric(8, 6));
        ui.add(edit);
        if !state.query.is_empty() && secondary_button(ui, p, "清空", true) {
            state.query.clear();
        }
        if busy.is_busy() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(busy.blocking_hint())
                        .size(p.sz(10.5))
                        .color(p.costly),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        }
    });
    ui.add_space(8.0);

    let visible = state.visible_items();
    if visible.is_empty() {
        // 「全部受保护」是一种真实且常见的结果（用户的记录、密钥、占用中的文件），
        // 不能画成「这一档里没有内容」——那会让用户以为扫描什么都没找到，
        // 而实际上有一大堆内容正被明确建议保留。
        let kept = state.count_of(Priority::Keep);
        if kept > 0 && state.query.is_empty() {
            let bytes: u64 = result
                .items
                .iter()
                .filter(|c| state.priority_of(c) == Priority::Keep)
                .map(|c| c.alloc_bytes.max(c.bytes))
                .sum();
            card(ui, p, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("这次扫到的内容都建议保留")
                            .size(p.sz(15.0))
                            .strong(),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                ui.add_space(3.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} 项、共 {}：都是你的记录、密钥或正在被使用的内容，这个工具不会移动它们。",
                            kept,
                            molt_scan::human_bytes(bytes)
                        ))
                        .size(p.sz(12.0))
                        .color(p.text_dim),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                ui.add_space(10.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("下面就是这一批，标了「不会动」的都不参与清理。")
                            .size(p.sz(11.0))
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            });

            // 就地列出，不跳页：用户要的是「它们是什么」，不是换一页再看一遍。
            // 这一批不可勾选，所以不走 `row` 的点击路径，只做展示。
            let shown: Vec<usize> = (0..result.items.len())
                .filter(|&i| state.priority_of(&result.items[i]) == Priority::Keep)
                .collect();
            ui.add_space(8.0);
            section_header(ui, p, "建议保留", "不会被移动", kept, bytes, p.protected);
            for i in &shown {
                let c = &result.items[*i];
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(crate::paths::friendly_name(&state.home, &c.path))
                                .size(p.sz(12.0))
                                .color(p.text),
                        )
                        .wrap_mode(egui::TextWrapMode::Truncate),
                    );
                    badge(ui, p, "不会动", p.protected, p.surface_alt);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(molt_scan::human_bytes(
                                c.alloc_bytes.max(c.bytes),
                            ))
                            .size(p.sz(11.0))
                            .color(p.text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(c.verdict.explanation())
                            .size(p.sz(10.5))
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                if let Some(n) = state.ai_note(&c.path.display().to_string()) {
                    ai_note(ui, p, n);
                }
                ui.add_space(4.0);
            }
            return;
        }

        state_block(
            ui,
            p,
            "○",
            if state.query.is_empty() {
                "这一档里没有内容"
            } else {
                "没有匹配的内容"
            },
            "换个关键词，或者清掉搜索框看全部。",
            p.text_faint,
        );
        if !state.query.is_empty() && secondary_button(ui, p, "清空搜索", true) {
            state.query.clear();
        }
        return;
    }

    // 按档分组，每组一个分节标题。用户读到的顺序就是动手的顺序。
    let groups = [
        (Priority::Suggested, "建议清理", "先动这一批", p.costly),
        (Priority::Safe, "可以安全清理", "删了会自动重建", p.reclaimable),
        (Priority::Costly, "要你确认", "能腾空间，但要重建", p.costly),
        (Priority::Keep, "建议保留", "不会被移动", p.protected),
    ];

    let mut toggle: Option<String> = None;
    for (pr, title, hint, tint) in groups {
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

        section_header(ui, p, title, hint, rows.len(), bytes, tint);
        for i in &rows {
            if row(ui, state, p, &result, *i) {
                toggle = Some(result.items[*i].path.display().to_string());
            }
        }
        ui.add_space(8.0);
    }

    if let Some(path) = toggle {
        state.toggle(&path);
    }
}

/// 一行数据。返回是否被点击（由调用方统一改状态，避免同帧多次借用）。
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
) -> bool {
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
    // 有 AI 解释时行要更高，否则解释会被压成一行截断，等于没给
    let min_h = if note.is_some() {
        p.sz(96.0)
    } else {
        p.sz(76.0)
    };

    let frame = egui::Frame::NONE
        .fill(if checked { p.accent_soft } else { p.surface })
        .stroke(egui::Stroke::new(
            if checked { 1.5 } else { 1.0 },
            if checked {
                p.reclaimable.gamma_multiply(0.5)
            } else {
                p.border
            },
        ))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL))
        .inner_margin(egui::Margin::symmetric(14, 10));

    let resp = frame
        .show(ui, |ui| {
            ui.set_width(full_w - 30.0);
            ui.set_min_height(min_h - p.sz(20.0));
            ui.horizontal(|ui| {
                checkbox(ui, p, checked, actionable, state.can_select());
                ui.add_space(4.0);

                let size_w = p.sz(92.0);
                let left_w = (ui.available_width() - size_w - 12.0).max(p.sz(160.0));
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, min_h - p.sz(20.0)),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
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
                        });
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(crate::paths::display_in(&state.home, &c.path))
                                    .size(p.sz(10.0))
                                    .monospace()
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        // 本地判定：为什么这样判
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(c.verdict.explanation())
                                    .size(p.sz(11.0))
                                    .color(color),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        // AI 解释：它是什么。直接挂在这一行上。
                        if let Some(n) = note {
                            ai_note(ui, p, n);
                        }
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(size_w, min_h - p.sz(20.0)),
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
                        },
                    );
                });
            });
        })
        .response;

    if actionable {
        let hit = ui.interact(
            resp.rect,
            ui.scope_id().with(("wb-row", i)),
            egui::Sense::click(),
        );
        if selectable && hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        return selectable && hit.clicked();
    }
    false
}

/// 自绘勾选框。`enabled = false` 时画成实心浅灰，
/// 让「现在不能选」与「这一项不能选」区分开。
fn checkbox(ui: &mut egui::Ui, p: &Palette, checked: bool, actionable: bool, interactive: bool) {
    let d = p.sz(17.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
    let r = egui::CornerRadius::same(4);
    if checked {
        ui.painter().rect_filled(rect, r, p.reclaimable);
        let c = rect.center();
        let s = p.sz(3.6);
        ui.painter().line_segment(
            [c + egui::vec2(-s, 0.0), c + egui::vec2(-s * 0.2, s * 0.8)],
            egui::Stroke::new(1.7, p.on_accent()),
        );
        ui.painter().line_segment(
            [
                c + egui::vec2(-s * 0.2, s * 0.8),
                c + egui::vec2(s, -s * 0.7),
            ],
            egui::Stroke::new(1.7, p.on_accent()),
        );
    } else {
        let stroke = if !actionable {
            p.border
        } else if interactive {
            p.border_strong
        } else {
            p.border.gamma_multiply(0.6)
        };
        ui.painter().rect_stroke(
            rect,
            r,
            egui::Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        if !actionable {
            // 一道斜杠：这条路不存在，不是「现在不能点」
            ui.painter().line_segment(
                [rect.left_bottom() + egui::vec2(3.0, -3.0), rect.right_top() - egui::vec2(3.0, 3.0)],
                egui::Stroke::new(1.0, p.border),
            );
        }
    }
}

/// 底部操作条：主操作在最右，危险操作紧邻其左。
fn footer(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    egui::Frame::NONE
        .fill(p.bg)
        .inner_margin(egui::Margin::symmetric(0, 6))
        .show(ui, |ui| {
            crate::widgets::hairline(ui, p);
            let n = state.checked_count();
            let enabled = n > 0 && state.can_start();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    if n == 0 {
                        ui.label(
                            egui::RichText::new("还没有选中任何项")
                                .size(p.sz(12.0))
                                .color(p.text_dim),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "已选 {n} 项 · {}",
                                    molt_scan::human_bytes(state.checked_bytes())
                                ))
                                .size(p.sz(13.0))
                                .strong(),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        ui.label(
                            egui::RichText::new("移入隔离区可还原；直接清理不可恢复")
                                .size(p.sz(10.5))
                                .color(p.text_faint),
                        );
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 主操作：可还原
                    if primary_button(ui, p, "移入隔离区", enabled) {
                        state.pending_clean = state.pending_from_checked(CleanMode::Quarantine);
                    }
                    ui.add_space(6.0);
                    // 危险操作：不可恢复，红色描边，后面还有一道确认
                    if danger_ghost_button(ui, p, "直接清理…", enabled) {
                        state.pending_clean = state.pending_from_checked(CleanMode::Delete);
                    }
                    ui.add_space(8.0);
                    if secondary_button(ui, p, "全不选", n > 0 && state.can_select()) {
                        state.clear_selection();
                    }
                    if secondary_button(ui, p, "全选本页", state.can_select()) {
                        state.select_all_visible();
                    }
                });
            });

            if state.activity().is_busy() {
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{}——所有会冲突的操作都暂时停用，完成后自动恢复。",
                            state.activity().blocking_hint()
                        ))
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

fn failed_card(ui: &mut egui::Ui, state: &mut AppState, p: &Palette, err: &str) {
    card(ui, p, |ui| {
        ui.label(
            egui::RichText::new("扫描没有完成")
                .size(p.sz(15.0))
                .strong()
                .color(p.danger),
        );
        ui.add_space(3.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(err)
                    .size(p.sz(12.0))
                    .color(p.text_dim),
            )
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
            start_scan(state);
        }
    });
}

/// 确认框。清理与隔离区彻底删除共用同一套视觉语言，文案只讲这一次的后果。
pub fn confirm_modal(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let Some(pending) = state.pending_clean.clone() else {
        return;
    };
    let mut close = false;
    let mut confirm = false;

    let is_delete = pending.mode == CleanMode::Delete;
    let title = if is_delete {
        "永久删除这些内容？"
    } else {
        "把这些内容移入隔离区？"
    };
    let accent = if is_delete { p.danger } else { p.reclaimable };

    egui::Modal::new(egui::Id::unique("molt-confirm-clean"))
        .backdrop_color(egui::Color32::from_black_alpha(80))
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0, if is_delete { accent } else { p.border }))
                .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
                .inner_margin(egui::Margin::same(24)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(p.sz(480.0).min(ui.available_width()));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(title)
                        .size(p.sz(17.0))
                        .strong()
                        .color(if is_delete { accent } else { p.text }),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{} 项", pending.count()))
                            .size(p.sz(22.0))
                            .strong()
                            .color(accent),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("共 {}", molt_scan::human_bytes(pending.bytes)))
                            .size(p.sz(22.0))
                            .strong(),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
            ui.add_space(10.0);

            if is_delete {
                egui::Frame::NONE
                    .fill(p.danger_soft)
                    .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL))
                    .inner_margin(egui::Margin::symmetric(12, 9))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("这是永久删除，不可恢复。")
                                    .size(p.sz(12.5))
                                    .strong()
                                    .color(p.danger),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        ui.add_space(3.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "内容不会被放进隔离区，删掉之后没有任何找回的途径。",
                                )
                                .size(p.sz(11.5))
                                .color(p.text_dim),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                    });
            } else {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "隔离区保留 {} 天，期间随时可以还原。可用空间不会立刻变多——文件只是被移走了。",
                            crate::paths::DEFAULT_RETAIN_DAYS
                        ))
                        .size(p.sz(11.5))
                        .color(p.text_dim),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            ui.add_space(10.0);

            object_list(ui, p, &pending.items, state, is_delete);

            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let label = if is_delete {
                    "确认永久删除"
                } else {
                    "确认移入隔离区"
                };
                let clicked = if is_delete {
                    danger_button(ui, p, label, true)
                } else {
                    primary_button(ui, p, label, true)
                };
                if clicked {
                    confirm = true;
                }
                if secondary_button(ui, p, "取消", true) {
                    close = true;
                }
            });
        });

    if confirm {
        do_clean(state);
    } else if close {
        state.pending_clean = None;
    }
}

/// 确认框里的对象清单。只给数字是不够的——用户有权在不可撤销之前看见选了什么，
/// 以及 AI 对每一条的解释。
fn object_list(
    ui: &mut egui::Ui,
    p: &Palette,
    items: &[PendingItem],
    state: &AppState,
    danger: bool,
) {
    const SHOWN: usize = 5;
    egui::Frame::NONE
        .fill(p.surface_alt)
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for item in items.iter().take(SHOWN) {
                ui.horizontal_top(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(4.0, p.sz(12.0)), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(2),
                        if danger { p.danger } else { p.reclaimable },
                    );
                    ui.add_space(7.0);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(crate::paths::display_in(
                                    &state.home,
                                    &item.path,
                                ))
                                .size(p.sz(11.0))
                                .monospace()
                                .color(p.text_dim),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        if let Some(n) = &item.note {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(n)
                                        .size(p.sz(10.0))
                                        .color(p.text_faint),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        }
                    });
                });
                ui.add_space(3.0);
            }
            if items.len() > SHOWN {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "还有 {} 项，共 {}",
                            items.len() - SHOWN,
                            molt_scan::human_bytes(items.iter().skip(SHOWN).map(|i| i.bytes).sum())
                        ))
                        .size(p.sz(10.5))
                        .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
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
pub fn do_clean(state: &mut AppState) {
    let Some(result) = state.result.clone() else {
        state.pending_clean = None;
        return;
    };
    let Some(pending) = state.pending_clean.clone() else {
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
    state.status = Some(StatusMessage::info("正在清理…"));
    state.clean_job = Some(crate::worker::CleanJob::start(
        selected,
        store,
        q,
        pending.mode,
        crate::paths::DEFAULT_RETAIN_DAYS,
    ));
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
            // 清完立刻重扫：清单里现在应该是清理后的真实状态
            crate::pages::workbench::start_scan(state);
            crate::pages::quarantine::reload(state);
            crate::pages::history::reload(state);
            state.refresh_disk();
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
            "已移入隔离区 {} 项，共 {}（{} 天内可还原）",
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
        };
    }

    if failed > 0 {
        crate::state::OpReport {
            summary: format!("{summary}；{failed} 项未处理，原因见下方"),
            done,
            done_bytes,
            failures: report.failures.clone(),
            kind: StatusKind::Warn,
        }
    } else {
        crate::state::OpReport {
            summary,
            done,
            done_bytes,
            failures: Vec::new(),
            kind: StatusKind::Info,
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
                    .filter(|c| matches!(state.priority_of(c), Priority::Safe | Priority::Suggested))
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
            state.apply_result(result);
            state.refresh_disk();
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
                    show(ui, state, p);
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
        }));
        state.ai.notes.insert(
            "/Users/me/project/mac/write-T/target".into(),
            "cargo build 的输出目录，删掉后下次构建会重新生成".into(),
        );
        state
    }

    #[test]
    fn workbench_puts_the_priority_sections_on_screen() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let dump = painted_text(&ctx, &mut state, &Palette::light());

        assert!(
            dump.iter().any(|l| l.contains("建议清理")),
            "必须有一档写清「建议清理」：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可以安全清理")),
            "必须有可回收那一段：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可以先清")),
            "顶部要给出「现在可以做什么」：{dump:#?}"
        );
        // AI 解释必须挂在条目上，而不是藏在另一页
        assert!(
            dump.iter().any(|l| l.contains("cargo build 的输出目录")),
            "AI 解释要直接显示在条目上：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l == "AI"),
            "AI 说的话要有标记与本地判定区分：{dump:#?}"
        );
    }

    #[test]
    fn rows_show_both_the_local_verdict_and_why() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("重新编译")),
            "本地判定理由要留下：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("删掉后会自动重建")),
            "可回收项也要给出理由：{dump:#?}"
        );
    }

    #[test]
    fn hero_count_matches_what_is_actually_checked() {
        // 回归：顶部曾把「建议清理」算进「可回收」再加一次，
        // 于是写着「可以先清 3 项」而列表里只有 2 项勾着。
        let state = state_with_notes();
        let (n, _) = state.reclaimable_summary();
        let (sug, _) = state.suggested_summary();
        assert_eq!(n + sug, state.checked_count(), "顶部数字必须与勾选数一致");
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
