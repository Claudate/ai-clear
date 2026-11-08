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

/// 一个分组选框的三种状态（外加一种「这一组根本不能选」）。
///
/// 组标题上的框如果只画勾/不勾，用户看不出「这组里我只勾了三成」——
/// 全选和部分选中会长得一模一样。中间那一档必须有一条横杠。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriState {
    /// 一个都没勾
    Unchecked,
    /// 勾了一部分
    Partial,
    /// 全勾上了
    Checked,
    /// 这一组不可选（受保护），框只做状态展示
    Locked,
}

impl TriState {
    /// 点一下之后该变成什么：没全选就全选，已全选就清空。
    pub fn toggled(self) -> TriState {
        match self {
            TriState::Checked => TriState::Unchecked,
            _ => TriState::Checked,
        }
    }

    pub fn is_checked(self) -> bool {
        matches!(self, TriState::Checked)
    }
}

/// 组级三态框。与条目上的框同一套尺寸和配色，只是多一条「部分选中」的横杠。
///
/// 返回的 `Response` 只用于无障碍与点击判定；它不参与布局，调用方自己决定放在哪。
pub fn tri_state(
    ui: &mut egui::Ui,
    p: &Palette,
    state: TriState,
    interactive: bool,
    name: &str,
) -> egui::Response {
    // 点击区域比图形大一圈：组标题这一行文字很小，逼用户精确点到方框上是刁难。
    let hit = p.sz(26.0);
    let enabled = interactive && state != TriState::Locked;
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(hit, hit), sense);
    let d = p.sz(17.0);
    let box_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(d, d));
    paint_tri_state(ui, p, box_rect, state, enabled);

    let checked = state.is_checked();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, checked, name));
    resp
}

/// 在指定矩形里画三态框。
fn paint_tri_state(
    ui: &mut egui::Ui,
    p: &Palette,
    rect: egui::Rect,
    state: TriState,
    interactive: bool,
) {
    match state {
        TriState::Unchecked | TriState::Locked => {
            checkbox_at(ui, p, rect, false, interactive, interactive);
        }
        TriState::Checked => checkbox_at(ui, p, rect, true, true, interactive),
        TriState::Partial => {
            // 中间态只画横杠，不能在横杠背后留下「已全选」的对勾。
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(4), p.reclaimable);
            let c = rect.center();
            ui.painter().rect_filled(
                egui::Rect::from_center_size(c, egui::vec2(p.sz(8.4), p.sz(2.0))),
                egui::CornerRadius::same(1),
                p.on_accent(),
            );
        }
    }
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
