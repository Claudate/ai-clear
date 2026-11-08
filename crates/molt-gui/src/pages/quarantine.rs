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
    page_header(ui, p, "临时空间", "", |ui| {
        if secondary_button(ui, p, "刷新", !state.is_purging() && state.can_start()) {
            acts.push(Action::LoadQuarantine);
        }
    });
    ui.add_space(p.sz(14.0));
    // 截图顶部的数量直接取当前审计记录；读取中或失败时不能拿旧值当现状。
    if state.quarantine_error.is_none() && !state.quarantine_loading {
        summary_card(ui, state, p);
        ui.add_space(p.sz(12.0));
    }
    storage_note(ui, p, acts);
    ui.add_space(p.sz(10.0));

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

    footer(ui, state, p, acts);

    match crate::widgets::op_report(ui, p, state.last_op.as_deref()) {
        crate::widgets::ReportAction::Dismiss => acts.push(Action::DismissReport),
        crate::widgets::ReportAction::Next(next) => acts.push(Action::FollowNextStep(next)),
        crate::widgets::ReportAction::None => {}
    }
    if state.last_op.is_some() {
        ui.add_space(10.0);
    }

    let row_h = p.sz(78.0);
    let list_h = ui.available_height().max(p.sz(80.0));

    // 标题也占一条虚拟行，避免为按批次分组牺牲千条记录的按需绘制。
    let grouped = grouped_rows(&state.quarantine);
    let mut restore_id: Option<i64> = None;
    let mut toggle_id: Option<i64> = None;
    let mut purge_id: Option<i64> = None;

    egui::ScrollArea::vertical()
        .max_height(list_h)
        .auto_shrink([false, false])
        .show_rows(ui, row_h, grouped.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                match grouped[i] {
                    GroupedRow::Header {
                        first,
                        count,
                        bytes,
                    } => {
                        group_header(ui, p, &state.quarantine[first], count, bytes, row_h);
                    }
                    GroupedRow::Item(index) => {
                        let row = &state.quarantine[index];
                        match line(ui, state, p, row, row_h) {
                            RowAction::None => {}
                            RowAction::Toggle => toggle_id = Some(row.id),
                            RowAction::Restore => restore_id = Some(row.id),
                            RowAction::Purge => purge_id = Some(row.id),
                        }
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
}

/// 标题和条目共用固定行高，`show_rows` 只会绘制视口里的行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupedRow {
    Header {
        first: usize,
        count: usize,
        bytes: u64,
    },
    Item(usize),
}

/// 以真实批次 ID 为键归组；旧记录或异常路径只进入一组未知批次。
fn grouped_rows(rows: &[crate::state::QuarantineRow]) -> Vec<GroupedRow> {
    let mut keys: std::collections::HashMap<Option<&str>, usize> = std::collections::HashMap::new();
    let mut groups: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let batch = row.batch.as_deref();
        let group = *keys.entry(batch).or_insert_with(|| {
            groups.push((batch, Vec::new()));
            groups.len() - 1
        });
        groups[group].1.push(index);
    }

    let mut display = Vec::with_capacity(rows.len() + groups.len());
    for (_, indices) in groups {
        let first = indices[0];
        let bytes = indices.iter().map(|&index| rows[index].bytes).sum();
        display.push(GroupedRow::Header {
            first,
            count: indices.len(),
            bytes,
        });
        display.extend(indices.into_iter().map(GroupedRow::Item));
    }
    display
}

/// 标题与条目等高，保持虚拟列表滚动定位准确；没有批次路径的不贴批次名。
fn group_header(
    ui: &mut egui::Ui,
    p: &Palette,
    first: &crate::state::QuarantineRow,
    count: usize,
    bytes: u64,
    row_h: f32,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0, p.surface_alt);
    let painter = ui.painter_at(rect.shrink2(egui::vec2(p.sz(14.0), p.sz(8.0))));
    let title = if first.batch.is_some() {
        "清理批次"
    } else {
        "未能确定批次"
    };
    let detail = first
        .batch
        .as_deref()
        .unwrap_or("旧版记录或路径不符合批次目录结构");
    let lines = [
        (
            title.to_owned(),
            egui::FontId::proportional(p.sz(12.0)),
            p.text,
        ),
        (
            format!(
                "移入 {} · {count} 项 · 原记录体积 {}",
                crate::components::time::short_time(&first.at),
                molt_scan::human_bytes(bytes)
            ),
            egui::FontId::proportional(p.sz(10.5)),
            p.text_dim,
        ),
        (
            detail.to_owned(),
            egui::FontId::monospace(p.sz(10.0)),
            p.text_faint,
        ),
    ];
    let galleys =
        lines.map(|(text, font, color)| (painter.layout_no_wrap(text, font, color), color));
    let gap = p.sz(3.0);
    let content_h: f32 = galleys
        .iter()
        .map(|(galley, _)| galley.size().y)
        .sum::<f32>()
        + gap * 2.0;
    let mut y = rect.center().y - content_h / 2.0;
    for (galley, color) in galleys {
        let h = galley.size().y;
        painter.galley(egui::pos2(rect.left() + p.sz(14.0), y), galley, color);
        y += h + gap;
    }
}

/// 让用户在空、加载失败和有内容时都能知道文件放在哪里以及如何打开。
fn storage_note(ui: &mut egui::Ui, p: &Palette, acts: &mut Vec<Action>) {
    card(ui, p, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("存放位置")
                    .size(p.sz(12.0))
                    .strong()
                    .color(p.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if secondary_button(ui, p, "在访达中显示", true) {
                    acts.push(Action::RevealQuarantine);
                }
            });
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(crate::paths::quarantine_root().display().to_string())
                    .size(p.sz(10.5))
                    .monospace()
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(p.sz(4.0));
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "当前直供版的数据与应用本体分开保存。移除应用不会自动清空这里；若手动删除应用数据目录，尚未还原的内容和还原记录也会丢失。",
                )
                .size(p.sz(10.5))
                .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
    });
}

/// 一屏最多列多少条待还原记录。
///
/// 上限本身是必要的（极端情况下审计库里有几万条），但它必须**说得出声**：
/// `summary_card` 到顶时会补一行「仅列出最近 N 条」。两处共用这个常量，
/// 免得哪天只改了查询、提示还按旧数判断。
pub(crate) const LIST_CAP: usize = 2000;

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

    ui.vertical(|ui| {
        // 与设计图一致：数量紧跟页头，大小和项数是同一层级，而不是孤立的两张卡。
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{} 待处理", molt_scan::human_bytes(bytes)))
                        .size(p.sz(19.0))
                        .strong()
                        .color(p.text),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
            ui.add_space(p.sz(10.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{} 项可还原", restorable.len()))
                        .size(p.sz(16.0))
                        .strong()
                        .color(p.text),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
        ui.add_space(p.sz(4.0));
        // 保留期不是全局常量：每条记录落盘时就带着自己的 `purge_after`，
        // 定时任务可以自行设。行内的进度条已经按各自的到期时间算，页头要是
        // 还写死「默认 7 天」，就会和下面某几行的「已到期」自相矛盾。
        ui.add(
            egui::Label::new(
                egui::RichText::new("到期前可还原；移入这里不会释放磁盘空间，彻底删除才会释放。")
                    .size(p.sz(11.5))
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        if let Some((d, row)) = soonest {
            ui.add_space(p.sz(4.0));
            let when = if d > 0 {
                format!("{d} 天后")
            } else if d == 0 {
                "今天".to_string()
            } else {
                "已到期".to_string()
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
            ui.add_space(p.sz(4.0));
            badge(
                ui,
                p,
                &format!("{broken} 项内容已不在"),
                p.costly,
                p.costly_soft,
            );
        }
        // 清单有上限（`LIST_CAP`，在 `reload` 里传给 `list_pending`）。到了
        // 上限就明说「没看全」——历史页已经这么做了，这里不能默默截断：
        // 用户数着行数会以为临时空间里就这么多。
        if state.quarantine.len() >= LIST_CAP {
            ui.add_space(p.sz(4.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "仅列出最近 {} 条，更早的记录没有显示。",
                        state.quarantine.len()
                    ))
                    .size(p.sz(11.0))
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
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
    // 批量与单条现在共用同一个后台任务：任何还原在跑时，这一行的按钮都该置灰。
    // 以前这里看 `state.restoring`，而单条路径改成后台之后那个字段就再也不会被写。
    let busy = state.restore_job.is_some();
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
                        let remaining = days_left(&row.purge_after);
                        let note = if row.restorable {
                            match remaining {
                                // 「还剩几天」比一个日期有用得多：用户来这页想知道的是
                                // 会不会没，而不是到期日是哪一天。
                                Some(d) if d > 0 => format!(
                                    "移入 {} · 还剩 {d} 天（{} 到期）",
                                    crate::components::time::short_time(&row.at),
                                    crate::components::time::short_time(&row.purge_after)
                                ),
                                Some(0) => format!(
                                    "移入 {} · 今天到期，下次启动尝试清理",
                                    crate::components::time::short_time(&row.at)
                                ),
                                Some(_) => format!(
                                    "移入 {} · 已到期，下次启动尝试清理",
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
                        if let Some(days) = remaining.filter(|_| row.restorable) {
                            // 定时任务可以自行设保留期，不能把所有记录都按默认 7 天画。
                            let total = days_until(&row.purge_after, &row.at)
                                .unwrap_or(crate::paths::DEFAULT_RETAIN_DAYS)
                                .max(1);
                            let fraction = (days as f32 / total as f32).clamp(0.0, 1.0);
                            let color = if days <= 1 { p.costly } else { p.reclaimable };
                            crate::widgets::progress_bar(
                                ui,
                                p,
                                fraction,
                                p.sz(148.0).min(ui.available_width()),
                                "",
                                p.sz(3.0),
                                color,
                            );
                        }
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
    egui::Panel::bottom("quarantine-actions")
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(p.bg)
                .inner_margin(egui::Margin::symmetric(0, 6)),
        )
        .show(ui, |ui| {
            crate::widgets::hairline(ui, p);
            let n = state.quarantine_checked_count();
            // 互斥：只要有任务在跑，还原与彻底删除都要置灰并给出理由
            let enabled = n > 0 && state.can_start();

            let status = |ui: &mut egui::Ui| {
                if n == 0 {
                    ui.label(
                        egui::RichText::new("选中条目后可批量还原或彻底删除")
                            .size(p.sz(12.0))
                            .color(p.text_dim),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "已选 {n} 项 · {}",
                            molt_scan::human_bytes(state.quarantine_checked_bytes())
                        ))
                        .size(p.sz(13.0))
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new("还原放回原位置；彻底删除不可恢复")
                            .size(p.sz(10.5))
                            .color(p.text_faint),
                    );
                }
            };
            let mut buttons = |ui: &mut egui::Ui| {
                if primary_button(ui, p, "还原所选", enabled) {
                    acts.push(Action::RestoreSelected);
                }
                ui.add_space(6.0);
                if danger_ghost_button(ui, p, "彻底删除…", enabled) {
                    acts.push(Action::PreparePurge);
                }
                ui.add_space(8.0);
                if secondary_button(ui, p, "全不选", n > 0 && state.can_select()) {
                    acts.push(Action::ClearQuarantineSelection);
                }
                // 「全选」只在真有可还原条目时才可点：全都写着「内容已不在」时
                // 按下去只会得到空集，什么反应都没有——那是死按钮。
                let any_restorable = state.quarantine.iter().any(|r| r.restorable);
                if secondary_button(ui, p, "全选", state.can_select() && any_restorable) {
                    acts.push(Action::SelectAllQuarantine);
                }
            };
            if ui.available_width() < p.sz(680.0) {
                ui.vertical(|ui| {
                    status(ui);
                    ui.add_space(p.sz(4.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        buttons(ui);
                    });
                });
            } else {
                ui.horizontal(|ui| {
                    ui.vertical(status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        buttons(ui);
                    });
                });
            }
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
            p.sz(6.0),
            p.reclaimable,
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
            p.sz(6.0),
            p.reclaimable,
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

/// 只认可 `root/<batch>/<item>` 的单层结构；旧版或异常路径不推断批次。
pub(crate) fn batch_id(
    root: &std::path::Path,
    quarantine_path: &std::path::Path,
) -> Option<String> {
    use std::path::Component;
    let mut relative = quarantine_path.strip_prefix(root).ok()?.components();
    let batch = match relative.next()? {
        Component::Normal(value) => value.to_str()?.to_owned(),
        _ => return None,
    };
    if batch.is_empty()
        || !matches!(relative.next()?, Component::Normal(_))
        || relative.next().is_some()
    {
        return None;
    }
    Some(batch)
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
    match store.list_pending(LIST_CAP) {
        Ok(recs) => {
            let home = state.home.clone();
            let root = crate::paths::quarantine_root();
            state.quarantine = recs
                .into_iter()
                .map(|r| crate::state::QuarantineRow {
                    id: r.id,
                    original: crate::paths::display_in(&home, &r.absolute_path),
                    bytes: r.bytes,
                    at: r.performed_at.clone(),
                    purge_after: r.purge_after.clone(),
                    batch: batch_id(&root, &r.quarantine_path),
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

/// 还原一条。
///
/// 走的是和批量完全相同的后台任务（`restore_ids`）：大项目目录要解压几十秒，
/// 同步做会把窗口冻住、连「还原中…」这几个字都画不出来——设置状态和清除状态
/// 发生在同一次调用里，中间没有一帧。
pub fn restore(state: &mut AppState, id: i64) {
    // 互斥：还原是写盘动作，不能和扫描/清理/定时任务并行
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    restore_ids(state, &[id]);
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
            batch: Some("2026-10-20-clean".into()),
            restorable,
        }
    }

    #[test]
    fn batch_id_only_accepts_one_batch_directory_under_root() {
        use std::path::Path;
        let root = Path::new("/tmp/quarantine");
        assert_eq!(
            batch_id(root, &root.join("2026-10-20-clean").join("42-cache")),
            Some("2026-10-20-clean".into())
        );
        for invalid in [
            root.join("42-cache"),
            root.join("batch").join("nested").join("42-cache"),
            root.join("..")
                .join("outside")
                .join("batch")
                .join("42-cache"),
            Path::new("/tmp/other/batch/42-cache").to_path_buf(),
        ] {
            assert_eq!(
                batch_id(root, &invalid),
                None,
                "不应把 {invalid:?} 说成真实批次"
            );
        }
    }

    #[test]
    fn grouped_rows_keep_original_row_indices_and_unknown_records_separate() {
        let mut rows = vec![row(1, true), row(2, true), row(3, false), row(4, true)];
        rows[1].batch = Some("second-batch".into());
        rows[2].batch = None;
        let grouped = grouped_rows(&rows);
        assert_eq!(
            grouped,
            vec![
                GroupedRow::Header {
                    first: 0,
                    count: 2,
                    bytes: 200
                },
                GroupedRow::Item(0),
                GroupedRow::Item(3),
                GroupedRow::Header {
                    first: 1,
                    count: 1,
                    bytes: 100
                },
                GroupedRow::Item(1),
                GroupedRow::Header {
                    first: 2,
                    count: 1,
                    bytes: 100
                },
                GroupedRow::Item(2),
            ]
        );
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
