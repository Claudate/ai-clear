//! 窗口主体。放在库里而不是 `main.rs` 里，是为了让验证工具
//! 能构造同一份界面并把它渲染成图片——验证用的画面和用户看到的必须是同一份代码。

use eframe::egui;

use crate::state::{Activity, AppState, Page, StatusKind, StatusMessage};
use crate::theme::Palette;

/// 整个窗口。
pub struct MoltApp {
    pub theme: crate::theme::Theme,
    pub state: AppState,
}

impl MoltApp {
    /// 用当前系统环境构造一个可以直接跑的窗口。
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theme::install_fonts(&cc.egui_ctx);
        let theme = crate::theme::Theme::install(&cc.egui_ctx);

        let home = crate::paths::home_dir().to_string_lossy().to_string();
        let mut state = AppState::new(home.clone());

        match molt_rules::load(&home, Some(&molt_act::paths::user_rules_dir())) {
            Ok(r) => state.rules = Some(r),
            Err(e) => state.error = Some(format!("规则库加载失败：{e}")),
        }

        state.refresh_disk();

        crate::pages::history::reload(&mut state);
        crate::pages::quarantine::reload(&mut state);
        // 启动时排空已到期的隔离项
        crate::pages::quarantine::purge_due_on_start(&mut state);

        let leftovers = molt_act::paths::legacy_leftovers();
        if !leftovers.is_empty() {
            state.status = Some(StatusMessage::warn(format!(
                "检测到旧版本数据 {} 项尚未迁移（位于 app.shou）。\
                 系统拒绝了自动搬运，请把该目录内容手动移到 app.molt 下。",
                leftovers.len()
            )));
        }

        // 打开就开始扫：让用户一进来就有东西看，而不是面对一个空页面等按钮
        crate::pages::workbench::start_scan(&mut state);

        Self { theme, state }
    }

    /// 不做任何 I/O 的构造：只装主题，状态由调用方填。
    pub fn headless(ctx: &egui::Context, mode: crate::theme::Mode, state: AppState) -> Self {
        crate::theme::install_fonts(ctx);
        ctx.set_theme(match mode {
            crate::theme::Mode::Light => egui::ThemePreference::Light,
            crate::theme::Mode::Dark => egui::ThemePreference::Dark,
        });
        let mut theme = crate::theme::Theme::install(ctx);
        theme.mode = mode;
        theme.palette = crate::theme::Palette::of(mode);
        theme.follow_system = false;
        crate::theme::reinstall(ctx, &theme);
        Self { theme, state }
    }

    /// 只画一帧。窗口与验证工具都走它。
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.theme.sync_system(ui.ctx());
        self.pump(ui.ctx());

        let p = self.theme.palette;

        top_bar(ui, &mut self.state, &self.theme, &p);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(28, 18)),
            )
            .show(ui, |ui| {
                // 内容自适应窗口宽度：宽窗口下卡片/列表要撑满可用宽度，
                // 只留一个上限，避免超宽屏上一行拉得过长。
                let max_w = 1600.0 * p.scale;
                let w = ui.available_width().min(max_w);
                ui.allocate_ui_with_layout(
                    egui::vec2(w, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_width(w);
                        match self.state.page {
                            Page::Workbench => {
                                crate::pages::workbench::show(ui, &mut self.state, &p)
                            }
                            Page::Quarantine => {
                                crate::pages::quarantine::show(ui, &mut self.state, &p)
                            }
                            Page::History => crate::pages::history::show(ui, &mut self.state, &p),
                            Page::Automation => {
                                crate::pages::automation::show(ui, &mut self.state, &p)
                            }
                            Page::Settings => crate::pages::settings::show(ui, &mut self.state, &p),
                        }
                        // 确认框画在滚动区之外：滚动时它不该跟着跑。
                        crate::pages::workbench::confirm_modal(ui, &mut self.state, &p);
                        crate::pages::quarantine::confirm_purge_modal(ui, &mut self.state, &p);
                    },
                );
            });

        // 底部状态条：所有「刚刚发生了什么」都从这里出去
        if let Some(msg) = self.state.status.clone() {
            egui::Panel::bottom("status")
                .resizable(false)
                .frame(
                    egui::Frame::NONE
                        .fill(p.surface_alt)
                        .inner_margin(egui::Margin::symmetric(24, 7)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        crate::widgets::status_bar(ui, &p, msg.kind, &msg.text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::widgets::secondary_button(ui, &p, "关闭", true) {
                                self.state.status = None;
                            }
                        });
                    });
                });
        }
    }

    /// 每帧推进所有后台任务。状态改动只发生在这里，界面只读状态。
    fn pump(&mut self, ctx: &egui::Context) {
        if self.state.request_scale_toggle {
            self.state.request_scale_toggle = false;
            self.theme.toggle_scale(ctx);
        }

        crate::pages::workbench::pump_scan(&mut self.state);
        crate::pages::workbench::pump_clean(&mut self.state);
        crate::pages::workbench::pump_ai(&mut self.state);
        crate::pages::quarantine::pump_purge(&mut self.state);
        crate::pages::automation::pump(&mut self.state);

        // 有任何动作在跑就持续重绘：否则进度条与「正在请求模型…」会停住
        if self.state.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
}

impl eframe::App for MoltApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

/// 顶部：品牌 + 五个页面 + 当前活动 + 字号开关。
fn top_bar(ui: &mut egui::Ui, state: &mut AppState, theme: &crate::theme::Theme, p: &Palette) {
    egui::Panel::top("top")
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin::symmetric(24, 10)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(egui::RichText::new("蜕 · Molt").size(p.sz(15.0)).strong());
                    ui.label(
                        egui::RichText::new("清掉能自己长回来的，留下该留的")
                            .size(p.sz(10.0))
                            .color(p.text_faint),
                    );
                });

                ui.add_space(26.0);

                for page in Page::ALL {
                    let selected = state.page == page;
                    let label = match page {
                        // 固定入口上带数量：用户不必切页就知道里面有没有东西
                        Page::Quarantine => match state
                            .quarantine
                            .iter()
                            .filter(|r| r.restorable)
                            .count()
                        {
                            0 => page.title().to_string(),
                            n => format!("{} {n}", page.title()),
                        },
                        Page::Automation if state.auto.config.enabled => {
                            format!("{} ·开", page.title())
                        }
                        _ => page.title().to_string(),
                    };
                    let text = egui::RichText::new(label)
                        .size(p.sz(12.5))
                        .color(if selected { p.text } else { p.text_dim });
                    let btn = egui::Button::new(text)
                        .frame(false)
                        .min_size(egui::vec2(p.sz(62.0), p.sz(34.0)));
                    let resp = ui.add(btn);
                    if selected {
                        let r = resp.rect;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(r.left() + p.sz(10.0), r.bottom() - p.sz(3.0)),
                                egui::vec2((r.width() - p.sz(20.0)).max(4.0), p.sz(2.0)),
                            ),
                            egui::CornerRadius::same(1),
                            p.reclaimable,
                        );
                    }
                    if resp.clicked() {
                        state.page = page;
                        match page {
                            Page::Quarantine => crate::pages::quarantine::reload(state),
                            Page::History => crate::pages::history::reload(state),
                            Page::Automation => {
                                state.auto.loaded = false;
                                crate::pages::automation::ensure_loaded(state);
                            }
                            Page::Settings => state.ai.reload_config(),
                            Page::Workbench => {}
                        }
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if theme.scale > 1.0 {
                        "标准字号"
                    } else {
                        "放大字号"
                    };
                    if crate::widgets::secondary_button(ui, p, label, true) {
                        state.request_scale_toggle = true;
                    }
                    activity_status(ui, state, p);
                });
            });

            // 底部细线把顶栏和内容分开
            let r = ui.max_rect();
            ui.painter().hline(
                r.left()..=r.right(),
                r.bottom() + p.sz(9.0),
                egui::Stroke::new(1.0, p.border),
            );
        });
}

/// 顶栏右侧：现在在做什么，或者上次扫描的结果。
///
/// 互斥的可视化在这里：只要有动作在跑，顶栏就一直显示它，
/// 用户不用去猜「为什么按钮都灰了」。
fn activity_status(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    let act = state.activity();
    if let Some(label) = act.label() {
        let detail = match act {
            Activity::Scanning => state.job.as_ref().and_then(|j| match j.peek() {
                crate::worker::Progress::Judging { done, total } if *total > 0 => {
                    Some(format!("{done}/{total}"))
                }
                crate::worker::Progress::Walking { found } if *found > 0 => {
                    Some(format!("已找到 {found} 项"))
                }
                _ => None,
            }),
            Activity::Cleaning => state.clean_job.as_ref().and_then(|j| {
                match j.peek() {
                    crate::worker::CleanProgress::Working { done, total } => {
                        Some(format!("{done}/{total}"))
                    }
                    _ => None,
                }
            }),
            _ => None,
        };
        let text = match detail {
            Some(d) => format!("{label} {d}"),
            None => label.to_string(),
        };
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(p.sz(10.5)).color(p.costly))
                .wrap_mode(egui::TextWrapMode::Truncate),
        );
        // 一个小圆点在闪：文字是静态的，动起来才像「还在跑」
        ui.add_space(2.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(p.sz(7.0), p.sz(7.0)), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), p.sz(3.5), p.costly);
        return;
    }

    if let Some(err) = &state.error {
        ui.add(
            egui::Label::new(
                egui::RichText::new(short(err, 26))
                    .size(p.sz(10.5))
                    .color(p.danger),
            )
            .wrap_mode(egui::TextWrapMode::Truncate),
        );
        return;
    }
    if let Some(r) = &state.result {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "上次扫描 {} 项 · {:.0} 秒",
                    r.items.len(),
                    r.elapsed_secs
                ))
                .size(p.sz(10.5))
                .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Extend),
        );
    }
}

/// 只取末尾 `n` 个字，用于顶栏这种放不下完整错误的地方。
pub fn short(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= n {
        return text.to_string();
    }
    format!("…{}", chars[chars.len() - n..].iter().collect::<String>())
}

#[allow(dead_code)]
fn _status_kinds() -> [StatusKind; 3] {
    [StatusKind::Info, StatusKind::Warn, StatusKind::Error]
}
