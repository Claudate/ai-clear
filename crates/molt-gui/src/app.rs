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
        self.tick_clean_anim(ui.ctx());

        let p = self.theme.palette;

        top_bar(ui, &mut self.state, &p);

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
                        // 确认框与提示弹窗画在滚动区之外：滚动时它们不该跟着跑。
                        crate::pages::workbench::confirm_modal(ui, &mut self.state, &p);
                        crate::pages::quarantine::confirm_purge_modal(ui, &mut self.state, &p);
                    },
                );
                status_modal(ui, &mut self.state, &p);
            });
    }

    /// 推进首页圆环的清理动画。
    ///
    /// 真实清理进度拿不到逐项回调（执行器是一次性跑完的），所以这里用
    /// 「按下确认 → 收尾」这段真实时长驱动一条逐渐逼近 92% 的进度：
    /// 它不会提前跑满然后卡住，也不会假装比真实更快。收尾那一刻直接跑满、
    /// 满绿停留 1.6 秒再回到磁盘读数——「清完了」与「清到一半」在环上完全不同。
    fn tick_clean_anim(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let running = self.state.is_cleaning();
        let st = &mut self.state;
        if running {
            // 渐近：每帧补上剩余差距的一小部分，越接近越慢。
            // 停在 92% 而不是 100%——剩下那一段留给真正结束的那一刻。
            let gap = 0.92 - st.clean_anim;
            if gap > 0.0 {
                st.clean_anim += gap * (dt * 2.2).min(1.0);
            }
            st.clean_done_secs = 0.0;
        } else if st.clean_done_secs > 0.0 {
            st.clean_done_secs = (st.clean_done_secs - dt).max(0.0);
            st.clean_anim = 1.0;
            if st.clean_done_secs <= 0.0 {
                // 庆祝结束：环回到磁盘读数
                st.clean_anim = 0.0;
            }
        } else if st.clean_was_running && st.clean_anim > 0.0 {
            // 上一帧还在清、这一帧不在了，也没有结果：被中断或失败了。
            // 直接把弧收回去，不做淡出——残留的弧会被读成「还在清」。
            st.clean_anim = 0.0;
        }
        st.clean_was_running = running;
        if st.clean_anim > 0.0 || st.clean_done_secs > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// 每帧推进所有后台任务。状态改动只发生在这里，界面只读状态。
    fn pump(&mut self, ctx: &egui::Context) {
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

/// 顶部：品牌一行，菜单单独一行。
///
/// 菜单不再和品牌、活动状态挤在同一行——旧版那一行里同时有品牌、五个页签
/// 和一个会变长的「正在扫描 12/48」，窗口一窄就互相压。拆成两行之后，
/// 每个页签之间的间距可以真正拉开，字号放大到 1.4 档也不会贴在一起。
fn top_bar(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    egui::Panel::top("top")
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin::symmetric(28, 12)),
        )
        .show(ui, |ui| {
            // ---- 第一行：品牌 + 右侧活动状态 ----
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = p.sz(3.0);
                    ui.label(
                        egui::RichText::new("净灵 · Cleanling")
                            .size(p.sz(17.0))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("清掉能自己长回来的，留下该留的")
                            .size(p.sz(10.5))
                            .color(p.text_faint),
                    );
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    activity_status(ui, state, p);
                });
            });

            ui.add_space(p.sz(12.0));

            // ---- 第二行：菜单。左右各留 14 点内边距，项间距 6 点。----
            egui::Frame::NONE
                .fill(p.surface_alt.gamma_multiply(0.7))
                .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_PILL))
                .inner_margin(egui::Margin::symmetric(6, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = p.sz(2.0);
                        for page in Page::ALL {
                            let selected = state.page == page;
                            let label = match page {
                                // 固定入口上带数量：用户不必切页就知道里面有没有东西
                                Page::Quarantine => {
                                    match state.quarantine.iter().filter(|r| r.restorable).count() {
                                        0 => page.title().to_string(),
                                        n => format!("{} {n}", page.title()),
                                    }
                                }
                                Page::Automation if state.auto.config.enabled => {
                                    format!("{} ·开", page.title())
                                }
                                // 历史会一直累积，页签上带数量，用户不必切页就知道有没有东西。
                                Page::History if !state.history.is_empty() => {
                                    format!("{} {}", page.title(), state.history.len())
                                }
                                _ => page.title().to_string(),
                            };
                            let text = egui::RichText::new(&label)
                                .size(p.sz(13.0))
                                .color(if selected { p.text } else { p.text_dim });
                            // 宽度按文字实际宽度加内边距算，不写死：
                            // 文案本身（「临时空间 3」「历史 12」）长短不一，
                            // 固定宽度在字号 1.4 下会把长标签压成两行；
                            // 而只给 0 宽度的按钮会让五个页签贴成「清理临时空间历史…」一串。
                            let text_w = ui
                                .painter()
                                .layout_no_wrap(
                                    label.clone(),
                                    egui::FontId::proportional(p.sz(13.0)),
                                    p.text,
                                )
                                .size()
                                .x;
                            let pad = p.sz(15.0);
                            let mut btn = egui::Button::new(text)
                                .frame(false)
                                .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_PILL))
                                .min_size(egui::vec2(text_w + pad * 2.0, p.sz(32.0)));
                            if selected {
                                btn = btn.fill(p.surface);
                            }
                            // 页签之间额外的呼吸量：只有这一处能加，多了就散
                            ui.add_space(p.sz(2.0));
                            if ui.add(btn).clicked() {
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
                    });
                });

            // 底部细线把顶栏和内容分开
            let r = ui.max_rect();
            ui.painter().hline(
                r.left()..=r.right(),
                r.bottom() + p.sz(11.0),
                egui::Stroke::new(1.0, p.border),
            );
        });
}

/// 居中弹窗：报错与提示从这里出去。
///
/// 底部状态条已经撤掉：它常驻、容易被忽略，字号小的用户根本不会看那个角落。
/// 弹窗会打断操作，所以只用于两类事：**失败**与**刚落幕的一次动作**——
/// 前者必须知道，后者刚发生完，此刻看见不碍事。浏览时的普通提示不再弹。
fn status_modal(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let Some(msg) = state.status.clone() else {
        return;
    };
    // 结果卡（`op_report`）已经在首页/临时空间页写了同一句结论，此时 Info 不再弹窗——
    // 否则「清完 24 项」会同时出现在卡片和弹窗里，用户还得关一次。
    // 但只有在那两页上卡片才真的画出来；别的页上没有卡片，Info 就得弹。
    // 出错和警告一律弹：失败必须被看见，不能只留在页面上。
    let card_on_screen =
        matches!(state.page, Page::Workbench | Page::Quarantine) && state.last_op.is_some();
    if msg.kind == StatusKind::Info && card_on_screen {
        return;
    }
    // 「正在扫描…」「正在还原…」这类进行中的提示不是结果，不能弹窗：
    // 弹出来会盖住界面，而用户这时正等着看进度。进行中的状态由顶栏的
    // 活动文字与页面里的进度条表达，本来就一直看得见。
    if msg.kind == StatusKind::Info && msg.text.ends_with('…') {
        return;
    }
    let (tint, soft, title) = match msg.kind {
        StatusKind::Info => (p.reclaimable, p.accent_soft, "完成"),
        StatusKind::Warn => (p.costly, p.costly_soft, "注意"),
        StatusKind::Error => (p.danger, p.danger_soft, "出错了"),
    };

    let mut close = false;
    egui::Modal::new(egui::Id::unique("cleanling-status"))
        .backdrop_color(egui::Color32::from_black_alpha(70))
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0, tint.gamma_multiply(0.6)))
                .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
                .inner_margin(egui::Margin::same(22)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(p.sz(420.0).min(ui.available_width()));
            ui.horizontal(|ui| {
                crate::widgets::dot(ui, tint);
                ui.add_space(p.sz(6.0));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(title)
                            .size(p.sz(15.0))
                            .strong()
                            .color(tint),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
            ui.add_space(p.sz(8.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&msg.text)
                        .size(p.sz(13.0))
                        .color(p.text),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(p.sz(14.0));
            // 按钮靠右：确认位在右下角，手不用在弹窗里找
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::widgets::primary_button(ui, p, "知道了", true) {
                    close = true;
                }
                if msg.kind == StatusKind::Error
                    && crate::widgets::secondary_button(ui, p, "去设置", true)
                {
                    state.page = Page::Settings;
                    state.ai.reload_config();
                    close = true;
                }
                let _ = soft;
            });
        });

    // Esc / 点遮罩关闭：弹窗不该只能靠一个小按钮关掉
    if close || ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        state.status = None;
    }
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
            Activity::Cleaning => state.clean_job.as_ref().and_then(|j| match j.peek() {
                crate::worker::CleanProgress::Working { done, total } => {
                    Some(format!("{done}/{total}"))
                }
                _ => None,
            }),
            _ => None,
        };
        let text = match detail {
            Some(d) => format!("{label} {d}"),
            None => label.to_string(),
        };
        // 宽度上限：这个位置和五个页签同处一行，长文案不能把页签挤走。
        // 必须写在 add 之前，否则限的是下一个控件。
        ui.set_max_width(p.sz(180.0));
        ui.add(
            egui::Label::new(egui::RichText::new(text).size(p.sz(10.5)).color(p.costly))
                .wrap_mode(egui::TextWrapMode::Truncate),
        );
        // 一个小圆点在闪：文字是静态的，动起来才像「还在跑」
        ui.add_space(2.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(p.sz(7.0), p.sz(7.0)), egui::Sense::hover());
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

/// 结果条上的一次点击落到状态上：切页、清报告。
///
/// 返回 `true` 表示这份报告已经看完，调用方可以把它清掉。
/// 工作台与隔离区都会用到，所以放在 `app` 里，而不是各页各写一遍。
pub fn apply_report_action(state: &mut AppState, action: crate::widgets::ReportAction) -> bool {
    use crate::widgets::ReportAction;
    match action {
        ReportAction::None => false,
        ReportAction::Dismiss => true,
        ReportAction::Next(next) => {
            match next {
                crate::state::NextStep::Quarantine => {
                    state.page = crate::state::Page::Quarantine;
                    crate::pages::quarantine::reload(state);
                }
                crate::state::NextStep::History => {
                    state.page = crate::state::Page::History;
                    crate::pages::history::reload(state);
                }
                crate::state::NextStep::Retry => {
                    if state.can_start() {
                        crate::pages::workbench::start_scan(state);
                    }
                }
            }
            true
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 画一帧整个窗口，把画出来的文字连同坐标收集回来。
    fn paint(state: AppState, w: f32) -> Vec<(String, egui::Pos2)> {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut app = MoltApp::headless(&ctx, crate::theme::Mode::Light, state);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w, 1600.0),
            )),
            ..Default::default()
        };
        let mut texts = Vec::new();
        // 两帧：弹窗有淡入，第一帧的透明度还是 0
        for _ in 0..2 {
            let raw = raw.clone();
            let mut out = ctx.run_ui(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
            });
            out.textures_delta.clear();
            texts.clear();
            for cs in &out.shapes {
                walk(&cs.shape, &mut texts);
            }
        }
        texts
    }

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

    /// 品牌与窗口标题必须是新名字，旧名不能再出现。
    #[test]
    fn brand_uses_the_new_name() {
        let texts = paint(AppState::new("/Users/me".into()), 1120.0);
        assert!(
            texts.iter().any(|(t, _)| t.contains("净灵")),
            "顶栏要写「净灵」"
        );
        assert!(
            texts.iter().any(|(t, _)| t.contains("Cleanling")),
            "顶栏要写英文名 Cleanling"
        );
        assert!(
            !texts.iter().any(|(t, _)| t.contains("蜕")),
            "旧名不该再出现：{texts:#?}"
        );
    }

    /// 菜单页签之间要有间隔，不能挤成一串。
    #[test]
    fn tabs_are_spaced_out() {
        let texts = paint(AppState::new("/Users/me".into()), 1120.0);
        // 五个页签的标题
        let mut xs: Vec<f32> = Vec::new();
        for page in Page::ALL {
            let title = page.title();
            if let Some((_, pos)) = texts
                .iter()
                .find(|(t, _)| t == title || t.starts_with(title))
            {
                xs.push(pos.x);
            }
        }
        assert_eq!(xs.len(), 5, "五个页签都要画出来：{texts:#?}");
        // 页签文字宽约 30–60 点；间距不到 40 就会读成一串。
        for w in xs.windows(2) {
            let gap = w[1] - w[0];
            assert!(
                gap > 44.0,
                "相邻页签的水平间隔太小（{gap}），会挤在一起：{xs:?}"
            );
        }
        // 菜单必须自成一行：五个页签的 y 应当一致，且与品牌行分开。
        let brand_y = texts
            .iter()
            .find(|(t, _)| t.contains("净灵"))
            .map(|(_, p)| p.y)
            .expect("要有品牌行");
        let tab_y = texts
            .iter()
            .find(|(t, _)| t == "清理")
            .map(|(_, p)| p.y)
            .expect("要有菜单行");
        assert!(
            tab_y - brand_y > 20.0,
            "菜单要在品牌下面单独一行：品牌 {brand_y}，菜单 {tab_y}"
        );
    }

    /// 报错必须弹出来，而不是在底部留一行小字。
    #[test]
    fn errors_pop_up_in_a_modal() {
        let mut state = AppState::new("/Users/me".into());
        state.status = Some(StatusMessage::error(
            "清理没能完成：/Users/me/project/molt/target 被另一个进程占用。",
        ));
        let texts = paint(state, 1120.0);
        assert!(
            texts.iter().any(|(t, _)| t.contains("出错了")),
            "错误弹窗要有标题：{texts:#?}"
        );
        assert!(
            texts.iter().any(|(t, _)| t.contains("被另一个进程占用")),
            "错误原文必须完整显示：{texts:#?}"
        );
        assert!(
            texts.iter().any(|(t, _)| t == "知道了"),
            "弹窗要有一个关闭按钮：{texts:#?}"
        );
    }
}
