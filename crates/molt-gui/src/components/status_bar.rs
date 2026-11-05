//! 底部提示条。
//!
//! 一条执行结果：贴在窗口底部，不遮罩、不阻断操作，带一个明确的关闭叉号。
//!
//! 为什么不做成居中模态框：执行结果的提示不是需要决策的问题。用户刚按下确认，
//! 他要知道的只是「成了没有、差在哪」，不该被挡在一次必须点掉的对话前面。
//!
//! 组件只吃 props，不认识 `AppState`：调用方把 [`StatusBar`] 组出来，
//! 看 [`StatusBar::show`] 的返回值决定是否清掉自己那份状态。

use eframe::egui;

use crate::theme::{Palette, RADIUS_CARD, RADIUS_CONTROL};
use crate::widgets::{close_button, dot, secondary_button};

/// 提示的三种语气。颜色与标题都由它决定，调用方只说「这是哪种」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    /// 事情做成了。
    Ok,
    /// 做成了但有代价，或者有件事值得知道。
    Warn,
    /// 没做成。必须被看见。
    Error,
}

impl StatusTone {
    /// 这条提示的标题。
    pub fn title(self) -> &'static str {
        match self {
            StatusTone::Ok => "完成",
            StatusTone::Warn => "注意",
            StatusTone::Error => "出错了",
        }
    }

    /// 描边、文字与状态点用的颜色。`soft` 由调用方按需取。
    pub fn tint(self, p: &Palette) -> egui::Color32 {
        match self {
            StatusTone::Ok => p.reclaimable,
            StatusTone::Warn => p.costly,
            StatusTone::Error => p.danger,
        }
    }
}

/// 一条待显示的提示。
#[derive(Clone, Debug)]
pub struct StatusBar {
    pub tone: StatusTone,
    /// 正文。会按宽度折行，不需要调用方自己截断。
    pub text: String,
    /// 右上角可选的第二个动作（出错时的「去设置」）。
    /// `None` 就只有关闭叉号。
    pub action: Option<String>,
}

impl StatusBar {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Ok,
            text: text.into(),
            action: None,
        }
    }

    pub fn warn(text: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Warn,
            text: text.into(),
            action: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            tone: StatusTone::Error,
            text: text.into(),
            action: None,
        }
    }

    /// 挂一个次要动作，例如出错时的「去设置」。
    pub fn with_action(mut self, label: impl Into<String>) -> Self {
        self.action = Some(label.into());
        self
    }

    /// 画出来，返回用户做了什么。
    ///
    /// 走 `Panel::bottom` 占位而不是 `Area` 浮层：浮层会**永久**盖住内容最底部
    /// 那一条——页面滚到底时最后那几十点正好落在条子底下，怎么滚都露不出来。
    /// 占位让内容区让出一条高度，滚到底就能看全。
    ///
    /// 面板底色必须填 `p.bg`，不能用透明：这块面板比 CentralPanel 先占位，
    /// 透明时窗口清屏色会透出来，软件底部横着一条灰带，跟上面脱节。
    pub fn show(&self, ui: &mut egui::Ui, p: &Palette) -> StatusBarOutcome {
        let tint = self.tone.tint(p);
        let mut outcome = StatusBarOutcome::None;

        egui::Panel::bottom("cleanling-status")
            .resizable(false)
            .frame(egui::Frame::NONE.fill(p.bg).inner_margin(egui::Margin {
                left: 28,
                right: 28,
                top: 8,
                bottom: 10,
            }))
            .show(ui, |ui| {
                egui::Frame::NONE
                    .fill(p.surface)
                    .stroke(egui::Stroke::new(1.0, tint.gamma_multiply(0.55)))
                    .corner_radius(egui::CornerRadius::same(RADIUS_CARD))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 18,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(38),
                    })
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // 右侧先摆操作区，再让文字占剩下的。
                        //
                        // 这两个东西是「动作」，跟左边那句结论不是同一种信息。
                        // 顺序上必须先 `right_to_left` 再排文字：反过来的话文字
                        // 宽度先按整行算，长句会一直顶到最右，把操作区挤出去。
                        ui.horizontal_top(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                if close_button(ui, p, "关闭（Esc）") {
                                    outcome = StatusBarOutcome::Dismiss;
                                }
                                if let Some(label) = &self.action {
                                    if secondary_button(ui, p, label, true) {
                                        outcome = StatusBarOutcome::Action;
                                    }
                                    ui.add_space(p.sz(8.0));
                                }
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.horizontal(|ui| {
                                        dot(ui, tint);
                                        ui.add_space(p.sz(6.0));
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(self.tone.title())
                                                    .size(p.sz(13.5))
                                                    .strong()
                                                    .color(tint),
                                            )
                                            .wrap_mode(egui::TextWrapMode::Extend),
                                        );
                                    });
                                    ui.add_space(p.sz(3.0));
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&self.text)
                                                .size(p.sz(12.5))
                                                .color(p.text),
                                        )
                                        .wrap_mode(egui::TextWrapMode::Wrap),
                                    );
                                });
                            });
                        });
                    });
            });

        // Esc 也能关：底部条常驻到最后，不该只能靠一个小小的叉号
        if outcome == StatusBarOutcome::None && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
        {
            outcome = StatusBarOutcome::Dismiss;
        }
        outcome
    }
}

/// [`StatusBar::show`] 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusBarOutcome {
    /// 用户什么也没做。
    None,
    /// 点了叉号或按下 Esc：调用方应清掉这条提示。
    Dismiss,
    /// 点了次要动作。调用方自己决定去哪。
    Action,
}

/// 一个圆角卡片式的提示框，供调用方嵌在自己的区域里。
///
/// 和 [`StatusBar`] 的区别：这个不占窗口底边、不管显示与隐藏的条件，
/// 只是一块画好的框。用于页面内的常驻说明（例如「下面这份结果是什么时候的」）。
pub fn notice_frame(
    ui: &mut egui::Ui,
    p: &Palette,
    tone: StatusTone,
    body: impl FnOnce(&mut egui::Ui),
) {
    let tint = tone.tint(p);
    let soft = match tone {
        StatusTone::Ok => p.accent_soft,
        StatusTone::Warn => p.costly_soft,
        StatusTone::Error => p.danger_soft,
    };
    egui::Frame::NONE
        .fill(soft)
        .stroke(egui::Stroke::new(1.0, tint.gamma_multiply(0.35)))
        .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
        .inner_margin(egui::Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}
