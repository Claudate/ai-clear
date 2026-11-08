//! 不可撤销动作的确认框。
//!
//! 两个地方要用它：首页「直接清理」与临时空间「彻底删除」。旧版各写一份，
//! 内边距、标题字号、按钮顺序都是复制的——复制之后必然各自演化，改了一处
//! 另一处就悄悄不一致。这里只剩骨架，具体讲什么由调用方给。

use eframe::egui;

use crate::theme::{Palette, RADIUS_CARD, RADIUS_CONTROL};

/// 这个框是「会毁掉东西」还是「只是要做一件事」。
///
/// 影响标题颜色、描边、以及确认按钮的样式——红色实心按钮和蓝色主按钮
/// 不是审美差异，它们表达「这一下和平时不一样」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    /// 不可恢复。红底、红描边、危险按钮。
    Danger,
    /// 可撤销或者只是确认。正常配色。
    Normal,
}

/// 一个确认框。
pub struct ConfirmDialog<'a> {
    id: &'a str,
    kind: ConfirmKind,
    title: String,
    /// 一句量化（「12 项 · 共 3.2 GB」）。大的那种数字。
    summary: String,
    /// 标题下方的高亮警示（红底小块）。只给真正危险的事。
    warning: Option<(String, String)>,
    /// 正文补充说明。
    note: Option<String>,
    confirm_label: String,
    cancel_label: String,
    width: f32,
}

impl<'a> ConfirmDialog<'a> {
    pub fn new(id: &'a str, kind: ConfirmKind, title: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            title: title.into(),
            summary: String::new(),
            warning: None,
            note: None,
            confirm_label: "确认".to_string(),
            cancel_label: "取消".to_string(),
            width: 440.0,
        }
    }

    /// 那个大数字：「12 项 · 共 3.2 GB」里的后两段。
    pub fn summary(mut self, text: impl Into<String>) -> Self {
        self.summary = text.into();
        self
    }

    /// 红底警示块：粗体一行 + 细一行。
    pub fn warning(mut self, bold: impl Into<String>, detail: impl Into<String>) -> Self {
        self.warning = Some((bold.into(), detail.into()));
        self
    }

    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.note = Some(text.into());
        self
    }

    pub fn confirm_label(mut self, text: impl Into<String>) -> Self {
        self.confirm_label = text.into();
        self
    }

    pub fn cancel_label(mut self, text: impl Into<String>) -> Self {
        self.cancel_label = text.into();
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }

    /// 画出来。`body` 是中间那段自定义内容（对象清单之类）。
    ///
    /// Esc 与点遮罩都返回取消；危险动作必须持续按住确认按钮 1.2 秒。
    pub fn show(
        &self,
        ui: &mut egui::Ui,
        p: &Palette,
        body: impl FnOnce(&mut egui::Ui),
    ) -> ConfirmOutcome {
        let danger = self.kind == ConfirmKind::Danger;
        let accent = if danger { p.danger } else { p.reclaimable };
        let mut outcome = ConfirmOutcome::Open;
        let focus_id = egui::Id::unique((self.id, "initial-cancel-focus"));
        let hold_id = egui::Id::unique((self.id, "danger-hold-start"));

        let modal = egui::Modal::new(egui::Id::unique(self.id))
            .backdrop_color(p.scrim)
            .frame(
                egui::Frame::NONE
                    .fill(p.surface)
                    .stroke(egui::Stroke::new(
                        1.0,
                        if danger { accent } else { p.border },
                    ))
                    .corner_radius(egui::CornerRadius::same(RADIUS_CARD))
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(p.sz(self.width).min(ui.available_width()));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&self.title)
                            .size(p.sz(17.0))
                            .strong()
                            .color(if danger { accent } else { p.text }),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
                ui.add_space(8.0);

                if !self.summary.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&self.summary)
                                    .size(p.sz(22.0))
                                    .strong()
                                    .color(accent),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
                    ui.add_space(10.0);
                }

                if let Some((bold, detail)) = &self.warning {
                    egui::Frame::NONE
                        .fill(p.danger_soft)
                        .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
                        .inner_margin(egui::Margin::symmetric(12, 9))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(bold)
                                        .size(p.sz(12.5))
                                        .strong()
                                        .color(p.danger),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                            ui.add_space(3.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(detail)
                                        .size(p.sz(11.5))
                                        .color(p.text_dim),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    ui.add_space(10.0);
                }

                if let Some(note) = &self.note {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(note).size(p.sz(11.5)).color(p.text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                    ui.add_space(10.0);
                }

                body(ui);

                ui.add_space(18.0);
                // 普通确认保持原有单击路径；不可撤销动作必须真正持续按住。
                ui.horizontal(|ui| {
                    if danger {
                        let button = egui::Button::new(
                            egui::RichText::new(format!("按住 1.2 秒 · {}", self.confirm_label))
                                .size(p.sz(13.0))
                                .strong()
                                .color(p.on_danger),
                        )
                        .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
                        .min_size(egui::vec2(0.0, p.sz(36.0)))
                        .fill(p.danger);
                        let response = ui.add(button);
                        let started = ui.data(|d| d.get_temp::<f64>(hold_id));
                        // egui 的 Button 在持续按住接近一秒后可能停止报告 down-on；
                        // 只对从本按钮起始的按压继续跟踪真实指针，不允许从外部拖进来触发。
                        let pointer_held = response.is_pointer_button_down_on()
                            || (started.is_some()
                                && ui.input(|i| {
                                    i.pointer.primary_down()
                                        && i.pointer
                                            .interact_pos()
                                            .is_some_and(|pos| response.rect.contains(pos))
                                }));
                        let held = pointer_held
                            || (response.has_focus() && ui.input(|i| i.key_down(egui::Key::Space)));
                        if held {
                            let now = ui.input(|i| i.time);
                            let started_at = started.unwrap_or(now);
                            ui.data_mut(|d| d.insert_temp(hold_id, started_at));
                            let fraction = ((now - started_at) / 1.2).clamp(0.0, 1.0) as f32;
                            if fraction >= 1.0 {
                                outcome = ConfirmOutcome::Confirmed;
                                ui.data_mut(|d| d.remove::<f64>(hold_id));
                            } else {
                                let bar = egui::Rect::from_min_size(
                                    response.rect.left_bottom() - egui::vec2(0.0, p.sz(3.0)),
                                    egui::vec2(response.rect.width() * fraction, p.sz(3.0)),
                                );
                                ui.painter().rect_filled(bar, 1.0, p.on_danger);
                                ui.ctx()
                                    .request_repaint_after(std::time::Duration::from_millis(40));
                            }
                        } else {
                            ui.data_mut(|d| d.remove::<f64>(hold_id));
                        }
                    } else if crate::widgets::primary_button(ui, p, &self.confirm_label, true) {
                        outcome = ConfirmOutcome::Confirmed;
                    }
                    let cancel = egui::Button::new(
                        egui::RichText::new(&self.cancel_label)
                            .size(p.sz(12.5))
                            .color(p.text),
                    )
                    .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
                    .min_size(egui::vec2(0.0, p.sz(30.0)))
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::new(1.0, p.border_strong));
                    let response = ui.add(cancel);
                    if !ui.data(|d| d.get_temp::<bool>(focus_id).unwrap_or(false)) {
                        response.request_focus();
                        ui.data_mut(|d| d.insert_temp(focus_id, true));
                    }
                    if response.clicked() {
                        outcome = ConfirmOutcome::Cancelled;
                    }
                });
            });
        if outcome == ConfirmOutcome::Open && modal.should_close() {
            outcome = ConfirmOutcome::Cancelled;
        }
        if outcome != ConfirmOutcome::Open {
            ui.ctx().data_mut(|d| {
                d.remove::<bool>(focus_id);
                d.remove::<f64>(hold_id);
            });
        }
        outcome
    }
}

/// [`ConfirmDialog::show`] 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// 框还开着，用户没做决定。
    Open,
    /// 用户点了确认。
    Confirmed,
    /// 用户点了取消。
    Cancelled,
}

/// 确认框里的对象清单。
///
/// 只给数字是不够的：用户有权在不可撤销之前看见选了什么。
/// 首页与隔离区共用这一份；默认只展示摘要，路径由用户主动展开核对。
pub fn item_list(
    ui: &mut egui::Ui,
    p: &Palette,
    danger: bool,
    rows: &[(String, String)],
    footer: Option<String>,
) {
    let open_id = ui.make_persistent_id("confirm-item-details");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    if crate::widgets::secondary_button(
        ui,
        p,
        if open {
            "收起所选明细"
        } else {
            "查看所选明细"
        },
        true,
    ) {
        open = !open;
        ui.data_mut(|d| d.insert_temp(open_id, open));
    }
    if !open {
        return;
    }
    ui.add_space(6.0);
    egui::Frame::NONE
        .fill(p.surface_alt)
        .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (main, sub) in rows {
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
                                egui::RichText::new(main).size(p.sz(11.0)).monospace(),
                            )
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        if !sub.is_empty() {
                            // 折行而不是截断：这一行常常是 AI 对这条的判断理由，
                            // 截掉半句等于没说。
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(sub).size(p.sz(10.5)).color(p.text_dim),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        }
                    });
                });
            }
            if let Some(text) = footer {
                ui.add_space(3.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text)
                            .size(p.sz(10.5))
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_text(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(t) if t.galley.text().contains(needle) => {
                Some(t.pos + egui::vec2(10.0, 8.0))
            }
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().find_map(|shape| find_text(shape, needle))
            }
            _ => None,
        }
    }

    fn frame(
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (ConfirmOutcome, Vec<egui::epaint::ClippedShape>) {
        let input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 680.0),
            )),
            events,
            ..Default::default()
        };
        let mut outcome = ConfirmOutcome::Open;
        let mut output = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                outcome =
                    ConfirmDialog::new("test-danger", ConfirmKind::Danger, "永久删除这些内容？")
                        .confirm_label("永久删除")
                        .show(ui, &Palette::light(), |_| {});
            });
        });
        output.textures_delta.clear();
        (outcome, output.shapes)
    }

    #[test]
    fn danger_requires_continuous_hold_and_escape_cancels() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        frame(&ctx, 0.0, vec![]);
        let (outcome, shapes) = frame(&ctx, 0.02, vec![]);
        assert_eq!(outcome, ConfirmOutcome::Open);
        let pos = shapes
            .iter()
            .find_map(|s| find_text(&s.shape, "按住 1.2 秒"))
            .expect("确认框必须展示长按按钮");
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        assert_eq!(
            frame(
                &ctx,
                0.1,
                vec![egui::Event::PointerMoved(pos), button(true)]
            )
            .0,
            ConfirmOutcome::Open
        );
        assert_eq!(
            frame(&ctx, 0.4, vec![button(false)]).0,
            ConfirmOutcome::Open,
            "短按释放不能删除"
        );
        assert_eq!(
            frame(&ctx, 2.0, vec![]).0,
            ConfirmOutcome::Open,
            "释放后等待不能累计长按时间"
        );
        assert_eq!(frame(&ctx, 2.2, vec![button(true)]).0, ConfirmOutcome::Open);
        for step in 1..=12 {
            let outcome = frame(
                &ctx,
                2.2 + step as f64 * 0.1,
                vec![egui::Event::PointerMoved(pos)],
            )
            .0;
            if step < 12 {
                assert_eq!(outcome, ConfirmOutcome::Open, "尚未按够 1.2 秒不能确认");
            } else {
                assert_eq!(
                    outcome,
                    ConfirmOutcome::Confirmed,
                    "连续跨帧按住至少 1.2 秒才确认"
                );
            }
        }

        let cancel_ctx = egui::Context::default();
        crate::theme::install_fonts(&cancel_ctx);
        crate::theme::Theme::install(&cancel_ctx);
        frame(&cancel_ctx, 0.0, vec![]);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        assert_eq!(
            frame(&cancel_ctx, 0.1, vec![key(egui::Key::Enter)]).0,
            ConfirmOutcome::Cancelled,
            "默认焦点应在取消按钮"
        );
        let esc_ctx = egui::Context::default();
        crate::theme::install_fonts(&esc_ctx);
        crate::theme::Theme::install(&esc_ctx);
        frame(&esc_ctx, 0.0, vec![]);
        assert_eq!(
            frame(&esc_ctx, 0.1, vec![key(egui::Key::Escape)]).0,
            ConfirmOutcome::Cancelled,
            "Esc 应取消而不是执行删除"
        );
    }
}
