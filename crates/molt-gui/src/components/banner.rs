//! 页面内的横幅。
//!
//! 框体来自 [`crate::components::status_bar::notice_frame`]，与提示条共用同一套
//! 「浅底 + 同色描边 + 圆角」的画法；这里只剩下文字怎么排。

use eframe::egui;

use crate::components::status_bar::{notice_frame, StatusTone};
use crate::theme::Palette;
use crate::widgets::secondary_button;

/// 一条横幅：一句粗体结论 + 一句细说明 + 一个可选的按钮。
///
/// 首页的「这次扫描失败」用它。`Warn` 语气对应原来的 `costly_soft` 底、
/// `costly×0.35` 描边——这个配对现在只在 `notice_frame` 里定义一次。
pub fn banner(
    ui: &mut egui::Ui,
    p: &Palette,
    bold: &str,
    detail: &str,
    action: Option<&str>,
    enabled: bool,
) -> bool {
    let mut clicked = false;
    notice_frame(ui, p, StatusTone::Warn, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(bold)
                        .size(p.sz(12.0))
                        .strong()
                        .color(p.costly),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            if !detail.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(detail)
                            .size(p.sz(11.0))
                            .color(p.text_dim),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            if let Some(label) = action {
                if secondary_button(ui, p, label, enabled) {
                    clicked = true;
                }
            }
        });
    });
    clicked
}
