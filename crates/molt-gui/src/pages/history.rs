//! 历史页：发生过什么。**只读**时间线。
//!
//! 这一页以前同时承担了「隔离区」的职责（列待还原 + 还原按钮），
//! 于是同一批东西在这里能操作、在别处找不到，用户分不清哪一页才是「我的文件」。
//! 现在职责分开了：
//!
//! - 隔离区：还能拿回来的内容，在那里还原或彻底删除
//! - 历史：每条操作事件的时间线，回答「谁在什么时候动了什么」
//!
//! 四种状态都要分开画：空、读失败、正常、大量条目。
//! 「读失败」绝不能被画成「空」。

use eframe::egui;

use crate::action::Action;
use crate::state::{AppState, HistoryAction, HistoryFilter, HistoryRow};
use crate::theme::Palette;
use crate::widgets::{badge, card, page_header, primary_button, secondary_button, state_block};

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    page_header(
        ui,
        p,
        "历史",
        "每一次清理都留一条记录：放入临时空间、还原、彻底删除各自一行。",
        |ui| {
            if secondary_button(ui, p, "刷新", true) {
                acts.push(Action::LoadHistory);
            }
        },
    );
    ui.add_space(12.0);

    // 读失败优先于「空」：这两件事长得像，但对用户的含义完全相反。
    if let Some(err) = state.history_error.clone() {
        card(ui, p, |ui| {
            ui.label(
                egui::RichText::new("读不到历史记录")
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
                acts.push(Action::LoadHistory);
            }
        });
        return;
    }

    if state.history.is_empty() {
        state_block(
            ui,
            p,
            "○",
            "还没有操作记录",
            "清理、还原、彻底删除都会在这里留下一条。现在还是空的。",
            p.text_faint,
        );
        return;
    }

    // 汇总：已经永久消失了多少。用户打开这页常常是想确认「到底删掉过什么」。
    let purged: Vec<&HistoryRow> = state
        .history
        .iter()
        .filter(|r| matches!(r.action, HistoryAction::Purged | HistoryAction::Deleted))
        .collect();
    let purged_bytes: u64 = purged.iter().map(|r| r.bytes).sum();
    let quarantined = state
        .history
        .iter()
        .filter(|r| r.action == HistoryAction::Quarantined)
        .count();

    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{} 条记录", state.history.len()))
                            .size(p.sz(22.0))
                            .strong(),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{} 项曾放入临时空间 · 累计永久删除 {}",
                        quarantined,
                        molt_scan::human_bytes(purged_bytes)
                    ))
                    .size(p.sz(11.5))
                    .color(p.text_dim),
                );
            });
            ui.add_space(20.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new("要还原或从临时空间彻底删除，请到「临时空间」页。")
                        .size(p.sz(11.5))
                        .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(8.0);
            // 文案已经指明了去哪，那就把这条路做成能点的：让用户读完还得
            // 自己回左侧导航找一遍，等于把一句话变成一次操作负担。
            if secondary_button(ui, p, "去临时空间", true) {
                acts.push(Action::GoPage(crate::state::Page::Quarantine));
            }
        });
    });
    ui.add_space(10.0);

    // 筛选：历史会一直累积，用户来这页几乎总是在找一个具体的类别。
    // 计数直接给在分段上，不点也能看出「永久删了多少次」。
    let n_all = state.history.len();
    let n_q = state
        .history
        .iter()
        .filter(|r| r.action == HistoryAction::Quarantined)
        .count();
    let n_gone = state
        .history
        .iter()
        .filter(|r| matches!(r.action, HistoryAction::Purged | HistoryAction::Deleted))
        .count();
    let n_back = state
        .history
        .iter()
        .filter(|r| r.action == HistoryAction::Restored)
        .count();

    let opts = [
        (format!("全部 {n_all}"), HistoryFilter::All),
        (format!("放入临时空间 {n_q}"), HistoryFilter::Quarantined),
        (format!("永久删除 {n_gone}"), HistoryFilter::Gone),
        (format!("已还原 {n_back}"), HistoryFilter::Restored),
    ];
    let refs: Vec<(&str, bool)> = opts
        .iter()
        .map(|(t, f)| (t.as_str(), state.history_filter == *f))
        .collect();
    if let Some(i) = crate::widgets::segmented(ui, p, &refs) {
        acts.push(Action::SetHistoryFilter(opts[i].1));
    }
    ui.add_space(10.0);

    // 虚拟滚动：历史会一直累积，全量绘制会掉帧。
    // 行数按筛选后算，否则滚到底后会空出一大片。
    let rows: Vec<HistoryRow> = state
        .history
        .iter()
        .filter(|r| state.history_filter.keeps(r.action))
        .cloned()
        .collect();

    if rows.is_empty() {
        // 两档空态：整页没记录，与「这一档没记录」完全是两回事。
        state_block(
            ui,
            p,
            "○",
            "这一类还没有记录",
            "换到「全部」看看，或者去做一次清理。",
            p.text_faint,
        );
        return;
    }

    let row_h = p.sz(66.0);
    let total = rows.len();
    let loaded = state.history.len();
    // 留出页脚那一行：载入有上限，用户得知道「看到的不是全部」。
    let list_h = (ui.available_height() - p.sz(26.0)).max(p.sz(160.0));

    egui::ScrollArea::vertical()
        .max_height(list_h)
        // 滚动位置必须跟着筛选走：不加这个 salt 时，从「全部 200 条」滚到中间
        // 再切到「已还原 2 条」，滚动偏移会原样留着，列表在那一档里已经被
        // clamp 到顶，用户看到的是凭空多出来的一片空白。
        .id_salt(("history-list", state.history_filter))
        .auto_shrink([false, false])
        .show_rows(ui, row_h, total, |ui, range| {
            // 行距清零：时间线靠一条细线分隔，行高本身已经是留白。
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                if let Some(row) = rows.get(i) {
                    line(ui, state, p, row);
                }
            }
        });

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(format!(
            "仅显示最近载入的 {loaded} 条记录，更早的不在列表里。"
        ))
        .size(p.sz(10.5))
        .color(p.text_faint),
    );
}

/// 一行历史事件。没有操作按钮——这一页只回答「发生过什么」。
fn line(ui: &mut egui::Ui, state: &AppState, p: &Palette, row: &HistoryRow) {
    let (tint, soft) = match row.action {
        HistoryAction::Quarantined => (p.reclaimable, p.accent_soft),
        HistoryAction::Restored => (p.text_dim, p.surface_alt),
        HistoryAction::Purged => (p.danger, p.danger_soft),
        HistoryAction::Deleted => (p.danger, p.danger_soft),
    };
    let full_w = ui.available_width();

    let resp = egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(full_w - 20.0);
            ui.horizontal(|ui| {
                let size_w = p.sz(84.0);
                let left_w = (ui.available_width() - size_w - 12.0).max(p.sz(140.0));

                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, p.sz(48.0)),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            badge(ui, p, row.action.label(), tint, soft);
                            ui.add_space(4.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(crate::paths::friendly_name(
                                        &state.home,
                                        std::path::Path::new(&row.path),
                                    ))
                                    .size(p.sz(12.0))
                                    .strong()
                                    .color(p.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                        });
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&row.path)
                                    .size(p.sz(10.0))
                                    .monospace()
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        ui.horizontal(|ui| {
                            // 来源：有人看着做的，还是定时任务自己做的。
                            match row.source {
                                molt_act::CleanSource::Auto => {
                                    badge(ui, p, "自动", p.costly, p.costly_soft)
                                }
                                molt_act::CleanSource::Manual => {
                                    badge(ui, p, "手动", p.text_dim, p.surface_alt)
                                }
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(crate::components::time::short_time(
                                        &row.at,
                                    ))
                                    .size(p.sz(10.0))
                                    .color(p.text_faint),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        });
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(molt_scan::human_bytes(row.bytes))
                                .size(p.sz(12.5))
                                .strong()
                                .color(
                                    if matches!(
                                        row.action,
                                        HistoryAction::Purged | HistoryAction::Deleted
                                    ) {
                                        p.text_dim
                                    } else {
                                        p.text
                                    },
                                ),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
            });
        })
        .response;

    // 时间线靠一条细线分隔行，不用卡片边框：一页记录是一整条流，
    // 每行一张白卡会把它们切成互不相干的条目。
    ui.painter().hline(
        resp.rect.x_range(),
        resp.rect.max.y - 0.5,
        egui::Stroke::new(1.0, p.row_separator),
    );
}

pub fn reload(state: &mut AppState) {
    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.history_error = Some(format!("打开审计库失败：{e}"));
            state.history.clear();
            return;
        }
    };
    match store.list_all(2000) {
        Ok(recs) => {
            let home = state.home.clone();
            state.history = recs
                .into_iter()
                .map(|r| {
                    let action = match r.action {
                        molt_act::ActionKind::Quarantine => HistoryAction::Quarantined,
                        molt_act::ActionKind::Restore => HistoryAction::Restored,
                        molt_act::ActionKind::Purge => {
                            // 直接清理没有隔离路径；经隔离的清除才有。
                            if r.quarantine_path.as_os_str().is_empty() {
                                HistoryAction::Deleted
                            } else {
                                HistoryAction::Purged
                            }
                        }
                    };
                    HistoryRow {
                        id: r.id,
                        path: crate::paths::display_in(&home, &r.absolute_path),
                        bytes: r.bytes,
                        at: r.performed_at.clone(),
                        source: r.source,
                        action,
                    }
                })
                .collect();
            state.history_error = None;
        }
        Err(e) => {
            state.history_error = Some(format!("读取审计库失败：{e}"));
            state.history.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gone_covers_both_ways_of_losing_something() {
        // 「彻底删除」和「直接清理」对用户是同一件事：永久没了。
        // 分成两档会让两边数字都变小，看不出永久删掉的总量。
        assert!(HistoryFilter::Gone.keeps(HistoryAction::Purged));
        assert!(HistoryFilter::Gone.keeps(HistoryAction::Deleted));
        assert!(!HistoryFilter::Gone.keeps(HistoryAction::Quarantined));
        assert!(!HistoryFilter::Gone.keeps(HistoryAction::Restored));
    }

    #[test]
    fn filters_are_exclusive_and_all_keeps_everything() {
        for a in [
            HistoryAction::Quarantined,
            HistoryAction::Restored,
            HistoryAction::Purged,
            HistoryAction::Deleted,
        ] {
            assert!(HistoryFilter::All.keeps(a), "「全部」不能漏掉任何一条");
            let hits = [
                HistoryFilter::Quarantined,
                HistoryFilter::Gone,
                HistoryFilter::Restored,
            ]
            .iter()
            .filter(|f| f.keeps(a))
            .count();
            assert_eq!(hits, 1, "{a:?} 应当只落在其中一档里");
        }
    }
}
