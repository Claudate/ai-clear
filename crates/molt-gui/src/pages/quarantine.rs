//! 隔离区页：从原位置移走、还没被真正删除的东西，都在这里。
//!
//! 这一页是「移入隔离区」这条路径的出口，也是它成立的全部理由：
//! 没有地方能看、能还原、能彻底删掉，隔离区就只是个隐藏的垃圾堆。
//! 所以它作为**独立的固定入口**放在顶部标签里，而不是折叠在某个弹窗或历史页里。
//!
//! 四种状态分开画：读失败、正在读、空、有内容。
//! 「读失败」绝不能画成「空」——那会让用户以为东西已经没了。

use eframe::egui;

use crate::action::Action;
use crate::state::{AppState, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{
    badge, card, danger_ghost_button, danger_text_button, page_header, primary_button,
    secondary_button, state_block,
};

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    page_header(
        ui,
        p,
        "临时空间",
        &format!(
            "从原位置移走的内容会保留 {} 天，期间随时可以还原，也可以在这里彻底删除。",
            crate::paths::DEFAULT_RETAIN_DAYS
        ),
        |ui| {
            if secondary_button(ui, p, "刷新", !state.is_purging() && state.can_start()) {
                acts.push(Action::LoadQuarantine);
            }
        },
    );
    ui.add_space(12.0);

    // 读失败优先于「空」：这两件事长得像，但对用户的含义完全相反。
    if let Some(err) = state.quarantine_error.clone() {
        card(ui, p, |ui| {
            ui.label(
                egui::RichText::new("读不到临时空间")
                    .size(p.sz(15.0))
                    .strong()
                    .color(p.danger),
            );
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(egui::RichText::new(&err).size(p.sz(12.0)).color(p.text_dim))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(10.0);
            if primary_button(ui, p, "重试", true) {
                acts.push(Action::LoadQuarantine);
            }
        });
        return;
    }

    if state.is_purging() {
        purging_view(ui, state, p);
        return;
    }

    if state.restore_job.is_some() {
        restoring_view(ui, state, p);
        return;
    }

    if state.quarantine_loading && state.quarantine.is_empty() {
        state_block(
            ui,
            p,
            "◌",
            "正在读取临时空间…",
            "稍等一下，正在从审计库里取出待还原的内容。",
            p.reclaimable,
        );
        return;
    }

    if state.quarantine.is_empty() {
        state_block(
            ui,
            p,
            "○",
            "临时空间是空的",
            "还没有内容被移进来。在清单里选「放入临时空间」后，那一批会出现在这里，7 天内可还原。",
            p.text_faint,
        );
        return;
    }

    summary_card(ui, state, p);
    ui.add_space(10.0);

    match crate::widgets::op_report(ui, p, state.last_op.as_deref()) {
        crate::widgets::ReportAction::Dismiss => acts.push(Action::DismissReport),
        crate::widgets::ReportAction::Next(next) => acts.push(Action::FollowNextStep(next)),
        crate::widgets::ReportAction::None => {}
    }
    if state.last_op.is_some() {
        ui.add_space(10.0);
    }

    let row_h = p.sz(78.0);
    let footer_h = p.sz(92.0);
    let list_h = (ui.available_height() - footer_h).max(p.sz(140.0));

    let total = state.quarantine.len();
    let rows = state.quarantine.clone();
    let mut restore_id: Option<i64> = None;
    let mut toggle_id: Option<i64> = None;
    let mut purge_id: Option<i64> = None;

    egui::ScrollArea::vertical()
        .max_height(list_h)
        .auto_shrink([false, false])
        .show_rows(ui, row_h, total, |ui, range| {
            // 行距清零：行与行靠一条细线分隔，而不是靠空隙。
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                if let Some(row) = rows.get(i) {
                    match line(ui, state, p, row, row_h) {
                        RowAction::None => {}
                        RowAction::Toggle => toggle_id = Some(row.id),
                        RowAction::Restore => restore_id = Some(row.id),
                        RowAction::Purge => purge_id = Some(row.id),
                    }
                }
            }
        });

    if let Some(id) = toggle_id {
        acts.push(Action::ToggleQuarantineCheck(id));
    }
    if let Some(id) = restore_id {
        acts.push(Action::RestoreOne(id));
    }
    if let Some(id) = purge_id {
        acts.push(Action::PreparePurgeOne(id));
    }

    ui.add_space(8.0);
    footer(ui, state, p, acts);
}

/// 顶部汇总：还剩多少、最晚什么时候到期。
fn summary_card(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    let restorable: Vec<&crate::state::QuarantineRow> =
        state.quarantine.iter().filter(|r| r.restorable).collect();
    let bytes: u64 = restorable.iter().map(|r| r.bytes).sum();
    let broken = state.quarantine.len() - restorable.len();

    // 最近到期的那一项：这页最该先说清的就是「什么时候会开始自己没」。
    let soonest = restorable
        .iter()
        .filter_map(|r| days_left(&r.purge_after).map(|d| (d, r)))
        .min_by_key(|(d, _)| *d);

    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(molt_scan::human_bytes(bytes))
                            .size(p.sz(24.0))
                            .strong()
                            .color(p.reclaimable),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.label(
                    egui::RichText::new(format!("{} 项还能还原", restorable.len()))
                        .size(p.sz(11.5))
                        .color(p.text_dim),
                );
            });
            ui.add_space(20.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "这些内容只是被移到了别处，磁盘空间要等彻底删除后才真正释放。",
                    )
                    .size(p.sz(11.5))
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            if let Some((d, row)) = soonest {
                let when = if d > 0 {
                    format!("{d} 天后")
                } else {
                    "今天".to_string()
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "最近到期：{when} · {}",
                            crate::paths::friendly_name(
                                &state.home,
                                std::path::Path::new(&row.original)
                            )
                        ))
                        .size(p.sz(11.0))
                        .color(p.costly),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            if broken > 0 {
                ui.add_space(8.0);
                badge(
                    ui,
                    p,
                    &format!("{broken} 项内容已不在"),
                    p.costly,
                    p.costly_soft,
                );
            }
        });
    });
}

enum RowAction {
    None,
    Toggle,
    Restore,
    Purge,
}

/// 一行隔离区条目。返回用户想做的动作。
fn line(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    row: &crate::state::QuarantineRow,
    row_h: f32,
) -> RowAction {
    let busy = state.restoring == Some(row.id);
    let checked = state.is_quarantine_checked(row.id);
    let full_w = ui.available_width();
    // 右栏是按钮区（大小、还原、彻底删除）。行点击区要把它让出来，
    // 否则整行 intera​ct 会盖在按钮上——点「还原」只会勾选整行。
    let actions_w = p.sz(248.0);
    let mut action = RowAction::None;

    // 行列表而不是卡片：这一页和首页清单一样是"一列条目"，
    // 每行套一张带边框的白卡会把它们切成互不相干的纸片。
    // 选中只用底色标出，不靠边框加粗。
    let resp = egui::Frame::NONE
        .fill(if checked { p.accent_soft } else { p.surface })
        .inner_margin(egui::Margin::symmetric(14, 9))
        .show(ui, |ui| {
            ui.set_width(full_w - 30.0);
            ui.set_height(row_h - p.sz(18.0));
            ui.horizontal(|ui| {
                // 勾选只对有内容可删的行开放
                if row.restorable {
                    // actionable = 这一行有内容可还原；interactive = 现在没有任务在跑。
                    // 两者分开，因为「这一行不能选」和「此刻全局禁用」对用户是两件事。
                    crate::components::checkbox::checkbox(ui, p, checked, true, state.can_select());
                    ui.add_space(4.0);
                } else {
                    ui.add_space(p.sz(21.0));
                }

                let size_w = p.sz(84.0);
                let left_w = (ui.available_width() - size_w - actions_w).max(p.sz(120.0));
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, row_h - p.sz(18.0)),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(crate::paths::friendly_name(
                                        &state.home,
                                        std::path::Path::new(&row.original),
                                    ))
                                    .size(p.sz(12.0))
                                    .strong()
                                    .color(p.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                            if row.restorable {
                                badge(ui, p, "可还原", p.reclaimable, p.accent_soft);
                            } else {
                                badge(ui, p, "内容已不在", p.costly, p.costly_soft);
                            }
                        });
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&row.original)
                                    .size(p.sz(10.0))
                                    .monospace()
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        let note = if row.restorable {
                            match days_left(&row.purge_after) {
                                // 「还剩几天」比一个日期有用得多：用户来这页想知道的是
                                // 会不会没，而不是到期日是哪一天。
                                Some(d) if d > 0 => format!(
                                    "移入 {} · 还剩 {d} 天（{} 到期）",
                                    crate::components::time::short_time(&row.at),
                                    crate::components::time::short_time(&row.purge_after)
                                ),
                                Some(_) => format!(
                                    "移入 {} · 今天到期，之后会自动删除",
                                    crate::components::time::short_time(&row.at)
                                ),
                                None => format!(
                                    "移入 {} · 最晚保留到 {}",
                                    crate::components::time::short_time(&row.at),
                                    crate::components::time::short_time(&row.purge_after)
                                ),
                            }
                        } else {
                            "临时空间里已经没有这份内容，只能从列表里清掉记录".to_string()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(note)
                                    .size(p.sz(10.0))
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(molt_scan::human_bytes(row.bytes))
                                .size(p.sz(12.5))
                                .strong()
                                .color(if row.restorable { p.text } else { p.text_faint }),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                    ui.add_space(10.0);
                    if row.restorable {
                        // 危险动作走文字按钮：单条删除是可达的，但不该和「还原」
                        // 长得一样重——这一行的默认动作永远是还原。
                        if danger_text_button(ui, p, "彻底删除…", !busy && state.can_start())
                        {
                            action = RowAction::Purge;
                        }
                        ui.add_space(8.0);
                        let label = if busy { "还原中…" } else { "还原" };
                        // 互斥：有扫描/清理/定时任务在跑时不能开始还原——
                        // 两边同时写盘会让失败原因变得无法解释
                        if secondary_button(ui, p, label, !busy && state.can_start()) {
                            action = RowAction::Restore;
                        }
                    }
                });
            });
        })
        .response;

    ui.painter().hline(
        resp.rect.x_range(),
        resp.rect.max.y - 0.5,
        egui::Stroke::new(1.0, p.row_separator),
    );

    if row.restorable {
        let mut hit_rect = resp.rect;
        hit_rect.max.x -= actions_w;
        let hit = ui.interact(
            hit_rect,
            ui.scope_id().with(("q-row", row.id)),
            egui::Sense::click(),
        );
        if hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if hit.clicked() && matches!(action, RowAction::None) {
            action = RowAction::Toggle;
        }
    }
    action
}

fn footer(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    egui::Frame::NONE
        .fill(p.bg)
        .inner_margin(egui::Margin::symmetric(0, 6))
        .show(ui, |ui| {
            crate::widgets::hairline(ui, p);
            let n = state.quarantine_checked_count();
            // 互斥：只要有任务在跑，还原与彻底删除都要置灰并给出理由
            let enabled = n > 0 && state.can_start();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    if n == 0 {
                        ui.label(
                            egui::RichText::new("选中条目后可批量还原或彻底删除")
                                .size(p.sz(12.0))
                                .color(p.text_dim),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "已选 {n} 项 · {}",
                                    molt_scan::human_bytes(state.quarantine_checked_bytes())
                                ))
                                .size(p.sz(13.0))
                                .strong(),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        ui.label(
                            egui::RichText::new("还原会把内容放回原位置；彻底删除不可恢复")
                                .size(p.sz(10.5))
                                .color(p.text_faint),
                        );
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 最右是这一页的主操作：还原（可逆、安全）
                    if primary_button(ui, p, "还原所选", enabled) {
                        acts.push(Action::RestoreSelected);
                    }
                    ui.add_space(6.0);
                    // 危险操作紧挨在左：彻底删除，要过一次确认
                    if danger_ghost_button(ui, p, "彻底删除…", enabled) {
                        acts.push(Action::PreparePurge);
                    }
                    ui.add_space(8.0);
                    if secondary_button(ui, p, "全不选", n > 0 && state.can_select()) {
                        acts.push(Action::ClearQuarantineSelection);
                    }
                    if secondary_button(ui, p, "全选", state.can_select()) {
                        acts.push(Action::SelectAllQuarantine);
                    }
                });
            });
        });
}

/// 隔离区彻底删除的确认框。
///
/// 与清单页的确认框同一套视觉语言，但文案只讲这一件事：
/// 这些内容本来还在隔离区里，这一步之后就真的没有了。
pub fn confirm_purge_modal(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    acts: &mut Vec<Action>,
) {
    let Some(pending) = state.pending_purge.clone() else {
        return;
    };

    // 骨架交给 `components::confirm_dialog`：标题、量化、红底警示、按钮排布
    // 都与首页「直接清理」那份共用。这一页只负责说清「删的是哪几项」。
    let shown = 6;
    let rows: Vec<(String, String)> = pending
        .items
        .iter()
        .take(shown)
        .map(|it| (it.original.display().to_string(), String::new()))
        .collect();

    let outcome = crate::components::ConfirmDialog::new(
        "molt-confirm-purge",
        crate::components::ConfirmKind::Danger,
        "从临时空间彻底删除这些内容？",
    )
    .summary(format!(
        "{} 项 · 共 {}",
        pending.count(),
        molt_scan::human_bytes(pending.bytes)
    ))
    .warning(
        "这是永久删除，不可恢复。",
        "删掉之后无法再还原到原位置。空间会立即释放。",
    )
    .confirm_label("确认彻底删除")
    .width(440.0)
    .show(ui, p, |ui| {
        let rest = pending.items.len().saturating_sub(shown);
        crate::components::confirm_dialog::item_list(
            ui,
            p,
            true,
            &rows,
            (rest > 0).then(|| format!("还有 {rest} 项")),
        );
    });

    match outcome {
        crate::components::ConfirmOutcome::Confirmed => acts.push(Action::RunPendingPurge),
        crate::components::ConfirmOutcome::Cancelled => acts.push(Action::CancelPendingPurge),
        crate::components::ConfirmOutcome::Open => {}
    }
}

fn restoring_view(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    let (done, total) = match state.restore_job.as_ref().map(|j| j.peek()) {
        Some(crate::worker::RestoreProgress::Working { done, total }) => (*done, *total),
        _ => (0, 0),
    };
    ui.add_space(p.sz(40.0));
    ui.vertical_centered(|ui| {
        ui.set_max_width(p.sz(420.0));
        ui.label(egui::RichText::new("正在还原…").size(p.sz(18.0)).strong());
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("共 {} 项，会放回原位置。", total.max(done)))
                    .size(p.sz(12.0))
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(16.0);
        crate::widgets::progress_bar(
            ui,
            p,
            if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            },
            p.sz(300.0),
            "处理中",
        );
    });
}

fn purging_view(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    let (done, total) = match state.purge_job.as_ref().map(|j| j.peek()) {
        Some(crate::worker::PurgeProgress::Working { done, total }) => (*done, *total),
        _ => (0, 0),
    };
    ui.add_space(p.sz(40.0));
    ui.vertical_centered(|ui| {
        ui.set_max_width(p.sz(420.0));
        ui.label(
            egui::RichText::new("正在彻底删除…")
                .size(p.sz(18.0))
                .strong(),
        );
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("共 {} 项，删除后不可恢复。", total.max(done)))
                    .size(p.sz(12.0))
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(16.0);
        crate::widgets::progress_bar(
            ui,
            p,
            if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            },
            p.sz(300.0),
            "处理中",
        );
    });
}

/// 排空已到期的隔离项。
///
/// 同步执行而不是丢给 `PurgeJob`：启动路径上它通常只有几条、几百毫秒内完成，
/// 而异步化会让界面先画出一屏很快就要消失的条目。失败只记进状态，
/// 不阻断启动——排空不了也不影响用户自己还原或手动处理。
pub fn purge_due_on_start(state: &mut AppState) {
    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(_) => return,
    };
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), "purge");
    let exec = molt_act::Executor::new(store, q, crate::paths::DEFAULT_RETAIN_DAYS);
    let outcome = exec.purge_due();
    if outcome.deleted.is_empty() && outcome.failed.is_empty() {
        return;
    }
    // 只有真的动了东西才提示：启动时无故弹一条状态会显得工具很吵
    let text = if outcome.failed.is_empty() {
        format!(
            "已排空 {} 项到期隔离内容，释放 {}。",
            outcome.deleted.len(),
            molt_scan::human_bytes(outcome.deleted_bytes())
        )
    } else {
        format!(
            "已排空 {} 项到期内容，{} 项未清掉（内容可能在磁盘上被删了）。",
            outcome.deleted.len(),
            outcome.failed.len()
        )
    };
    state.status = Some(StatusMessage::info(text));
}

/// 从审计库读隔离区。
pub fn reload(state: &mut AppState) {
    state.quarantine_loading = true;
    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.quarantine_error = Some(format!("打开审计库失败：{e}"));
            state.quarantine.clear();
            state.quarantine_checked.clear();
            state.quarantine_loading = false;
            return;
        }
    };
    match store.list_pending(2000) {
        Ok(recs) => {
            let home = state.home.clone();
            state.quarantine = recs
                .into_iter()
                .map(|r| crate::state::QuarantineRow {
                    id: r.id,
                    original: crate::paths::display_in(&home, &r.absolute_path),
                    bytes: r.bytes,
                    at: r.performed_at.clone(),
                    purge_after: r.purge_after.clone(),
                    restorable: r.quarantine_path.exists(),
                })
                .collect();
            // 只保留仍然存在且可还原的勾选：刚删掉的那些不该继续计入「已选」
            let valid: std::collections::HashSet<i64> = state
                .quarantine
                .iter()
                .filter(|r| r.restorable)
                .map(|r| r.id)
                .collect();
            state.quarantine_checked.retain(|id| valid.contains(id));
            state.quarantine_error = None;
        }
        Err(e) => {
            state.quarantine_error = Some(format!("读取审计库失败：{e}"));
            state.quarantine.clear();
            state.quarantine_checked.clear();
        }
    }
    state.quarantine_loading = false;
}

/// 还原一条（或全部勾选）。
pub fn restore(state: &mut AppState, id: i64) {
    // 互斥：还原是写盘动作，不能和扫描/清理/定时任务并行
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    state.restoring = Some(id);
    state.status = Some(StatusMessage::info("正在还原…"));
    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.restoring = None;
            state.status = Some(StatusMessage::error(format!("打开审计库失败：{e}")));
            return;
        }
    };
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), "restore");
    let exec = molt_act::Executor::new(store, q, crate::paths::DEFAULT_RETAIN_DAYS);

    match exec.restore(id) {
        Ok(p) => {
            let shown = crate::paths::display_in(&state.home, &p);
            state.status = Some(StatusMessage::info(format!("已还原到 {shown}")));
            state.last_op = None;
        }
        Err(e) => {
            // 还原失败必须说清原因，并且保留这一条在列表里——
            // 画成「已还原」会让用户以为内容回去了。
            state.status = Some(StatusMessage::error(format!("还原失败：{e}")));
        }
    }
    state.restoring = None;
    reload(state);
    crate::pages::history::reload(state);
}

/// 批量还原给定的这些条目（后台执行）。
///
/// 只接收 id 列表，不自己从勾选里挑——挑选是 `Action::RestoreSelected`
/// 在 `apply` 里做的。这样「用户选了哪些」与「执行了什么」是同一份数据，
/// 不会在执行途中因为状态变化而偷偷改变。
pub fn restore_ids(state: &mut AppState, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.status = Some(StatusMessage::error(format!("打开审计库失败：{e}")));
            return;
        }
    };
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), "restore");
    // 交给后台：解压几十 GB 的条目不把窗口卡住，进度由 `pump_restore` 回填。
    // 任务存在期间 `activity()` 报「正在还原」，勾选与按钮自然置灰。
    state.restore_job = Some(crate::worker::RestoreJob::start(ids.to_vec(), store, q));
    state.status = Some(StatusMessage::info("正在还原…"));
}

/// 取回后台还原的结果。由主循环每帧调用。
pub fn pump_restore(state: &mut AppState) {
    let Some(job) = state.restore_job.take() else {
        return;
    };
    let mut job = job;
    let progress = job.poll().clone();
    match progress {
        crate::worker::RestoreProgress::Done(report) => {
            let failures = report.failures.len();
            let kind = if report.restored == 0 {
                crate::state::StatusKind::Error
            } else if failures > 0 {
                crate::state::StatusKind::Warn
            } else {
                crate::state::StatusKind::Info
            };
            let summary = if report.restored == 0 {
                let first = report
                    .failures
                    .first()
                    .map(|(n, r)| format!("{n}：{r}"))
                    .unwrap_or_else(|| "没有可还原的内容".to_string());
                format!("还原没有生效：{first}")
            } else if failures > 0 {
                format!(
                    "已还原 {} 项；{failures} 项未还原，原因见下方",
                    report.restored
                )
            } else {
                format!("已还原 {} 项", report.restored)
            };
            state.status = Some(StatusMessage {
                kind,
                text: summary.clone(),
            });
            state.last_op = Some(std::sync::Arc::new(crate::state::OpReport {
                summary,
                done: report.restored,
                done_bytes: 0,
                failures: report.failures.clone(),
                kind,
                // 还原之后要去的地方就是这页的原位置：没有跨页入口。
                next: None,
            }));
            state.clear_quarantine_selection();
            reload(state);
            crate::pages::history::reload(state);
        }
        _ => {
            state.restore_job = Some(job);
        }
    }
}

/// 执行隔离区彻底删除（后台）。
pub fn do_purge(state: &mut AppState) {
    if !state.can_start() {
        state.pending_purge = None;
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    let Some(pending) = state.pending_purge.clone() else {
        return;
    };
    // 只认确认框里那一批的编号。确认之后用户改了勾选也不影响执行——
    // 这行以前是重读 `quarantine_checked`，于是「确认 3 项」可能删掉 5 项。
    let ids = pending.ids();

    if ids.is_empty() {
        state.pending_purge = None;
        state.status = Some(StatusMessage::warn("没有选中可删除的内容"));
        return;
    }

    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.pending_purge = None;
            state.status = Some(StatusMessage::error(format!(
                "打开审计库失败，未执行删除：{e}"
            )));
            return;
        }
    };
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), "purge");

    state.pending_purge = None;
    state.status = Some(StatusMessage::info("正在彻底删除…"));
    state.purge_job = Some(crate::worker::PurgeJob::start(ids, store, q));
}

/// 取回后台彻底删除的结果。由主循环每帧调用。
pub fn pump_purge(state: &mut AppState) {
    let Some(job) = state.purge_job.take() else {
        return;
    };
    let mut job = job;
    let progress = job.poll().clone();
    match progress {
        crate::worker::PurgeProgress::Done(report) => {
            let failures = report.failures.len();
            let kind = if report.deleted == 0 {
                crate::state::StatusKind::Error
            } else if failures > 0 {
                crate::state::StatusKind::Warn
            } else {
                crate::state::StatusKind::Info
            };
            let summary = if report.deleted == 0 {
                let first = report
                    .failures
                    .first()
                    .map(|(p, r)| format!("{}：{r}", crate::paths::display_path_str(p)))
                    .unwrap_or_else(|| "没有可删除的内容".to_string());
                format!("彻底删除没有生效：{first}")
            } else if failures > 0 {
                format!(
                    "已从临时空间彻底删除 {} 项，释放 {}；{} 项未删除，原因见下方",
                    report.deleted,
                    molt_scan::human_bytes(report.deleted_bytes),
                    failures
                )
            } else {
                format!(
                    "已从临时空间彻底删除 {} 项，释放 {}",
                    report.deleted,
                    molt_scan::human_bytes(report.deleted_bytes)
                )
            };

            state.session_freed += report.deleted_bytes;
            state.status = Some(StatusMessage {
                kind,
                text: summary.clone(),
            });
            state.last_op = Some(std::sync::Arc::new(crate::state::OpReport {
                summary,
                done: report.deleted,
                done_bytes: report.deleted_bytes,
                failures: report.failures.clone(),
                kind,
                // 彻底删除之后唯一的痕迹在历史里。
                next: Some(crate::state::NextStep::History),
            }));
            state.clear_quarantine_selection();
            reload(state);
            crate::pages::history::reload(state);
            state.refresh_disk();
        }
        _ => {
            state.purge_job = Some(job);
        }
    }
}

/// 从 `purge_after` 算到「还剩几天」。
///
/// 只按日期（UTC）算整数天：用户问的是「明天会不会没」，
/// 精确到分钟只会让文案变长而没有用。
fn days_left(iso: &str) -> Option<i64> {
    days_until(iso, &molt_act::now_iso8601())
}

/// `days_left` 的可测版本：把「现在」也当参数传进来。
fn days_until(iso: &str, now_iso: &str) -> Option<i64> {
    Some(civil_days(iso)? - civil_days(now_iso)?)
}

/// 把 ISO 时间的前 10 个字符（`YYYY-MM-DD`）换成「距 1970-01-01 的天数」。
///
/// 自己算而不是引入日期库：整个界面只差一个减法，
/// 而 Howard Hinnant 这段公历转换是标准做法，六十行依赖换不来别的东西。
/// 解析不出来返回 `None`——调用方退回显示原日期，不编一个天数。
fn civil_days(iso: &str) -> Option<i64> {
    let bytes = iso.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let y: i64 = iso.get(0..4)?.parse().ok()?;
    let m: i64 = iso.get(5..7)?.parse().ok()?;
    let d: i64 = iso.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::QuarantineRow;

    fn row(id: i64, restorable: bool) -> QuarantineRow {
        QuarantineRow {
            id,
            original: format!("~/cache/{id}"),
            bytes: 100,
            at: "2026-10-20T10:00:00Z".into(),
            purge_after: "2026-10-27T10:00:00Z".into(),
            restorable,
        }
    }

    #[test]
    fn days_until_counts_calendar_days() {
        // 同一天 → 0；明天 → 1；已过期 → 负数
        assert_eq!(
            days_until("2026-10-25T23:00:00Z", "2026-10-25T01:00:00Z"),
            Some(0)
        );
        assert_eq!(
            days_until("2026-10-26T01:00:00Z", "2026-10-25T23:00:00Z"),
            Some(1)
        );
        assert_eq!(
            days_until("2026-10-20T10:00:00Z", "2026-10-25T10:00:00Z"),
            Some(-5)
        );
        // 跨月与跨年
        assert_eq!(
            days_until("2026-11-01T00:00:00Z", "2026-10-31T00:00:00Z"),
            Some(1)
        );
        assert_eq!(
            days_until("2027-01-01T00:00:00Z", "2026-12-31T00:00:00Z"),
            Some(1)
        );
        // 解析不出来时不编天数
        assert_eq!(days_until("不是时间", "2026-10-25T10:00:00Z"), None);
        assert_eq!(
            days_until("2026-13-01T00:00:00Z", "2026-10-25T10:00:00Z"),
            None
        );
    }

    #[test]
    fn a_missing_entry_is_not_counted_as_restorable() {
        let mut s = AppState::new("/Users/me".into());
        s.quarantine = vec![row(1, true), row(2, false)];
        s.select_all_quarantine();
        assert_eq!(s.quarantine_checked_count(), 1);
        // 失效的那条没有内容可删，不能混进待确认批次
        let p = s.pending_purge_from_checked().unwrap();
        assert_eq!(p.count(), 1);
    }
    /// 行尾的「还原」必须点得到。
    ///
    /// 整行点击区曾经盖住右栏按钮：点「还原」只会把整行勾上。
    /// 这条按真正画出来的文字位置点，布局再改也能抓住同一件事。
    #[test]
    fn clicking_restore_in_a_row_asks_to_restore_that_row() {
        fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::epaint::Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos)),
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }

        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let p = crate::theme::Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);
        let base = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1120.0, 900.0),
            )),
            ..Default::default()
        };

        let mut state = AppState::new("/Users/me".into());
        state.quarantine = vec![row(1, true), row(2, true)];
        let mut acts: Vec<Action> = Vec::new();

        // 第一帧把页面画出来，拿到「还原」两个字的位置。
        let mut texts: Vec<(String, egui::Pos2)> = Vec::new();
        for _ in 0..2 {
            texts.clear();
            let mut out = ctx.run_ui(base.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, &state, &p, &mut acts);
                });
            });
            out.textures_delta.clear();
            for cs in &out.shapes {
                walk(&cs.shape, &mut texts);
            }
        }
        let (_, pos) = texts
            .iter()
            .find(|(t, _)| t.trim() == "还原")
            .expect("行尾必须有「还原」按钮");
        // `pos` 是文字左上角，往右下挪一点落到按钮内部。
        let click = egui::Pos2::new(pos.x + p.sz(6.0), pos.y + p.sz(6.0));

        acts.clear();
        let mut down = base.clone();
        down.events = vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ];
        let mut up = base.clone();
        up.events = vec![egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }];
        for input in [down, up] {
            let mut out = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, &state, &p, &mut acts);
                });
            });
            out.textures_delta.clear();
        }

        assert!(
            acts.iter().any(|a| matches!(a, Action::RestoreOne(1))),
            "点行尾「还原」应请求还原第 1 行，实际动作：{acts:?}"
        );
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Action::ToggleQuarantineCheck(_))),
            "点按钮不该被整行点击区抢走，变成勾选整行"
        );
    }
}
