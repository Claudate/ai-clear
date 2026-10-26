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

/// 顶部：只有一行。品牌在左，活动状态与五个图标靠右。
///
/// 上一版是「品牌 + 副标题」一行、五个中文页签再一行，占掉两条栏高，
/// 而首屏真正该先看到的是磁盘数字。现在压成一行：品牌留作身份，
/// 导航换成图标，中文名走悬停提示——五个中文词一旦并排，就等于把
/// 那条被撤掉的菜单又长回来了。
///
/// 图标比文字更依赖形状记忆，所以五个形状刻意选得互不相像（见 `NavIcon`），
/// 并且每页正文顶部本来就有大标题，认错了也能立刻纠正。
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
            // ---- 唯一一行：品牌在左，活动状态与五个图标靠右 ----
            //
            // 图标在右、活动文字在它的左边：`right_to_left` 里先加的贴最右，
            // 所以图标先加、活动文字后加。图标内部再倒序加一遍，视觉顺序才
            // 还是「清理 → 临时空间 → 历史 → 自动清理 → 设置」。
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("净灵 · Cleanling")
                            .size(p.sz(17.0))
                            .strong(),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for page in Page::ALL.iter().rev() {
                        let badge = badge_for(state, *page);
                        let resp = crate::widgets::nav_icon(
                            ui,
                            p,
                            page.icon(),
                            state.page == *page,
                            badge.as_deref(),
                        );
                        // 中文名只在悬停时出现——把它写回这一行，就等于把菜单加回来了
                        if resp.on_hover_text(page.title()).clicked() {
                            go_page(state, *page);
                        }
                        ui.add_space(p.sz(4.0));
                    }
                    ui.add_space(p.sz(12.0));
                    activity_status(ui, state, p);
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

/// 切页并做该页需要的加载。
///
/// 抽出来是为了让测试能走「点图标」这条真路径：副作用留在这里一份，
/// 界面与测试就不会各理解一遍「切到临时空间要重读列表」。
fn go_page(state: &mut AppState, page: Page) {
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

/// 图标右上角的角标：有东西待处理时不必点进去就知道。
///
/// 空串表示「只画一个点」——定时清理开着是个状态，不是一个数量，
/// 用数字反而会被读成「有 0 项」。
fn badge_for(state: &AppState, page: Page) -> Option<String> {
    match page {
        Page::Quarantine => {
            let n = state.quarantine.iter().filter(|r| r.restorable).count();
            (n > 0).then(|| n.to_string())
        }
        Page::History => {
            let n = state.history.len();
            (n > 0).then(|| n.to_string())
        }
        Page::Automation if state.auto.config.enabled => Some(String::new()),
        _ => None,
    }
}
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

    /// 顶栏只剩一行：品牌在左，图标在右；中文菜单与副标题都不该再出现。
    ///
    /// 方案原本写的是「断言没有任何 `Page::title()` 出现在画面文字里」。
    /// 按字面做不到：每页正文顶部都有 `page_header` 的大标题，首页正文写的
    /// 正是「清理」。所以这里限定在**顶栏那一条高度带**里断言——菜单换成
    /// 图标，要守的是「顶栏里不再有中文页名」，不是「全屏不能出现这四个字」。
    #[test]
    fn nav_icons_sit_in_one_row_at_top_right() {
        let texts = paint(AppState::new("/Users/me".into()), 1120.0);

        // 断言一：副标题已删。它原本是品牌名下面那行小字。
        assert!(
            !texts.iter().any(|(t, _)| t.contains("清掉能自己长回来")),
            "副标题必须删掉：{texts:#?}"
        );

        // 断言三：品牌还在，并且在左半边。
        let (_, brand) = texts
            .iter()
            .find(|(t, _)| t.contains("净灵"))
            .expect("品牌要留着");
        assert!(brand.x < 1120.0 * 0.5, "品牌要在左边：x = {}", brand.x);

        // 断言二：顶栏里没有任何中文页名。顶栏只有一行，高度是
        // 上内边距 12 + 图标高 34×1.4 ≈ 60；取品牌基线往下 40 点作为这条带的下沿。
        let bar_bottom = brand.y + 40.0;
        for page in Page::ALL {
            let title = page.title();
            assert!(
                !texts
                    .iter()
                    .any(|(t, pos)| t == title && pos.y < bar_bottom),
                "「{title}」还画在顶栏里——菜单没换成图标：{texts:#?}"
            );
        }
    }

    /// 图标必须真的能点。只断言「画出来了」会漏掉「点了没反应」——
    /// 而那一类问题在截图上完全看不出来。
    #[test]
    fn nav_icon_click_reports_hit() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let p = crate::theme::Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);

        // 第一帧只为拿到图标的矩形；第二帧往它中心送一次完整的按下 + 抬起。
        let rect = std::rc::Rc::new(std::cell::Cell::new(egui::Rect::NOTHING));
        let hit = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 120.0),
            )),
            ..Default::default()
        };

        for frame in 0..3 {
            if frame == 2 {
                let c = rect.get().center();
                let modifiers = egui::Modifiers::default();
                input.events = vec![
                    egui::Event::PointerMoved(c),
                    egui::Event::PointerButton {
                        pos: c,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    },
                    egui::Event::PointerButton {
                        pos: c,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers,
                    },
                ];
            }
            let mut out = ctx.run_ui(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let resp = crate::widgets::nav_icon(ui, &p, Page::Settings.icon(), false, None);
                    rect.set(resp.rect);
                    if resp.clicked() {
                        hit.set(true);
                    }
                });
            });
            out.textures_delta.clear();
        }

        assert!(
            hit.get(),
            "点图标中心必须算一次点击（rect = {:?}）",
            rect.get()
        );
    }

    /// 点图标必须真的切页并触发该页的加载，而不是只改一个字段。
    ///
    /// 方案原本要求「断言 `Page::Automation` 会把 `auto.loaded` 置回 false」。
    /// 按字面断言必挂：`go_page` 里紧跟着调 `ensure_loaded`，它读完配置
    /// 又会把 `loaded` 置回 `true`——`false` 只是那一瞬间的中间态，从外面
    /// 观察不到。所以改成能证明「重读真的发生了」的判据：先把内存里的配置
    /// 塞一个磁盘上不可能有的标记，再进这一页；如果 `loaded = false` 那一步
    /// 被删掉，`ensure_loaded` 会立刻 early-return，标记就会留在内存里。
    #[test]
    fn go_page_switches_and_reloads() {
        let mut state = AppState::new("/Users/me".into());
        for page in Page::ALL {
            go_page(&mut state, page);
            assert_eq!(state.page, page, "切到 {page:?} 之后 page 必须跟着变");
        }

        // 一个绝不会出现在任何真实配置里的规则 id
        const SENTINEL: &str = "__go_page_reload_probe__";
        state.auto.config.rule_ids = vec![SENTINEL.to_string()];
        state.auto.loaded = true; // 内存里这份「已经读过」——不重置就会直接复用
        state.auto.draft = None;

        go_page(&mut state, Page::Automation);

        assert!(
            !state.auto.config.rule_ids.iter().any(|r| r == SENTINEL),
            "进自动清理页必须重读配置；标记还在，说明 loaded=false 那一步丢了"
        );
        assert!(state.auto.loaded, "重读跑完之后 loaded 应回到 true");
        assert!(
            state.auto.draft.is_some(),
            "重读配置后要重建草稿，否则界面还在编辑上一份"
        );
    }

    /// 角标：有东西才有数字，定时开着只有点。
    #[test]
    fn badges_only_show_when_there_is_something() {
        let mut state = AppState::new("/Users/me".into());
        assert_eq!(badge_for(&state, Page::Quarantine), None);
        assert_eq!(badge_for(&state, Page::History), None);
        assert_eq!(badge_for(&state, Page::Automation), None);
        assert_eq!(badge_for(&state, Page::Workbench), None);

        state.history = vec![crate::state::HistoryRow {
            id: 1,
            path: "~/.npm/_cacache".into(),
            bytes: 1,
            at: "2026-10-23T03:00:12Z".into(),
            source: molt_act::CleanSource::Manual,
            action: crate::state::HistoryAction::Quarantined,
        }];
        assert_eq!(badge_for(&state, Page::History).as_deref(), Some("1"));

        state.auto.config.enabled = true;
        assert_eq!(
            badge_for(&state, Page::Automation).as_deref(),
            Some(""),
            "定时开着是状态不是数量，只画点"
        );
    }

    /// 端到端：往真实的 `MoltApp::draw` 里注入指针事件，逐个点顶栏图标，
    /// 看真的切到哪一页。
    ///
    /// 这条替代「肉眼看截图」：图标画出来了、位置对不对、点了有没有反应，
    /// 三件事只有在真布局 + 真事件里才同时成立。扫描顶栏一条横线，
    /// 记录每个 x 点下去落到哪一页，再由落点还原出五个可点区间。
    #[test]
    fn clicking_each_icon_switches_to_its_page() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let w = 1120.0_f32;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 1600.0));
        let mut app = MoltApp::headless(
            &ctx,
            crate::theme::Mode::Light,
            AppState::new("/Users/me".into()),
        );
        // 顶栏图标垂直居中：上内边距 12 + 图标高的一半
        let y = 12.0 + crate::theme::Palette::light().sz(34.0) / 2.0;

        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // 第一帧定布局
        run_frame(&ctx, &mut app, base.clone());

        // 起点用「设置」而不是「清理」：这样点清理图标也会产生可见变化，
        // 否则「点了没反应」和「本来就是这一页」分不开。
        //
        // 但任何单一起点都会让**那一页自己的图标**测不出来（点了页面不变）。
        // 所以扫两遍，起点分别是两个不同的页，两遍互补；同一个 x 至少一遍
        // 能给出答案。
        let mut known: std::collections::BTreeMap<i32, Page> = std::collections::BTreeMap::new();
        for start in [Page::Settings, Page::Workbench] {
            let mut x = 780.0_f32;
            while x < w - 20.0 {
                app.state.page = start;
                let pos = egui::Pos2::new(x, y);
                let mut down = base.clone();
                down.events = vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                ];
                run_frame(&ctx, &mut app, down);
                let mut up = base.clone();
                up.events = vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                }];
                run_frame(&ctx, &mut app, up);
                if app.state.page != start {
                    known.insert((x * 10.0) as i32, app.state.page);
                }
                x += 4.0;
            }
        }

        // 把相邻且落到同一页的采样点合并成一段可点区间
        let mut bands: Vec<(f32, f32, Page)> = Vec::new();
        for (kx, page) in known {
            let x = kx as f32 / 10.0;
            match bands.last_mut() {
                Some((_, hi, p)) if *p == page && x - *hi <= 4.5 => *hi = x,
                _ => bands.push((x, x, page)),
            }
        }

        assert_eq!(
            bands.len(),
            5,
            "顶栏要能点出五个互不重叠的图标，实际 {:?}",
            bands
        );
        // 视觉顺序必须还是 清理 → 临时空间 → 历史 → 自动清理 → 设置
        let order: Vec<Page> = bands.iter().map(|(_, _, p)| *p).collect();
        assert_eq!(order, Page::ALL.to_vec(), "图标顺序不对：{bands:?}");

        for (lo, hi, page) in &bands {
            assert!(
                hi - lo + 4.0 >= 34.0,
                "{page:?} 的可点区域只有 {} 点，低于 34 点的下限",
                hi - lo + 4.0
            );
        }

        // 五个图标都在右半边，且最右那个贴着右边距（28 点内边距）
        let (first, _, _) = bands[0];
        let (_, last_hi, last_page) = *bands.last().unwrap();
        assert_eq!(last_page, Page::Settings, "最右一个该是「设置」");
        assert!(first > w * 0.5, "图标要排到右半边：最左 {first}");
        assert!(
            (w - 28.0 - last_hi).abs() < 8.0,
            "最右的图标要贴住右边距：可点区右边缘 {last_hi}，应为 {}",
            w - 28.0
        );

        // 品牌在左半边，图标在右——中间不能有重叠
        assert!(first > 300.0, "图标不该压到品牌名上：最左 {first}");
    }

    /// 悬停要弹出中文名：图标替代了文字，认页全靠它。
    #[test]
    fn hovering_an_icon_shows_its_chinese_name() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        // 提示气泡默认有 0.5 秒延迟，测试里调到 0，否则要等半秒。
        // 必须放在 `headless` 之后：它会重装整套样式，之前改的会被覆盖。
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
        let w = 1120.0_f32;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 1600.0));
        let mut app = MoltApp::headless(
            &ctx,
            crate::theme::Mode::Light,
            AppState::new("/Users/me".into()),
        );
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
        let y = 12.0 + crate::theme::Palette::light().sz(34.0) / 2.0;
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        run_frame(&ctx, &mut app, base.clone());

        // 最右那个图标是「设置」：从右边距 28 + 半宽 23.8 处往里点
        let x = w - 28.0 - 47.6 / 2.0;
        let pos = egui::Pos2::new(x, y);
        let mut texts = Vec::new();
        for i in 0..4 {
            let mut raw = base.clone();
            raw.time = Some(i as f64 * 0.2);
            raw.events = vec![egui::Event::PointerMoved(pos)];
            texts = run_frame(&ctx, &mut app, raw);
        }
        assert!(
            texts.iter().any(|(t, _)| t == "设置"),
            "悬停设置图标要弹出中文名：{texts:#?}"
        );
    }

    /// 跑一帧真实的窗口，返回这一帧画出来的文字与坐标。
    fn run_frame(
        ctx: &egui::Context,
        app: &mut MoltApp,
        raw: egui::RawInput,
    ) -> Vec<(String, egui::Pos2)> {
        let mut texts = Vec::new();
        let mut out = ctx.run_ui(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        out.textures_delta.clear();
        for cs in &out.shapes {
            walk(&cs.shape, &mut texts);
        }
        texts
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
