//! 自绘的勾选框。

use eframe::egui;

use crate::theme::Palette;

/// 一个勾选框。不用 egui 自带的：那一个的尺寸不跟大字号档走。
///
/// `actionable` 为假表示这一条**不可选**（受保护的东西），此时勾上也是淡的；
/// `interactive` 为假表示整页正在执行动作，只画不给指针反馈。
/// 两者分开，是因为「这条不能选」和「现在不能选」对用户是两件事。
pub fn checkbox(
    ui: &mut egui::Ui,
    p: &Palette,
    checked: bool,
    actionable: bool,
    interactive: bool,
) {
    let d = p.sz(17.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
    checkbox_at(ui, p, rect, checked, actionable, interactive);
}

/// 在**指定矩形**里画一个勾选框，不占布局。
///
/// 列表行常常是「整行可点、框在行内某个固定位置」——这时框本身不参与布局，
/// 调用方已经算好了它该在哪。把画法抽出来，两处就只用一套实现。
pub fn checkbox_at(
    ui: &mut egui::Ui,
    p: &Palette,
    rect: egui::Rect,
    checked: bool,
    actionable: bool,
    interactive: bool,
) {
    let r = egui::CornerRadius::same(4);

    if checked {
        let bg = if actionable {
            p.reclaimable
        } else {
            p.reclaimable.gamma_multiply(0.45)
        };
        ui.painter().rect_filled(rect, r, bg);
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
        let stroke = if interactive && actionable {
            p.border_strong
        } else {
            p.border
        };
        ui.painter().rect_stroke(
            rect,
            r,
            egui::Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
    }
}
