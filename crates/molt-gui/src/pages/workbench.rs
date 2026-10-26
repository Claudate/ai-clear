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

use crate::state::{AppState, Filter, PendingItem, Priority, StatusKind, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{
    ai_note, badge, busy_bar, card, danger_button, danger_ghost_button, page_header,
    primary_button, progress_bar, secondary_button, section_header, state_block,
};
use molt_act::CleanMode;

pub fn show(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    // 底栏先占位：主操作必须始终看得见，绝不能随清单滚出屏幕。
    //
    // 只在清单展开时出现：折叠状态首页已经有一个满宽的大按钮，
    // 同一屏摆两个「清这些」会让人不知道该点哪个；而且底栏会吃掉
    // 首屏高度，把环挤小。
    let has_items = state
        .result
        .as_ref()
        .map(|r| !r.items.is_empty())
        .unwrap_or(false);
    if has_items && state.list_expanded {
        footer_bar(ui, state, p);
    }

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
                // 旧结果还在：不丢掉它，但必须先说明它是旧的。
                stale_banner(ui, state, p, &err);
            }

            // 磁盘读数不依赖扫描（state.disk 每帧都有），所以下面两条早退路径
            // 也要先把仪表盘画出来：否则打开软件的第一眼是一张空的扫描卡，
            // 「盘还剩多少」这件最该先看到的事反而要等扫描跑完。
            let Some(result) = state.result.clone() else {
                hero(ui, state, p);
                ui.add_space(12.0);
                if state.is_scanning() {
                    scanning_card(ui, state, p);
                } else {
                    hint_line(
                        ui,
                        p,
                        "还没有扫描结果。点上面的按钮开始——只读取文件信息，不会改动任何东西。",
                    );
                }
                return;
            };

            if result.items.is_empty() {
                hero(ui, state, p);
                ui.add_space(12.0);
                hint_line(
                    ui,
                    p,
                    "没有找到可清理的内容：规则覆盖的目录目前都是空的，\
                     或者这些工具还没在这台 Mac 上产生缓存。",
                );
                return;
            }

            hero(ui, state, p);
            ui.add_space(12.0);
            report_banner(ui, state, p);
            // 首屏只留一行「看看都清什么」。清单默认收起，否则一屏全被条目吃掉，
            // 环和大按钮被顶到看不见——而它们才是「一眼看懂」要解决的问题。
            list_toggle(ui, state, p);
            if state.list_expanded {
                list(ui, state, p);
            }
        });
}

fn header(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    page_header(
        ui,
        p,
        "清理",
        "扫描只读；清理先放进临时空间，7 天内可还原。",
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

/// 首页主体：一块盘的仪表盘。
///
/// 排法参照 360 那类「一眼看懂」的做法：一个环，环心写还剩多少，环正下方是
/// 一行居中的三色图例与一个满宽大按钮。打开这一页要能同时得到三件事——盘还剩多少、
/// 能腾多少、下一步点哪——而且都不需要读完一句话。
///
/// 磁盘读数不依赖扫描（`state.disk` 每帧都有），所以第一帧就有画面，
/// 不会出现「打开先看到一张空的扫描卡」。
fn hero(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let (clean_n, clean_bytes) = state.cleanable_summary();

    card(ui, p, |ui| {
        // 上下排：环在上、文字与按钮在下。
        //
        // 旧版宽窗口时左右分栏，用户的眼睛要在环与按钮之间来回横跳；
        // 这一页只回答一件事——「清多少、点哪个」，那就必须是一条从环
        // 向下读到底的竖线。宽度用窗口宽度判：这一页在 ScrollArea 里，
        // `available_width` 拿到的是可滚动范围，判什么都是「够宽」。
        let screen_w = ui.ctx().content_rect().width();
        let d = (screen_w * 0.30).clamp(p.sz(180.0), p.sz(300.0));

        ui.vertical_centered(|ui| ring(ui, state, p, d, clean_bytes));
        ui.add_space(p.sz(18.0));
        dashboard_side(ui, state, p, clean_n, clean_bytes);
    });
}

/// 圆环本体：三段扇区 + 圆心的三行字，以及清理时的动画叠层。
///
/// 分三段而不是两块：用户要知道「能清掉的那部分在整块盘里占多大」，
/// 所以可清理单独一色，剩下的已用是中性色，可用是浅色。
///
/// 清理进行中时，在上面叠一层满绿弧（从 12 点顺时针长出去）：
/// 环本身在动、圆心也在数数，用户不需要读任何文字就知道「它正在干活」。
fn ring(ui: &mut egui::Ui, state: &AppState, p: &Palette, d: f32, clean_bytes: u64) {
    use crate::widgets::Arc;

    let thick = (d * 0.115).max(p.sz(10.0));
    let mut arcs: Vec<Arc> = Vec::new();
    let headline: String;
    let headline_color: egui::Color32;
    let footnote: String;

    // 清理中 / 刚清完：圆心写进度与结果，而不是磁盘数字。
    // 判据用动画本身而不是 `is_cleaning()`：动画只在清理期间被推进，
    // 被中断时立刻归零，所以「弧还在」就等于「正在清」。
    let anim = state.clean_anim.clamp(0.0, 1.0);
    let done = state.clean_done_secs > 0.0;
    let cleaning = (anim > 0.0 && !done) || state.is_cleaning();

    match &state.disk {
        Some(Ok(disk)) => {
            let denom = (disk.used + disk.available).max(1) as f32;
            let used_share = disk.used as f32 / denom;
            let avail_share = disk.available as f32 / denom;
            // 「可清理」最多只能占「已用」那一段：它本来就是已用空间里的缓存，
            // 若因读数抖动算超过已用，环上会出现一块不属于任何含义的颜色。
            let clean_share = (clean_bytes as f32 / denom).min(used_share);

            let mut at = 0.0;
            if clean_share > 0.0 {
                let sweep = clean_share * 360.0;
                arcs.push(Arc {
                    start: at,
                    sweep,
                    color: p.reclaimable,
                });
                at += sweep;
            }
            let rest = (used_share - clean_share).max(0.0) * 360.0;
            if rest > 0.0 {
                arcs.push(Arc {
                    start: at,
                    sweep: rest,
                    color: p.used_bar,
                });
                at += rest;
            }
            arcs.push(Arc {
                start: at,
                sweep: avail_share * 360.0,
                color: p.border_strong,
            });

            headline = molt_scan::human_bytes(disk.available);
            headline_color = if avail_share < 0.10 {
                p.danger
            } else if avail_share < 0.20 {
                p.costly
            } else {
                p.reclaimable
            };
            footnote = if state.result.is_none() && state.is_scanning() {
                format!("共 {} · 正在检查", molt_scan::human_bytes(disk.total))
            } else if state.result.is_none() {
                format!("共 {} · 还没开始扫描", molt_scan::human_bytes(disk.total))
            } else {
                format!(
                    "共 {} · 已用 {:.0}%",
                    molt_scan::human_bytes(disk.total),
                    used_share * 100.0
                )
            };
        }
        // 读不到就不画扇区、也不编数字：一条 0% 的环会让人以为盘是空的。
        _ => {
            headline = "—".to_string();
            headline_color = p.text_dim;
            footnote = "读不到磁盘容量".to_string();
        }
    }

    let resp = crate::widgets::donut(ui, p, d, thick, &arcs);
    let center = resp.rect.center();
    let outer = d / 2.0;
    let inner = (outer - thick).max(0.0);

    // ---- 清理动画层：叠在盘使用的扇区之上 ----
    if anim > 0.0 || cleaning || done {
        let progress = if done { 1.0 } else { anim };

        // 先把整圈压暗一层。不压暗的话，进度弧和底下「可清理」那段是同一种绿，
        // 叠上去几乎看不出来——「正在清」就成了需要仔细比对才发现的差别。
        let veil =
            egui::Color32::from_rgba_unmultiplied(p.surface.r(), p.surface.g(), p.surface.b(), 190);
        crate::widgets::paint_ring(
            ui.painter(),
            p,
            center,
            outer,
            inner,
            false,
            &[Arc {
                start: 0.0,
                sweep: 360.0,
                color: veil,
            }],
        );

        let overlay = [Arc {
            start: 0.0,
            sweep: progress * 360.0,
            color: p.reclaimable,
        }];
        // 不铺底环（`base = false`）：底下是磁盘真数字，盖掉就变成两张图
        crate::widgets::paint_ring(ui.painter(), p, center, outer, inner, false, &overlay);

        // 进度头：一个亮点跟着弧头跑。静止的弧看不出「还在动」，
        // 而清理可能持续几十秒，用户需要看到它在走。
        if cleaning && progress > 0.01 && progress < 0.999 {
            let deg = progress * 360.0 - 90.0;
            let (sn, cs) = (deg.to_radians().sin(), deg.to_radians().cos());
            let mid = (outer + inner) / 2.0;
            let head = center + egui::vec2(cs * mid, sn * mid);
            ui.painter()
                .circle_filled(head, thick * 0.62, p.reclaimable.gamma_multiply(0.45));
            ui.painter()
                .circle_filled(head, thick * 0.34, egui::Color32::WHITE);
        }
    }

    let big = (d * 0.135).clamp(p.sz(15.0), p.sz(30.0));
    let small = (d * 0.048).clamp(p.sz(8.5), p.sz(11.5));

    // 圆心三行：清理中 → 百分比；刚清完 → 腾出多少；其余 → 磁盘读数
    let (top, mid, mid_color, bottom) = if cleaning {
        (
            "正在清理".to_string(),
            format!("{:.0}%", (anim * 100.0).min(99.0)),
            p.reclaimable,
            if state.clean_total_items > 0 {
                format!(
                    "共 {} 项 · {}",
                    state.clean_total_items,
                    molt_scan::human_bytes(state.clean_total_bytes)
                )
            } else {
                "请稍候…".to_string()
            },
        )
    } else if done {
        (
            "清理完成".to_string(),
            format!("+{}", molt_scan::human_bytes(state.clean_total_bytes)),
            p.reclaimable,
            if state.clean_was_delete {
                "已腾出空间".to_string()
            } else {
                "已放进临时空间".to_string()
            },
        )
    } else {
        ("还剩".to_string(), headline, headline_color, footnote)
    };

    ui.painter().text(
        egui::pos2(center.x, center.y - big * 0.92),
        egui::Align2::CENTER_CENTER,
        top,
        egui::FontId::proportional(small),
        p.text_dim,
    );
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        &mid,
        egui::FontId::proportional(big),
        mid_color,
    );
    ui.painter().text(
        egui::pos2(center.x, center.y + big * 0.86),
        egui::Align2::CENTER_CENTER,
        &bottom,
        egui::FontId::proportional(small),
        p.text_faint,
    );
}

/// 首页那颗大按钮按下之后做什么。
///
/// 抽成函数是为了让测试能走「点按钮」这条真路径，而不是在测试里把同样的
/// 分支再写一遍——那样界面改了、测试还是绿的。
///
/// 有可清理量时走 `pending_from_checked`，和底栏是同一个入口：
/// 首页不另开一条执行路径，否则两处对「默认清什么」的理解会慢慢走偏。
pub fn hero_action(state: &mut AppState) {
    let (clean_n, _) = state.cleanable_summary();
    if clean_n > 0 {
        state.pending_clean = state.pending_from_checked(CleanMode::Quarantine);
    } else {
        start_scan(state);
    }
}

/// 环右侧（窄窗口时是环下方）的图例与主按钮。
fn dashboard_side(
    ui: &mut egui::Ui,
    state: &mut AppState,
    p: &Palette,
    clean_n: usize,
    clean_bytes: u64,
) {
    // 先把磁盘数字取出来，后面改 state 时不再借它。
    let disk = match &state.disk {
        Some(Ok(d)) => Some((d.used, d.available)),
        _ => None,
    };

    // 图例：环正下方一行，整行水平居中，永不折行。
    //
    // 为什么不能直接 `ui.horizontal_wrapped` 或再套一层 `vertical_centered`：
    // `horizontal` 的 frame 会吃掉整条可用宽度（`available_size_before_wrap`），
    // 于是「把这一行居中」等于「把一条满宽的框居中」，视觉上没有位移；
    // 而 `Layout::left_to_right` 的 x 对齐在 egui 内部被强制成 `Align::LEFT`，
    // 套 `vertical_centered` 也改不动它。唯一可靠的做法是先量出这一行真实的
    // 宽度，再向父级申请**正好这么宽**的一块，由父级的十字轴居中。
    let clean_value = if state.result.is_none() {
        "—".to_string()
    } else {
        molt_scan::human_bytes(clean_bytes)
    };
    let mut cells: Vec<LegendCell> = vec![LegendCell {
        color: p.reclaimable,
        label: "可清理".to_string(),
        value: clean_value,
        clickable: state.result.is_some(),
    }];
    if let Some((used, available)) = disk {
        cells.push(LegendCell {
            color: p.used_bar,
            label: "已用".to_string(),
            value: molt_scan::human_bytes(used),
            clickable: false,
        });
        cells.push(LegendCell {
            color: p.border_strong,
            label: "可用".to_string(),
            value: molt_scan::human_bytes(available),
            clickable: false,
        });
    }
    if legend_row(ui, p, &cells).is_some() {
        state.filter = Filter::Cleanable;
        state.list_expanded = true;
        state.scroll_to_list = true;
    }


    ui.add_space(p.sz(14.0));

    let scanning = state.is_scanning();
    let can = state.can_start();

    // 两种清理方式并排：左边是默认的「放入临时空间」，右边是「直接清理」。
    //
    // 用户明确要两个选项，而不是一个主按钮加一个措辞模糊的尾巴：
    // 「直接清理」能真的腾出空间，「放入临时空间」能反悔。这是两种目的，
    // 不是两种危险程度，所以两个都得看得见、都能点。
    ui.vertical_centered(|ui| {
        let label = if scanning {
            "正在检查…".to_string()
        } else if state.is_cleaning() {
            "正在清理…".to_string()
        } else if clean_n > 0 {
            format!(
                "放入临时空间 · {}（{} 天可还原）",
                molt_scan::human_bytes(clean_bytes),
                crate::paths::DEFAULT_RETAIN_DAYS
            )
        } else if state.result.is_some() {
            "重新扫描".to_string()
        } else {
            "开始扫描".to_string()
        };
        let enabled = !scanning && can;
        // 限宽而不是满宽：一条通栏的长条会盖过圆环，
        // 而它只是「下一步」，不该比环更抢眼。
        let w = ui.available_width().min(p.sz(460.0));
        if crate::widgets::hero_button(ui, p, &label, w, enabled) {
            hero_action(state);
        }

        // 「直接清理」：同一次勾选，另一种模式。它不抢主位，但也不藏进省略号。
        if clean_n > 0 && !scanning {
            ui.add_space(p.sz(8.0));
            let dl = format!(
                "直接清理掉 {}（不可恢复）",
                molt_scan::human_bytes(clean_bytes)
            );
            if danger_ghost_button(ui, p, &dl, can) {
                state.pending_clean = state.pending_from_checked(CleanMode::Delete);
            }
        }

        if !enabled && !scanning {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(state.activity().blocking_hint())
                        .size(p.sz(11.0))
                        .color(p.costly),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
    });

    ui.add_space(p.sz(8.0));
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "只清理能自动重建的缓存，你的文件、密钥、聊天记录不会被碰。\
                 清理后 7 天内可随时还原，期间空间不会立刻变多。",
            )
            .size(p.sz(11.0))
            .color(p.text_dim),
        )
        .wrap_mode(egui::TextWrapMode::Wrap),
    );

    if state.session_freed > 0 {
        ui.add_space(p.sz(4.0));
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
}

/// 图例里的一格。先收集再排版，是因为整行要居中就得先知道整行多宽。
struct LegendCell {
    color: egui::Color32,
    label: String,
    value: String,
    /// 可点的只有「可清理」：点它等于说「我要看这些条目」。
    clickable: bool,
}

/// 图例一行：环正下方，整行水平居中，永不折行。
///
/// 为什么不用 `ui.horizontal_wrapped`：它在窄窗口里会把「已用 / 可用」折到
/// 第二行，用户读到的是两行等宽的数字，反而比一行更难比大小。也不套
/// `vertical_centered`——`horizontal` 的 frame 会吃掉整条可用宽度，
/// 把一条满宽的框居中，视觉上没有任何位移。
///
/// 所以分两步：先按真实字体量出每格宽度（`layout_no_wrap`），算出整行宽度，
/// 再向父级申请**正好这么宽**的一块，由父级的十字轴居中。量宽和画图必须
/// 用同一份字符串、同一个字号，否则会出现「量的时候是 `—`、画的时候是 `19.0 GB`」
/// 的错位，整行就会偏。
///
/// 返回被点中的格序号。
fn legend_row(ui: &mut egui::Ui, p: &Palette, cells: &[LegendCell]) -> Option<usize> {
    if cells.is_empty() {
        return None;
    }

    let gap = p.sz(18.0);
    let inner_gap = p.sz(5.0);
    let label_font = egui::FontId::proportional(p.sz(11.5));
    let value_font = egui::FontId::proportional(p.sz(15.0));
    // `ui.fonts` 只给不可变借用，而排版要可变；painter 的克隆能拿同一套字体。
    let painter = ui.painter().clone();
    let dot = crate::widgets::dot_size();

    let mut total_w = gap * (cells.len() as f32 - 1.0);
    let mut row_h: f32 = dot;
    for c in cells {
        let lw = painter
            .layout_no_wrap(c.label.clone(), label_font.clone(), p.text_dim)
            .size();
        let vw = painter
            .layout_no_wrap(c.value.clone(), value_font.clone(), p.text)
            .size();
        total_w += dot + inner_gap + lw.x + inner_gap + vw.x;
        row_h = row_h.max(lw.y).max(vw.y);
    }

    let mut clicked = None;
    ui.vertical_centered(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(total_w, row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (i, c) in cells.iter().enumerate() {
                    let hit = if c.clickable {
                        legend(ui, p, c.color, &c.label, &c.value)
                    } else {
                        legend_readonly(ui, p, c.color, &c.label, &c.value);
                        false
                    };
                    if hit {
                        clicked = Some(i);
                    }
                }
            },
        );
    });
    clicked
}

/// 图例一格（可点）。返回是否被点中。
///
/// 图例放在环下方而不是贴着弧线做指引线：指引线在窄窗口里会互相压住。
fn legend(ui: &mut egui::Ui, p: &Palette, color: egui::Color32, label: &str, value: &str) -> bool {
    let resp = ui.horizontal(|ui| legend_body(ui, p, color, label, value));
    let hit = resp.response.interact(egui::Sense::click());
    if hit.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    hit.clicked()
}

fn legend_readonly(ui: &mut egui::Ui, p: &Palette, color: egui::Color32, label: &str, value: &str) {
    ui.horizontal(|ui| legend_body(ui, p, color, label, value));
}

fn legend_body(ui: &mut egui::Ui, p: &Palette, color: egui::Color32, label: &str, value: &str) {
    ui.spacing_mut().item_spacing.x = p.sz(5.0);
    crate::widgets::dot(ui, color);
    ui.add(
        egui::Label::new(
            egui::RichText::new(label)
                .size(p.sz(11.5))
                .color(p.text_dim),
        )
        .wrap_mode(egui::TextWrapMode::Extend),
    );
    ui.add(
        egui::Label::new(egui::RichText::new(value).size(p.sz(15.0)).strong())
            .wrap_mode(egui::TextWrapMode::Extend),
    );
}

/// 一行可点的文字链：用来把「还有一档」直接指到清单里的对应分段。
///
/// 首页只放结论，不铺明细；想看某一档的条目时，点这里就跳到清单并切好筛选——
/// 比让用户自己去底部找分段控件少一步。
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

fn report_banner(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    if crate::app::apply_report_action(
        state,
        crate::widgets::op_report(ui, p, state.last_op.as_deref()),
    ) {
        state.last_op = None;
    }
    if state.last_op.is_some() {
        ui.add_space(10.0);
    }
}

/// 重扫失败但旧结果还在时，先说清「下面这些是什么时候的」。
///
/// 不这么做的话，用户看到的是上一次成功的数字，却以为刚扫过——而清理
/// 是按这些数字做的决定，代价可能是一次不必要的重建。
fn stale_banner(ui: &mut egui::Ui, state: &mut AppState, p: &Palette, err: &str) {
    egui::Frame::NONE
        .fill(p.costly_soft)
        .stroke(egui::Stroke::new(1.0, p.costly.gamma_multiply(0.35)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("这次扫描失败：{err}"))
                            .size(p.sz(12.0))
                            .strong()
                            .color(p.costly),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                if let Some(at) = &state.last_scan_at {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("下面仍是 {at} 那次的结果"))
                                .size(p.sz(11.0))
                                .color(p.text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                } else {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("下面是一份较早的结果")
                                .size(p.sz(11.0))
                                .color(p.text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
                if secondary_button(ui, p, "重试", state.can_start()) {
                    start_scan(state);
                }
            });
        });
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

/// 「看看都清什么（24 项）」那一行。点开才走完整的工具条与分节清单。
///
/// 为什么折叠：清单一次列二十多项，用户第一眼看到的是「一堆不认识的名字」；
/// 而这一页真正要回答的是「还剩多少、能腾多少、点哪个」。
fn list_toggle(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let total = state.result.as_ref().map(|r| r.items.len()).unwrap_or(0);
    let label = if state.list_expanded {
        "收起清单 \u{25b4}".to_string()
    } else {
        format!("看看都清什么（{total} 项） \u{25b8}")
    };
    if secondary_button(ui, p, &label, true) {
        state.list_expanded = !state.list_expanded;
        if state.list_expanded {
            state.scroll_to_list = true;
        }
    }
}

/// 分档清单：只在用户点了「看看都清什么」之后才走。
///
/// 打开首页时的目标很窄——**看清能清多少、然后点清理**。所以清单默认收起，
/// 顶部一行分段筛选，想只看某一档时点一下就换，不靠滚；
/// AI 解释收进这一行右侧的一个按钮，因为它是可选动作。
fn list(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let Some(result) = state.result.clone() else {
        return;
    };
    let busy = state.activity();

    toolbar(ui, state, p, &busy);
    ui.add_space(10.0);

    // hero 里的「要重建 / 是你的记录」两个入口只设了筛选，用户点了如果没滚动，
    // 会觉得「什么都没发生」——清单在屏幕外。这里把视图移到清单开头。
    if state.scroll_to_list {
        state.scroll_to_list = false;
        ui.scroll_to_rect(ui.max_rect(), Some(egui::Align::TOP));
    }

    let visible = state.visible_items();
    if visible.is_empty() {
        empty_list(ui, state, p, &result);
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
            if row(ui, state, p, &result, *i) {
                toggle = Some(result.items[*i].path.display().to_string());
            }
        }
        ui.add_space(12.0);
    }

    if let Some(path) = toggle {
        state.toggle(&path);
    }
}

/// 清单上方的工具条：筛选分段 + 搜索 + AI 解释入口。
fn toolbar(ui: &mut egui::Ui, state: &mut AppState, p: &Palette, busy: &crate::state::Activity) {
    let (clean_n, clean_bytes) = state.cleanable_summary();
    let (costly_n, _) = state.summary_of(Priority::Costly);
    let (keep_n, _) = state.summary_of(Priority::Keep);
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
            (format!("全部 {total_n}"), state.filter == Filter::All),
        ];
        let refs: Vec<(&str, bool)> = opts.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        if let Some(i) = crate::widgets::segmented(ui, p, &refs) {
            state.filter = match i {
                0 => Filter::Cleanable,
                1 => Filter::Costly,
                2 => Filter::Protected,
                _ => Filter::All,
            };
            state.list_expanded = true;
        }

        // 筛选一旦不是「全部」，就必须永远有一条回得去的路。
        // hero 里的两个链接会切筛选，用户点完只看到列表变了、却找不到开关。
        if state.filter != Filter::All && link(ui, p, "显示全部", p.reclaimable) {
            state.filter = Filter::All;
        }

        ui.add_space(8.0);
        let edit = egui::TextEdit::singleline(&mut state.query)
            .hint_text(
                egui::RichText::new("搜索名字或路径")
                    .size(p.sz(12.0))
                    .color(p.text_faint),
            )
            .desired_width(200.0 * p.scale)
            .margin(egui::Margin::symmetric(8, 6));
        ui.add(edit);
        if !state.query.is_empty() && secondary_button(ui, p, "清空", true) {
            state.query.clear();
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ai_action(ui, state, p, busy);
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
fn ai_action(ui: &mut egui::Ui, state: &mut AppState, p: &Palette, busy: &crate::state::Activity) {
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
    if secondary_button(ui, p, label, state.can_start() && resolved.is_ok()) {
        start_analysis(state);
    }
    if let Some(f) = &state.ai.failure {
        if !state.ai.is_running() {
            // 顺序：原因在左、动作在右。这段画在 right_to_left 布局里，
            // 所以先加的那个反而靠右——先加按钮，用户才会先读到原因。
            if f.fixable_by_config && secondary_button(ui, p, "去设置", true) {
                state.page = crate::state::Page::Settings;
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
    state: &mut AppState,
    p: &Palette,
    result: &crate::worker::ScanResult,
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
            state.query.clear();
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
                state.filter = Filter::Protected;
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
                [
                    rect.left_bottom() + egui::vec2(3.0, -3.0),
                    rect.right_top() - egui::vec2(3.0, 3.0),
                ],
                egui::Stroke::new(1.0, p.border),
            );
        }
    }
}

/// 窗口底部固定操作条：**主操作不随清单滚动**。
///
/// 旧版把「清理这些」埋在首页那张大卡片中间，下面还接着 AI 条与几百行清单；
/// 用户往下翻一眼清单，那个按钮就再也看不到了，只能往回滚。
/// 清理工具里「选中 → 执行」是最常走的一步，它必须一直在手边。
fn footer_bar(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
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
                        state.pending_clean = state.pending_from_checked(CleanMode::Quarantine);
                    }
                    // 不可恢复的路留在主按钮左边，且降一档：它需要用户主动去找，
                    // 而不是和主按钮并排抢眼。
                    if danger_ghost_button(ui, p, "直接清理…", n > 0 && can) {
                        state.pending_clean = state.pending_from_checked(CleanMode::Delete);
                    }
                    if secondary_button(ui, p, "全不选", n > 0 && state.can_select()) {
                        state.clear_selection();
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
        "直接清理这些内容？"
    } else {
        "把这些内容放入临时空间？"
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
                                    "内容不会被放进临时空间，删掉之后没有任何找回的途径。",
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
                            "临时空间会保留 {} 天，期间随时可以还原。可用空间不会立刻变多——文件只是被移到了我们自己的临时空间里。",
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
                    "确认直接清理"
                } else {
                    "确认放入临时空间"
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
                                    egui::RichText::new(n).size(p.sz(10.0)).color(p.text_faint),
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
            // 圆环跑满、满绿停 1.8 秒再回到磁盘读数：清完那一下要看得见。
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
            state.apply_result(result);
            state.refresh_disk();
            // 记下这份结果的时刻：下次扫描失败时要用它说明「旧数据是哪来的」。
            state.last_scan_at = Some(crate::paths::now_hhmm());
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

    /// 首页必须是上下排：圆环在上、图例与按钮在下。
    ///
    /// 用画出来的 y 坐标验，而不是靠看图：宽窗口下曾经排成左右两栏，
    /// 用户的视线要在环和按钮之间横跳。这条断言钉住「竖着读」这个要求。
    #[test]
    fn hero_stacks_ring_above_the_buttons() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        // 宽窗口——旧版正是从这里开始分栏的
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 2200.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(ui, &mut state, &Palette::light());
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
        let ring_y = y("还剩").expect("圆心要有「还剩」");
        let legend_y = y("可清理").expect("要有可清理图例");
        let button_y = y("放入临时空间").expect("要有清理主按钮");

        assert!(
            ring_y < legend_y,
            "圆环要在图例上面：环 {ring_y}，图例 {legend_y}"
        );
        assert!(
            legend_y < button_y,
            "图例要在按钮上面：图例 {legend_y}，按钮 {button_y}"
        );
    }

    /// 图例必须一行、居中、不折行——两种窗口宽度下都要成立。
    ///
    /// 用画出来的矩形验，而不是靠看图：折行与偏移在缩略图上都不明显，
    /// 但用户一眼就能看出「这一行歪了」。1120 是默认窗口，760 是窄窗口
    /// （`shots.rs` 里 `18-narrow-workbench` 用的就是 760×640）。
    #[test]
    fn legend_is_one_line_and_centered() {
        for w in [1120.0_f32, 760.0] {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            crate::theme::Theme::install(&ctx);
            let mut state = state_with_notes();
            // 有磁盘读数才会画出「已用 / 可用」两格，三格才谈得上「一行」
            state.disk = Some(Ok(molt_scan::disk::DiskUsage {
                total: 500 << 30,
                used: 320 << 30,
                available: 180 << 30,
            }));

            let mut rects = Vec::new();
            for _ in 0..2 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w, 900.0),
                    )),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(raw, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        show(ui, &mut state, &Palette::light());
                    });
                });
                out.textures_delta.clear();
                rects.clear();
                collect_text_rects(&out.shapes, &mut rects);
            }

            let find = |needle: &str| -> egui::Rect {
                rects
                    .iter()
                    .find(|(t, _)| t == needle)
                    .map(|(_, r)| *r)
                    .unwrap_or_else(|| panic!("{w} 宽时找不到「{needle}」：{rects:#?}"))
            };
            let clean = find("可清理");
            let used = find("已用");
            let avail = find("可用");

            // 断言一：三格在同一条基线上
            let ys = [clean.center().y, used.center().y, avail.center().y];
            let (lo, hi) = (
                ys.iter().cloned().fold(f32::MAX, f32::min),
                ys.iter().cloned().fold(f32::MIN, f32::max),
            );
            assert!(
                hi - lo < 1.0,
                "{w} 宽时三格不在同一行（y 差 {}）：{ys:?}",
                hi - lo
            );

            // 断言二：整行居中。取与这三格同一条基线的全部文字（含色点旁的
            // 数字），左右边界的中点应当落在内容区中心。
            // 卡片左右内边距各 18，而 CentralPanel 的内容区中心正好是 W/2。
            let base = clean.center().y;
            let band: Vec<&egui::Rect> = rects
                .iter()
                .filter(|(_, r)| (r.center().y - base).abs() < 10.0)
                .map(|(_, r)| r)
                .collect();
            let left = band.iter().map(|r| r.left()).fold(f32::MAX, f32::min);
            let right = band.iter().map(|r| r.right()).fold(f32::MIN, f32::max);
            let mid = (left + right) / 2.0;
            assert!(
                (mid - w / 2.0).abs() < 8.0,
                "{w} 宽时图例没居中：行中点 {mid}，应为 {}（left {left}, right {right}）",
                w / 2.0
            );
        }
    }

    fn collect_text_rects(
        shapes: &[egui::epaint::ClippedShape],
        out: &mut Vec<(String, egui::Rect)>,
    ) {
        for cs in shapes {
            walk_text_rect(&cs.shape, out);
        }
    }

    fn walk_text_rect(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                let size = t.galley.size();
                out.push((
                    t.galley.text().to_string(),
                    egui::Rect::from_min_size(t.pos, size),
                ));
            }
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    walk_text_rect(s, out);
                }
            }
            _ => {}
        }
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

    /// 清理进行中：环心必须写进度，环上必须有进度弧。
    #[test]
    fn cleaning_shows_progress_in_the_ring() {
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
            "清理中圆心要写「正在清理」：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains('%')),
            "清理中圆心要写百分比，让用户看到它在动：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("24 项")),
            "圆心要写这次一共清多少项：{dump:#?}"
        );
        // 清完之后不再显示「还剩」——那一格已经被结果占了
        assert!(
            !dump.iter().any(|l| l == "还剩"),
            "清理中不该同时写「还剩」：{dump:#?}"
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
        let mut dump = painted_text(&ctx, &mut state, &Palette::light());

        // 首屏是仪表盘，不是清单。三件事必须一眼都在：
        // 还剩多少（圆心）、能清多少（图例 + 按钮）、下一步点哪（大按钮）。
        assert!(
            dump.iter().any(|l| l.contains("还剩")),
            "圆心要写清还剩多少：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可清理")),
            "图例要有可清理那一档：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("放入临时空间")),
            "首页的主操作要直接写出能清多少：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("7 天可还原")),
            "主按钮必须说清可还原：{dump:#?}"
        );
        // 两种清理方式必须同时出现：能反悔的那条，和真的腾出空间的那条。
        assert!(
            dump.iter().any(|l| l.contains("直接清理掉")),
            "首页要同时给出直接清理：{dump:#?}"
        );
        // 清单默认收起，所以条目本身此时不该出现。
        assert!(
            !dump.iter().any(|l| l.contains("cargo build 的输出目录")),
            "首屏不该铺开清单：{dump:#?}"
        );

        // 点开之后才是分档清单。
        state.list_expanded = true;
        dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("建议清掉")),
            "必须有一档写清「建议清掉」：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可以直接清")),
            "必须有可回收那一段：{dump:#?}"
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
        state.list_expanded = true;
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
        state.list_expanded = true;
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
        state.list_expanded = true;
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        // 这份 state 里已经有解释，按钮因此写「重新解释」——两种文案都算入口在。
        assert!(
            dump.iter()
                .any(|l| l.contains("解释") && l.contains("AI") || l == "重新解释"),
            "清单里要保留 AI 解释入口：{dump:#?}"
        );
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
