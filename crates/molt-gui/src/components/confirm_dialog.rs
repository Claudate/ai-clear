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
    /// 返回 `true` 表示用户确认了。点遮罩或按 Esc 不算确认，也不算取消——
    /// 调用方自己决定这种情况怎么处理（通常是关掉）。
    pub fn show(
        &self,
        ui: &mut egui::Ui,
        p: &Palette,
        body: impl FnOnce(&mut egui::Ui),
    ) -> ConfirmOutcome {
        let danger = self.kind == ConfirmKind::Danger;
        let accent = if danger { p.danger } else { p.reclaimable };
        let mut outcome = ConfirmOutcome::Open;

        egui::Modal::new(egui::Id::unique(self.id))
            .backdrop_color(egui::Color32::from_black_alpha(80))
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
                // 确认在前、取消在后：动作按钮靠左，与全站「主操作在左」一致。
                ui.horizontal(|ui| {
                    if danger {
                        if crate::widgets::danger_button(ui, p, &self.confirm_label, true) {
                            outcome = ConfirmOutcome::Confirmed;
                        }
                    } else if crate::widgets::primary_button(ui, p, &self.confirm_label, true) {
                        outcome = ConfirmOutcome::Confirmed;
                    }
                    if crate::widgets::secondary_button(ui, p, &self.cancel_label, true) {
                        outcome = ConfirmOutcome::Cancelled;
                    }
                });
            });

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
/// 首页与隔离区共用这一份；差别只在尾部那句汇总，由调用方给文案。
pub fn item_list(
    ui: &mut egui::Ui,
    p: &Palette,
    danger: bool,
    rows: &[(String, String)],
    footer: Option<String>,
) {
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
