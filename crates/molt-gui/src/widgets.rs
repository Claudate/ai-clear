//! 共用的小部件。三页都用得上，且不该各画一遍的东西放这里。
//!
//! 这里的每个部件都对应一个明确的界面意图，不做「通用容器」：
//! 卡片是「一件事」，徽章是「这条属于哪一类」，主按钮是「下一步就点它」。
//! 不懂电脑的用户靠这些形状判断该看哪儿，所以形状必须一致——
//! 同一种东西在两页长得不一样，他就得重新学一次。

use eframe::egui;

use crate::state::StatusKind;
use crate::theme::{Palette, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL};

/// 一张卡片。整个界面的内容都装在这种白底圆角块里。
///
/// 卡片自带 1 像素描边而不是靠阴影：egui 的阴影在深色模式下几乎看不见，
/// 而描边两侧都能读清「这里到那里是一块」。
///
/// 宽度一律撑满可用宽度。egui 的 `Frame::show` 默认按内容收缩，
/// 结果是一张只有半屏宽的卡片贴在左边、右侧空出一大片——在宽窗口下
/// 看起来像页面没加载完。内容该多宽由内容自己决定，而**块**应当占据整行。
pub fn card<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let w = ui.available_width();
    egui::Frame::NONE
        .fill(p.surface)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(egui::CornerRadius::same(RADIUS_CARD))
        .inner_margin(egui::Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_width(w - 36.0);
            add(ui)
        })
}

/// 页面标题区：一个大标题 + 一句说明 + 可选的行内操作。
///
/// 每页开头都用它，用户换页时位置固定，不用重新找「这页是干什么的」。
pub fn page_header(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    subtitle: &str,
    right: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(title).size(p.sz(22.0)).strong())
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
            if !subtitle.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(subtitle)
                            .size(p.sz(12.5))
                            .color(p.text_dim),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right(ui);
        });
    });
}

/// 一个统计块：大数字 + 小标签。
///
/// 用「数字大、标签小」而不是反过来：用户扫一眼要读到的是体积，
/// 而「可回收 / 有代价 / 受保护」这三个词他看两次就记住了。
pub fn stat(ui: &mut egui::Ui, p: &Palette, label: &str, value: &str, color: egui::Color32) {
    ui.vertical(|ui| {
        // 数字必须整块显示。默认的 label 会按可用宽度折行，
        // 于是在窄窗口 + 放大字号下「516.7 MB」会断成三行，
        // 一个体积数字被拆开读起来比缺字还糟。Extend 让它撑开，
        // 由外层的 horizontal_wrapped 决定是并排还是换行。
        ui.add(
            egui::Label::new(
                egui::RichText::new(value)
                    .size(p.sz(27.0))
                    .strong()
                    .color(color),
            )
            .wrap_mode(egui::TextWrapMode::Extend),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(label)
                    .size(p.sz(11.5))
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Extend),
        );
    });
}

/// 一行说明：左边是键，右边是值。用于「扫描范围：…」这类事实陈列。
pub fn kv(ui: &mut egui::Ui, p: &Palette, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(key)
                .size(p.sz(11.5))
                .color(p.text_faint),
        );
        ui.label(
            egui::RichText::new(value)
                .size(p.sz(11.5))
                .color(p.text_dim),
        );
    });
}

/// 分隔线。比默认的 `ui.separator()` 淡一档，避免页面被横线切碎。
pub fn hairline(ui: &mut egui::Ui, p: &Palette) {
    let rect = ui.available_rect_before_wrap();
    let y = rect.top() + 4.0;
    ui.painter().hline(
        rect.left()..=rect.right(),
        y,
        egui::Stroke::new(1.0, p.border),
    );
    ui.add_space(9.0);
}

/// 可点选的小标签。用于服务/模型这类「从一组里挑一个」的选择。
///
/// 与 `segmented` 的区别：这里选项数量不定、可以换行，
/// 适合「配置里列了哪些模型就显示哪些」这种由数据决定数量的场景。
pub fn chip(ui: &mut egui::Ui, p: &Palette, text: &str, selected: bool) -> bool {
    let font = egui::FontId::proportional(p.sz(11.5));
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        font,
        if selected { p.on_accent() } else { p.text_dim },
    );
    let pad = egui::vec2(p.sz(10.0), p.sz(5.0));
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = resp.hovered();

    ui.painter().rect(
        rect,
        egui::CornerRadius::same(RADIUS_PILL),
        if selected {
            p.reclaimable
        } else if hovered {
            p.surface_hover
        } else {
            p.surface_alt
        },
        egui::Stroke::new(1.0, if selected { p.reclaimable } else { p.border }),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(rect.min + pad, galley, p.text_dim);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// 小徽章：判定、来源这类「这条属于哪一类」的信息。
pub fn badge(ui: &mut egui::Ui, p: &Palette, text: &str, fg: egui::Color32, bg: egui::Color32) {
    let font = egui::FontId::proportional(p.sz(10.5));
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let pad = egui::vec2(p.sz(7.0), p.sz(2.5));
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(RADIUS_PILL), bg);
    ui.painter().galley(rect.min + pad, galley, fg);
}

// 底部状态条已被弹窗取代（见 `app::status_modal`）：结果与报错一律弹出来，
// 看过即关，不再在窗口底部留一条常驻、容易被忽略的细线。
// 操作收尾的**结果**仍然走下面的 `op_report`（首页那张报告卡），
// 因为那里要列逐项失败原因，一张居中的弹窗放不下长清单。

/// 用户对结果条做了什么。
///
/// 两个调用方都需要知道「只是关掉」与「要去另一页」的区别：前者清报告就完事，
/// 后者要先切页、再清报告。用一个枚举表达，比在 bool 上叠一个 Option 清楚。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportAction {
    /// 还没动
    None,
    /// 点「知道了」
    Dismiss,
    /// 点了下一步
    Next(crate::state::NextStep),
}

/// 一次操作的收尾：结论 + **逐项失败明细** + 下一步。
///
/// 底部状态条只写一句话，放不下「哪几项没成」。而这条要求是硬的：
/// 删不掉的项必须说清原因，且不能把失败画成成功。所以有失败时，
/// 这里把每一条路径和原因都列出来——用户刚看着清单按下确认，
/// 有权知道具体是哪一项没动、为什么。
pub fn op_report(
    ui: &mut egui::Ui,
    p: &Palette,
    op: Option<&crate::state::OpReport>,
) -> ReportAction {
    next_button(ui, p, op).unwrap_or(ReportAction::None)
}

/// 结果条本身。返回用户点的是哪一下。
fn next_button(
    ui: &mut egui::Ui,
    p: &Palette,
    op: Option<&crate::state::OpReport>,
) -> Option<ReportAction> {
    let op = op?;
    if op.done == 0 && op.failures.is_empty() {
        return None;
    }

    let (tint, soft) = match op.kind {
        StatusKind::Info => (p.reclaimable, p.accent_soft),
        StatusKind::Warn => (p.costly, p.costly_soft),
        StatusKind::Error => (p.danger, p.danger_soft),
    };
    let mut action = ReportAction::None;

    egui::Frame::NONE
        .fill(soft)
        .stroke(egui::Stroke::new(1.0, tint.gamma_multiply(0.35)))
        .corner_radius(egui::CornerRadius::same(RADIUS_CARD))
        .inner_margin(egui::Margin::symmetric(16, 13))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                dot(ui, tint);
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&op.summary)
                            .size(p.sz(12.5))
                            .strong()
                            .color(tint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if secondary_button(ui, p, "知道了", true) {
                        action = ReportAction::Dismiss;
                    }
                    // 「下一步」比「知道了」更该被点：做成之后用户要的是接着走，
                    // 不是把提示关掉。所以它放在关闭按钮左边、用主按钮样式。
                    if let Some(next) = op.next {
                        if primary_button(ui, p, next.label(), true) {
                            action = ReportAction::Next(next);
                        }
                    }
                });
            });

            if op.failures.is_empty() {
                return;
            }

            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{} 项未完成——其余 {} 项已照常处理：",
                        op.failures.len(),
                        op.done
                    ))
                    .size(p.sz(11.0))
                    .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(4.0);
            // 失败明细全部列出，最多 12 条后折成一句：再多就不该在这个位置读了。
            for (path, reason) in op.failures.iter().take(12) {
                ui.horizontal_top(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(4.0, p.sz(12.0)), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(2),
                        tint.gamma_multiply(0.7),
                    );
                    ui.add_space(7.0);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(crate::paths::display_path_str(path))
                                    .size(p.sz(10.5))
                                    .monospace()
                                    .color(p.text_dim),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(reason).size(p.sz(10.5)).color(tint),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                    });
                });
                ui.add_space(3.0);
            }
            if op.failures.len() > 12 {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "还有 {} 项失败未列出",
                            op.failures.len() - 12
                        ))
                        .size(p.sz(10.5))
                        .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });

    if action == ReportAction::None {
        None
    } else {
        Some(action)
    }
}

/// 空 / 加载 / 出错状态的统一外壳。
///
/// 三种状态长得一样、只有内容不同，是有意的：用户只需要学会一种形状，
/// 「这里现在没有内容」和「这里正在准备内容」在版面上就应该连着。
/// 图标位用一个圆，里面放一个字符标记——不用 emoji，emoji 在两种主题下
/// 的观感完全不可控。
pub fn state_block(
    ui: &mut egui::Ui,
    p: &Palette,
    mark: &str,
    title: &str,
    hint: &str,
    tint: egui::Color32,
) {
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        let d = p.sz(44.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), d / 2.0, tint.gamma_multiply(0.13));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            mark,
            egui::FontId::proportional(p.sz(18.0)),
            tint,
        );
        ui.add_space(12.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(title)
                    .size(p.sz(15.0))
                    .strong()
                    .color(p.text),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(4.0);
        // 说明文字限宽：不懂电脑的用户怕长句子，一行太长会直接跳过
        ui.set_max_width(ui.available_width().min(p.sz(420.0)));
        ui.add(
            egui::Label::new(egui::RichText::new(hint).size(p.sz(12.0)).color(p.text_dim))
                .wrap_mode(egui::TextWrapMode::Wrap),
        );
    });
    ui.add_space(24.0);
}

/// 主按钮：深底白字或浅底深字，永远保证对比。
///
/// 高度由 `interact_size` 决定，但这里额外抬到 36 点：
/// 这是整页唯一一个「该点这里」的按钮，在触控板与低视力用户手里都要够大。
pub fn primary_button(ui: &mut egui::Ui, p: &Palette, text: &str, enabled: bool) -> bool {
    let mut btn = egui::Button::new(
        egui::RichText::new(text)
            .size(p.sz(13.0))
            .strong()
            .color(if enabled { p.on_accent() } else { p.text_faint }),
    )
    .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
    .min_size(egui::vec2(0.0, p.sz(36.0)));
    if enabled {
        btn = btn.fill(p.reclaimable);
    } else {
        btn = btn
            .fill(p.surface_alt)
            .stroke(egui::Stroke::new(1.0, p.border));
    }
    ui.add_enabled(enabled, btn).clicked()
}

/// 次按钮：描边、透明底。
pub fn secondary_button(ui: &mut egui::Ui, p: &Palette, text: &str, enabled: bool) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(text)
            .size(p.sz(12.5))
            .color(if enabled { p.text } else { p.text_faint }),
    )
    .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
    .min_size(egui::vec2(0.0, p.sz(30.0)))
    .fill(egui::Color32::TRANSPARENT)
    .stroke(egui::Stroke::new(1.0, p.border_strong));
    ui.add_enabled(enabled, btn).clicked()
}

/// 危险按钮：用于不可撤销的动作，只在确认之后出现。
pub fn danger_button(ui: &mut egui::Ui, p: &Palette, text: &str, enabled: bool) -> bool {
    let mut btn = egui::Button::new(egui::RichText::new(text).size(p.sz(13.0)).strong().color(
        if enabled {
            egui::Color32::WHITE
        } else {
            p.text_faint
        },
    ))
    .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
    .min_size(egui::vec2(0.0, p.sz(36.0)));
    if enabled {
        btn = btn.fill(p.danger);
    } else {
        btn = btn
            .fill(p.surface_alt)
            .stroke(egui::Stroke::new(1.0, p.border));
    }
    ui.add_enabled(enabled, btn).clicked()
}

/// 危险描边按钮：表达「这是一个不可撤销的动作」，但不抢主按钮的位置。
///
/// 它和 `danger_button` 的分工：
/// - `danger_button` 只在**确认框里**出现，是「我已经决定要删」的那一次点击
/// - 这个用在**列表底栏**，是「我想走删除这条路」的意图，后面还有一道确认
///
/// 底栏如果直接放红色实心块，它会和主操作一样抢眼，用户就会先点它；
/// 描边 + 危险色足够说明后果，又不会盖过默认路径。
pub fn danger_ghost_button(ui: &mut egui::Ui, p: &Palette, text: &str, enabled: bool) -> bool {
    let fg = if enabled { p.danger } else { p.text_faint };
    let btn = egui::Button::new(egui::RichText::new(text).size(p.sz(12.5)).color(fg))
        .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
        .min_size(egui::vec2(0.0, p.sz(30.0)))
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::new(
            1.0,
            if enabled { p.danger } else { p.border },
        ));
    ui.add_enabled(enabled, btn).clicked()
}

/// 分段控件：几个互斥选项并排，当前项用实心强调色。
///
/// 用它替代一排放着的独立按钮：独立按钮看起来都「可以点」，
/// 用户不知道自己现在在哪一档；分段控件天然表达「这是同一个值」。
pub fn segmented(ui: &mut egui::Ui, p: &Palette, options: &[(&str, bool)]) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::NONE
        .fill(p.surface_alt)
        .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, (label, selected)) in options.iter().enumerate() {
                    // 未选中的项给一层极轻的底色，把相邻文字分开。
                    // 光靠 2 点间距时，「可回收 90」与「有代价 210」会读成一串数字。
                    let text = egui::RichText::new(*label)
                        .size(p.sz(12.0))
                        .color(if *selected { p.text } else { p.text_dim });
                    let mut btn = egui::Button::new(text)
                        .frame(false)
                        .corner_radius(egui::CornerRadius::same(RADIUS_PILL))
                        .min_size(egui::vec2(0.0, p.sz(26.0)));
                    btn = if *selected {
                        btn.fill(p.surface)
                    } else {
                        btn.fill(p.surface_alt.gamma_multiply(0.6))
                    };
                    if i > 0 {
                        // 分段之间补一点水平内距，数字和括号不再背靠背
                        ui.add_space(1.0);
                    }
                    if ui.add(btn).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        });
    clicked
}

/// 一条横向占比条：用已用比例画，超出 90% 变红。
pub fn storage_bar(ui: &mut egui::Ui, p: &Palette, used_ratio: f32) {
    let w = ui.available_width().max(120.0);
    let h = p.sz(9.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let r = egui::CornerRadius::same(5);
    ui.painter().rect_filled(rect, r, p.surface_alt);
    let ratio = used_ratio.clamp(0.0, 1.0);
    if ratio > 0.0 {
        let filled = egui::Rect::from_min_size(
            rect.min,
            egui::vec2((rect.width() * ratio).max(2.0), rect.height()),
        );
        let color = if ratio > 0.90 {
            p.danger
        } else if ratio > 0.80 {
            p.costly
        } else {
            p.reclaimable
        };
        ui.painter().rect_filled(filled, r, color);
    }
}

/// 不确定态的扫描条：一条来回移动的色块。
///
/// 扫描前期拿不到总数（还在遍历目录），此时用进度条会一直停在 0%，
/// 看起来像卡死。动起来比精确更重要。
pub fn busy_bar(ui: &mut egui::Ui, p: &Palette, width: f32) {
    let h = p.sz(4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let r = egui::CornerRadius::same(2);
    ui.painter().rect_filled(rect, r, p.surface_alt);
    let t = ui.input(|i| i.time) as f32;
    let seg_w = (rect.width() * 0.28).max(24.0);
    // 两端停留一下再回头，比匀速来回更像「在处理」而不是「在滚动」
    let phase = ((t * 0.9).sin() + 1.0) / 2.0;
    let x = rect.left() + (rect.width() - seg_w) * phase;
    let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(seg_w, h));
    ui.painter().rect_filled(seg, r, p.reclaimable);
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(40));
}

/// 确定性进度条。
pub fn progress_bar(ui: &mut egui::Ui, p: &Palette, frac: f32, width: f32, label: &str) {
    let h = p.sz(6.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let r = egui::CornerRadius::same(3);
    ui.painter().rect_filled(rect, r, p.surface_alt);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let filled =
            egui::Rect::from_min_size(rect.min, egui::vec2((rect.width() * f).max(3.0), h));
        ui.painter().rect_filled(filled, r, p.reclaimable);
    }
    if !label.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(label)
                .size(p.sz(11.0))
                .color(p.text_dim),
        );
    }
}

/// 把一条长路径折成多行，优先断在 `/` 后面。
///
/// 不用 egui 自带的折行：它按字符宽度硬断，`.../src-tauri/target`
/// 会被切成 `src-` + `tauri/...`，一条路径看起来像两条。
/// 路径自带分隔符，沿它断开既不浪费宽度，也不会把段名切开。
///
/// `measure` 由调用方传入——只有界面知道当前字号下的实际像素宽度。
pub fn wrap_path(text: &str, max_w: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = text;

    while !rest.is_empty() {
        if measure(rest) <= max_w {
            lines.push(rest.to_string());
            break;
        }

        // 能塞进这一行的最长前缀
        let mut fit = 0;
        for (i, ch) in rest.char_indices() {
            let end = i + ch.len_utf8();
            if measure(&rest[..end]) > max_w {
                break;
            }
            fit = end;
        }
        // 连一个字符都放不下（极窄窗口）：硬放一个，否则死循环
        if fit == 0 {
            fit = rest.chars().next().map_or(rest.len(), |c| c.len_utf8());
        }

        // 回退到前缀里最后一个 `/` 之后。`rest[1..]` 里找是为了避开
        // 绝对路径开头那个 `/`——在那里断会切出一行孤零零的斜杠。
        let cut = rest
            .get(1..fit)
            .and_then(|s| s.rfind('/'))
            .map_or(fit, |p| p + 2);
        lines.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }

    lines
}

/// 画一条路径，按 `/` 折行。
/// 折行宽度用当前字号下的真实像素量出来：固定字符数在中文路径上会明显偏窄，
/// 而这套界面允许用户把字号放大。
pub fn path_label(ui: &mut egui::Ui, p: &Palette, path: &str) {
    let font = egui::FontId::monospace(p.sz(11.0));
    let max_w = ui.available_width();
    // 用 painter 量宽度：`ui.fonts` 只给不可变借用，而排版需要可变。
    let painter = ui.painter().clone();
    let lines = wrap_path(path, max_w, |s| {
        painter
            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    });
    for line in lines {
        ui.add(
            egui::Label::new(
                egui::RichText::new(line)
                    .size(p.sz(11.0))
                    .monospace()
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Extend),
        );
    }
}

/// 一段扇形：从 `start` 度开始、顺时针扫过 `sweep` 度。
///
/// 角度以 12 点方向为 0、顺时针为正——这就是钟表的读法，
/// 用户不需要知道数学上的极坐标是哪一套。
pub struct Arc {
    pub start: f32,
    pub sweep: f32,
    pub color: egui::Color32,
}

/// 环形饼图（圆环）。
///
/// 为什么是环而不是实心饼：圆心要写字。磁盘清理工具最该被一眼读到的数字
/// 是「还剩多少」，把字放进圆心，它天然成为整页的视觉重心；
/// 实心饼图只能把数字挂在旁边，读者还要自己找。
///
/// 返回圆心区域，调用方在它上面叠字（`donut_center` 负责排版）。
/// 尺寸完全由调用方给：这里不假设窗口有多宽。
/// 需要在不新占一块版面时叠加动画层（清理进度、扫描头），
/// 用 [`paint_ring`] 在同一个圆心上再画一层。
pub fn donut(
    ui: &mut egui::Ui,
    p: &Palette,
    diameter: f32,
    thickness: f32,
    arcs: &[Arc],
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    let outer = diameter / 2.0;
    let inner = (outer - thickness).max(0.0);
    paint_ring(ui.painter(), p, rect.center(), outer, inner, true, arcs);
    resp
}

/// 把若干扇区画到给定的圆心与内外半径上。
///
/// `base` 为真时先铺一整圈底色——只有最底层需要它；
/// 叠在它上面的动画层不能铺底，否则会把下面的扇区盖掉。
pub fn paint_ring(
    painter: &egui::Painter,
    p: &Palette,
    center: egui::Pos2,
    outer: f32,
    inner: f32,
    base: bool,
    arcs: &[Arc],
) {
    // 底环先整圈铺一层：扇区加总不足 360 度时（例如还没扫描，
    // 「可清理」那一档未知），剩下那段不能是透明的，否则像图没画完。
    if base {
        painter.circle_stroke(
            center,
            (outer + inner) / 2.0,
            egui::Stroke::new(outer - inner, p.surface_alt),
        );
    }

    let mut mesh = egui::Mesh::default();
    // 每段 1.2 度：220px 的环上弧长约 2.3px，配合下面的羽化已经看不出折线。
    const STEP: f32 = 1.2;
    // 边缘羽化 1px。Mesh 是硬边三角形，不像文字那样自带抗锯齿——
    // 不加这一圈 alpha=0 的顶点，环的内外圈会是一圈明显的锯齿。
    const FEATHER: f32 = 1.0;
    let with_alpha = |c: egui::Color32, a: u8| -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
    };

    for arc in arcs {
        if arc.sweep <= 0.0 {
            continue;
        }
        let steps = ((arc.sweep / STEP).ceil() as usize).max(1);
        let solid = arc.color;
        let clear = with_alpha(arc.color, 0);
        for i in 0..steps {
            let a0 = arc.start + arc.sweep * (i as f32 / steps as f32);
            let a1 = arc.start + arc.sweep * ((i + 1) as f32 / steps as f32);
            let at = |deg: f32, r: f32| {
                let (c, s) = (
                    (deg - 90.0).to_radians().cos(),
                    (deg - 90.0).to_radians().sin(),
                );
                center + egui::vec2(c * r, s * r)
            };
            // 四条同心边：内羽化 → 内实体 → 外实体 → 外羽化
            let rings = [
                (inner - FEATHER, clear),
                (inner, solid),
                (outer, solid),
                (outer + FEATHER, clear),
            ];
            for w in rings.windows(2) {
                let (r0, c0) = w[0];
                let (r1, c1) = w[1];
                let base = mesh.vertices.len() as u32;
                for (p, c) in [
                    (at(a0, r0), c0),
                    (at(a0, r1), c1),
                    (at(a1, r1), c1),
                    (at(a1, r0), c0),
                ] {
                    mesh.colored_vertex(p, c);
                }
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
        }
    }
    if !mesh.is_empty() {
        painter.add(egui::Shape::Mesh(mesh.into()));
    }
}

/// 首页那唯一一个「下一步就点它」的大按钮。
///
/// 比 `primary_button` 更高更宽：这是整页唯一的主操作，在触控板上
/// 随手点下去就该命中。宽度由调用方限制，这里只保证高度与字号。
pub fn hero_button(ui: &mut egui::Ui, p: &Palette, text: &str, width: f32, enabled: bool) -> bool {
    let w = width;
    let mut btn = egui::Button::new(
        egui::RichText::new(text)
            .size(p.sz(18.0))
            .strong()
            .color(if enabled { p.on_accent() } else { p.text_faint }),
    )
    .corner_radius(egui::CornerRadius::same(RADIUS_CARD))
    .min_size(egui::vec2(w, p.sz(56.0)));
    btn = if enabled {
        btn.fill(p.reclaimable)
    } else {
        btn.fill(p.surface_alt)
            .stroke(egui::Stroke::new(1.0, p.border))
    };
    ui.add_enabled(enabled, btn).clicked()
}

/// 判定色点。比文字更省地方，也比图标更不容易误读。
pub fn dot(ui: &mut egui::Ui, color: egui::Color32) {
    let d = p_dot();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), d / 2.0, color);
}

/// 一段清单前的分节标题：左边一句「这是什么档」，右边数量与体积。
///
/// 用它替代筛选标签，是因为用户打开界面要读的是「哪些建议清理、哪些留着」，
/// 而不是自己在一排筛选里试。分节标题把这件事直接写进版面。
pub fn section_header(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    hint: &str,
    count: usize,
    bytes: u64,
    tint: egui::Color32,
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, p.sz(16.0)), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(2), tint);
        ui.add_space(p.sz(6.0));
        ui.add(
            egui::Label::new(
                egui::RichText::new(title)
                    .size(p.sz(13.5))
                    .strong()
                    .color(p.text),
            )
            .wrap_mode(egui::TextWrapMode::Extend),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("{count} 项 · {}", molt_scan::human_bytes(bytes)))
                    .size(p.sz(11.0))
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Extend),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(hint)
                        .size(p.sz(10.5))
                        .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Truncate),
            );
        });
    });
    ui.add_space(4.0);
}

/// AI 对一条数据的解释。前面带一个「AI」小标记，与本地判定区分开——
/// 用户必须一眼看出哪句话是模型说的、哪句是规则说的。
pub fn ai_note(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.horizontal_top(|ui| {
        badge(ui, p, "AI", p.costly, p.costly_soft);
        ui.add_space(p.sz(4.0));
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(p.sz(10.5)).color(p.text_dim))
                .wrap_mode(egui::TextWrapMode::Wrap),
        );
    });
}

fn p_dot() -> f32 {
    7.0
}

#[cfg(test)]
mod tests {
    use super::wrap_path;

    /// 等宽兜底：每个字符按固定宽度算，测试才不依赖字体。
    fn fixed(w: f32) -> impl Fn(&str) -> f32 {
        move |s: &str| s.chars().count() as f32 * w
    }

    #[test]
    fn short_path_stays_on_one_line() {
        let lines = wrap_path("~/a/b", 100.0, fixed(1.0));
        assert_eq!(lines, vec!["~/a/b"]);
    }

    #[test]
    fn breaks_after_a_slash_not_inside_a_segment() {
        // 每行最多 20 个字符；路径里最长的一段 moshu-desktop 是 13 个字符，
        // 单独成行放得下，所以它绝不该被切开。
        let lines = wrap_path("~/project/mac/wros/moshu-desktop", 20.0, fixed(1.0));
        assert_eq!(lines, vec!["~/project/mac/wros/", "moshu-desktop"]);
        // 关键：没有任何一行把段名从中间切开
        for l in &lines[..lines.len() - 1] {
            assert!(l.ends_with('/'), "非末行应断在斜杠后：{l}");
        }
    }

    #[test]
    fn a_single_long_segment_is_still_broken() {
        // 没有 `/` 可断时只能硬断，但绝不能返回空行或丢字符
        let lines = wrap_path("abcdefghij", 4.0, fixed(1.0));
        assert_eq!(lines.concat(), "abcdefghij");
        assert!(lines.iter().all(|l| !l.is_empty()));
    }

    #[test]
    fn every_character_survives() {
        let src = "/Users/me/project/mac/ai-clear/crates/molt-gui/src/widgets.rs";
        let lines = wrap_path(src, 20.0, fixed(1.0));
        assert_eq!(lines.concat(), src);
        // 只断在 `/` 之后
        for l in &lines[..lines.len() - 1] {
            assert!(l.ends_with('/'), "非末行应断在斜杠后：{l}");
        }
    }
}
