//! 窗口主体。放在库里而不是 `main.rs` 里，是为了让验证工具
//! 能构造同一份界面并把它渲染成图片——验证用的画面和用户看到的必须是同一份代码。
//!
//! 数据流是单向的：页面拿到的是**只读**的 `&AppState`，改动一律通过
//! [`crate::action::Action`] 发回来，由这里调 `apply` 落到状态上。
//! 需要真的跑线程或碰盘的意图（`Effect`）也在这一层执行——全程序只有这里
//! 有副作用，因此「界面点了会发生什么」在别处是可推演的。

use eframe::egui;

use crate::action::{self, Action, Effect};
use crate::components::{StatusBar, StatusBarOutcome, StatusTone};
use crate::state::{AppState, Page, StatusKind, StatusMessage};

/// 整个窗口。
pub struct MoltApp {
    pub theme: crate::theme::Theme,
    pub state: AppState,
    /// 这一帧里界面发出来的动作。每帧开头清空，`draw` 结束时统一执行。
    actions: Vec<Action>,
}

impl MoltApp {
    /// 用当前系统环境构造一个可以直接跑的窗口。
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theme::install_fonts(&cc.egui_ctx);
        let mut theme = crate::theme::Theme::install(&cc.egui_ctx);
        let saved_scale = cc
            .storage
            .and_then(|storage| storage.get_string("cleanling.font_scale"))
            .and_then(|text| text.parse::<f32>().ok())
            .filter(|scale| [1.0, 1.2, crate::theme::Theme::DEFAULT_SCALE].contains(scale))
            .unwrap_or(crate::theme::Theme::DEFAULT_SCALE);
        theme.set_scale(&cc.egui_ctx, saved_scale);

        let home = crate::paths::home_dir().to_string_lossy().to_string();
        let mut state = AppState::new(home.clone());
        state.display_scale = saved_scale;

        match molt_rules::load(&home, Some(&molt_act::paths::user_rules_dir())) {
            Ok(r) => state.rules = Some(r),
            Err(e) => state.error = Some(format!("规则库加载失败：{e}")),
        }

        state.refresh_disk();

        // 先排空到期项，再读最终状态：首页待释放量与临时空间须同一口径。
        crate::pages::quarantine::purge_due_on_start(&mut state);
        crate::pages::history::reload(&mut state);
        crate::pages::quarantine::reload(&mut state);

        let leftovers = molt_act::paths::legacy_leftovers();
        if !leftovers.is_empty() {
            // 两条都是启动时该说的。以前这里直接给 status 赋值，把上面
            // `purge_due_on_start` 刚写的「排空了 N 项、释放 X」整条盖掉——
            // 用户永远不知道启动时有过一次删除，这在清理工具里是致命的。
            // 有排空在先就把迁移提示接在后面，一条都别丢。
            let migration = format!(
                "检测到旧版本数据 {} 项尚未迁移（位于 app.shou）。\
                 系统拒绝了自动搬运，请把该目录内容手动移到 app.molt 下。",
                leftovers.len()
            );
            state.status = Some(match state.status.take() {
                Some(prev) => StatusMessage::info(format!("{} {}", prev.text, migration)),
                None => StatusMessage::warn(migration),
            });
        }

        // 定时清理配了却不会跑，是这一版最安静的一种故障：配置写着
        // `enabled = true`，自动清理页上也写着「已启用」，唯独 launchd 里
        // 没有这个任务——到点什么都不发生，也没有任何提示。以前这条状态只在
        // 用户主动进「自动清理」页时才查一次，可一个定时任务坏掉之后，人恰恰
        // 最不会再去打开那一页。
        //
        // 启动时静默查一次（不装不卸、不写任何东西），发现「说启用了、系统里
        // 却没加载」就在首页直接说，并指向能修它的那一页。
        if let Some(text) = crate::schedule::startup_health_warning() {
            state.status = Some(match state.status.take() {
                Some(prev) => StatusMessage::warn(format!("{} {text}", prev.text)),
                None => StatusMessage::warn(text),
            });
        }

        // 先用上次存下的结果铺一屏，再在后台扫。
        //
        // 全盘扫描在真机上要十几秒（几十万个文件的体积）。以前窗口一打开就是
        // 一片空白 + 「正在扫描…」，用户每次启动都得等；而磁盘在十几秒里
        // 几乎不会变。读到缓存就先显示，后台刷新完再换掉。
        // 缓存超过 6 小时就不用：那时它已经不代表现在的磁盘。
        let cached = crate::worker::ScanResult::load_cache(
            &crate::paths::scan_cache(),
            std::time::Duration::from_secs(6 * 3600),
        );
        let have_cache = cached.is_some();
        if let Some((result, at)) = cached {
            // 用缓存里的时刻，不是「现在」——否则「10:34 那次扫描」里
            // 的时间会和实际扫出来的数字对不上。
            state.last_scan_at = Some(at);
            state.apply_result(std::sync::Arc::new(result));
        }

        // 有缓存就**不在启动时自动扫**。
        //
        // 这一条是「每次打开都加载那么多数据」的正面回答：以前不管有没有缓存，
        // 每次开窗都要把几十万个文件重新量一遍，风扇一打开就转。现在改成按需——
        // 数字已经在了，真要更新由用户点「重新扫描」。
        // 第一次用（没有缓存）仍然自动扫，否则用户面对的是空页面。
        if !have_cache {
            crate::pages::workbench::start_scan(&mut state);
        }

        Self {
            theme,
            state,
            actions: Vec::new(),
        }
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
        theme.palette = crate::theme::Palette::of(mode).with_scale(theme.scale);
        theme.follow_system = false;
        crate::theme::reinstall(ctx, &theme);
        Self {
            theme,
            state,
            actions: Vec::new(),
        }
    }

    /// 用一份给定的主题与状态构造，供出图工具使用。
    ///
    /// 出图工具需要自己控制主题模式与外接数据，所以不能走 `headless`；
    /// 但它也不该直接写结构体字面量——那样每加一个字段就要改它一次。
    pub fn with_parts(theme: crate::theme::Theme, state: AppState) -> Self {
        Self {
            theme,
            state,
            actions: Vec::new(),
        }
    }

    /// 收下一个界面动作，等这一帧画完统一执行。
    ///
    /// 不当场执行，是因为执行会改 `state`，而页面此时正拿着 `&AppState` 在读。
    /// 攒到帧末再一起处理，既避免了借用冲突，也让「一帧内的多次点击」有序可循。
    pub fn send(&mut self, act: Action) {
        self.actions.push(act);
    }

    /// 画一帧。
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.theme.sync_system(ui.ctx());
        self.theme.set_scale(ui.ctx(), self.state.display_scale);
        self.pump(ui.ctx());
        self.tick_clean_completion(ui.ctx());
        self.navigation_shortcuts(ui.ctx());

        let p = self.theme.palette;
        // 侧栏先占位：面板按占位顺序分配空间，内容区拿剩下的宽度。
        // 导航从顶栏图标换成访达式侧栏：中文名常显，认页不再靠记形状。
        self.sidebar(ui);
        self.status_modal(ui);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(p.bg)
                    .inner_margin(egui::Margin::symmetric(22, 18)),
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
                        // 页面只读状态、只发动作。确认框同理。
                        //
                        // 工作台已经改完（它是写入最多的一页，也是这套结构收益最直接的
                        // 一页）；其余四页仍以 `&mut AppState` 为入口，改造是下一步。
                        // 两种签名并存是刻意的过渡态：先让一条路径可测，再推平其余，
                        // 而不是一次性改动两千多行、让整轮都无法验证。
                        let mut acts = Vec::new();
                        match self.state.page {
                            Page::Workbench => {
                                crate::pages::workbench::show(ui, &self.state, &p, &mut acts);
                                crate::pages::workbench::confirm_modal(
                                    ui,
                                    &self.state,
                                    &p,
                                    &mut acts,
                                );
                            }
                            Page::Quarantine => {
                                crate::pages::quarantine::show(ui, &self.state, &p, &mut acts);
                                crate::pages::quarantine::confirm_purge_modal(
                                    ui,
                                    &self.state,
                                    &p,
                                    &mut acts,
                                );
                            }
                            Page::History => {
                                crate::pages::history::show(ui, &self.state, &p, &mut acts)
                            }
                            Page::Automation => {
                                crate::pages::automation::show(ui, &self.state, &p, &mut acts)
                            }
                            Page::Settings => {
                                crate::pages::settings::show(ui, &self.state, &p, &mut acts)
                            }
                        }
                        self.actions.append(&mut acts);
                    },
                );
            });

        self.dispatch();
    }

    /// 把这一帧攒下的动作全部落到状态上，并执行它们要求的副作用。
    fn dispatch(&mut self) {
        if self.actions.is_empty() {
            return;
        }
        // 换成局部变量再处理：`apply` 要拿 `&mut self.state`，
        // 不能同时借用 `self.actions`。
        let acts = std::mem::take(&mut self.actions);
        for act in acts {
            let effects = action::apply(&mut self.state, act);
            for fx in effects {
                self.run(fx);
            }
        }
    }

    /// 执行一个副作用意图。**这是全程序唯一会起线程/碰盘的地方。**
    fn run(&mut self, fx: Effect) {
        match fx {
            Effect::Scan => crate::pages::workbench::start_scan(&mut self.state),
            Effect::Analyze => crate::pages::workbench::start_analysis(&mut self.state),
            Effect::Clean(pending) => {
                crate::pages::workbench::start_clean(&mut self.state, &pending)
            }
            Effect::Purge(pending) => {
                // 现阶段 quarantine 仍以 `&mut AppState` 为入口；
                // 这里把批次写回状态再调它，就不必为了重构而改动
                // 那个页面的内部结构（它的改造留给下一步）。
                self.state.pending_purge = Some(*pending);
                crate::pages::quarantine::do_purge(&mut self.state)
            }
            Effect::RestoreQuarantine(id) => crate::pages::quarantine::restore(&mut self.state, id),
            Effect::RestoreQuarantineBatch(ids) => {
                crate::pages::quarantine::restore_ids(&mut self.state, &ids)
            }
            Effect::ReloadQuarantine => crate::pages::quarantine::reload(&mut self.state),
            Effect::ReloadHistory => crate::pages::history::reload(&mut self.state),
            Effect::ReloadAiConfig => self.state.ai.reload_config(),
            Effect::ReloadRules => crate::pages::settings::reload_rules(&mut self.state),
            Effect::SaveAiConfig => crate::pages::settings::save_ai_config(&mut self.state),
            Effect::SaveApiKey => crate::pages::settings::save_api_key(&mut self.state),
            Effect::EnsureAutoLoaded => crate::pages::automation::ensure_loaded(&mut self.state),
            Effect::SaveAuto => crate::pages::automation::save_draft(&mut self.state),
            Effect::SaveAutoDisabled => crate::pages::automation::save_disabled(&mut self.state),
            Effect::RefreshAutoStatus => crate::pages::automation::refresh_status(&mut self.state),
            Effect::RunScheduledNow => crate::pages::automation::run_now(&mut self.state),
            Effect::RefreshDisk => self.state.refresh_disk(),
            Effect::CollectExpired => crate::pages::quarantine::purge_due_on_start(&mut self.state),
            Effect::RevealInFinder(path) => {
                // `-R` 是在访达里**选中**它，不是打开它：用户想看看这个路径
                // 在哪、旁边还有什么，不该因此把它启动起来。
                if let Err(e) = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn()
                {
                    self.state.status = Some(crate::state::StatusMessage::error(format!(
                        "没能打开 {path} 所在的目录：{e}"
                    )));
                }
            }
            Effect::RevealQuarantine => {
                // 目录可能还不存在（一次都没清理过）：先建出来再打开，
                // 让「打开临时空间」永远有事发生，而不是弹一个找不到的错。
                let dir = crate::paths::quarantine_root();
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    self.state.status = Some(crate::state::StatusMessage::error(format!(
                        "打不开临时空间 {}：{e}",
                        dir.display()
                    )));
                } else if let Err(e) = std::process::Command::new("open").arg(&dir).spawn() {
                    self.state.status = Some(crate::state::StatusMessage::error(format!(
                        "没能让访达打开 {}：{e}",
                        dir.display()
                    )));
                }
            }
            Effect::ScanDuplicates => {
                // 重复文件检测是只读的后台任务：只展示与定位，绝不删除。
                if self.state.duplicate_job.is_none() {
                    self.state.duplicate_job =
                        Some(crate::worker::DuplicateJob::start(self.state.home.clone()));
                }
            }
            Effect::CopyPath(path) => {
                // macOS 专属：pbcopy 写入系统剪贴板。失败要给出可见原因。
                let mut child = match std::process::Command::new("pbcopy")
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => {
                        self.state.status = Some(crate::state::StatusMessage::error(format!(
                            "复制路径失败：{e}"
                        )));
                        return;
                    }
                };
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    if let Err(e) = stdin.write_all(path.as_bytes()) {
                        self.state.status = Some(crate::state::StatusMessage::error(format!(
                            "复制路径失败：{e}"
                        )));
                        return;
                    }
                }
                match child.wait() {
                    Ok(status) if status.success() => {
                        // 空字符串会画出一条什么字都没有的提示条：用户点完
                        // 「复制路径」看不到任何回应，以为没生效。
                        self.state.status =
                            Some(crate::state::StatusMessage::info("路径已复制到剪贴板"));
                    }
                    Ok(status) => {
                        self.state.status = Some(crate::state::StatusMessage::error(format!(
                            "复制路径失败（退出码 {}）",
                            status.code().unwrap_or(-1)
                        )));
                    }
                    Err(e) => {
                        self.state.status = Some(crate::state::StatusMessage::error(format!(
                            "复制路径失败：{e}"
                        )));
                    }
                }
            }
        }
    }

    /// 清理完成后将结果保留片刻；运行中的百分比只由执行器回调更新。
    fn tick_clean_completion(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let running = self.state.is_cleaning();
        let st = &mut self.state;
        if !running && st.clean_done_secs > 0.0 {
            st.clean_done_secs = (st.clean_done_secs - dt).max(0.0);
            st.clean_anim = if st.clean_done_secs > 0.0 { 1.0 } else { 0.0 };
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        } else if !running && st.clean_was_running {
            // 被中断或失败时没有完成报告，清掉残留进度。
            st.clean_anim = 0.0;
        }
        st.clean_was_running = running;
    }

    /// 每帧推进所有后台任务。后台结果回填不属于 `Action`：
    /// 它不是用户做的，而是任务完成的通知。
    fn pump(&mut self, ctx: &egui::Context) {
        crate::pages::workbench::pump_scan(&mut self.state);
        crate::pages::workbench::pump_clean(&mut self.state);
        crate::pages::workbench::pump_ai(&mut self.state);
        crate::pages::workbench::pump_duplicates(&mut self.state);
        crate::pages::quarantine::pump_purge(&mut self.state);
        crate::pages::quarantine::pump_restore(&mut self.state);
        crate::pages::automation::pump(&mut self.state);

        // 有任何动作在跑就持续重绘：否则进度条与「正在请求模型…」会停住
        if self.state.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    /// 通过已有动作处理 macOS 键盘导航，避免快捷键绕开扫描的忙碌守卫。
    fn navigation_shortcuts(&mut self, ctx: &egui::Context) {
        let shortcut = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                else {
                    return None;
                };
                if !modifiers.command {
                    return None;
                }
                let page_keys = [
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                ];
                if let Some(index) = page_keys.iter().position(|candidate| candidate == key) {
                    return Some(Action::GoPage(Page::ALL[index]));
                }
                match key {
                    egui::Key::Comma => Some(Action::GoPage(Page::Settings)),
                    egui::Key::R => Some(Action::FollowNextStep(crate::state::NextStep::Retry)),
                    _ => None,
                }
            })
        });
        if let Some(action) = shortcut {
            self.actions.push(action);
        }
    }

    /// 左侧导航栏：图标 + 中文名常显；窗口窄于一档时收成图标列。
    ///
    /// 导航从顶栏搬到这里，是为了让页面名一直是可读的文字，而不是悬停才
    /// 出现的气泡——「认页靠记形状」正是旧顶栏的问题。侧栏还顺带把窗口
    /// 顶部整条让给内容：首页第一眼要看的是磁盘数字，不是一条菜单。
    ///
    /// 宽窄两档画的是同一份 `Page::NAV_GROUPS`，只是窄档走 `nav_icon`
    /// 并把名字放回悬停提示——五十六点放不下「临时空间」四个字。
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.theme.palette;
        let state = &self.state;
        let mut acts = Vec::new();
        // 断点看整窗宽度：此时内容区还没占位，`available_width` 就是窗口宽。
        // 按参考图保留约五分之一窗口给导航；窄窗收成图标列，保证操作区足够宽。
        let compact = ui.available_width() < p.sz(700.0);
        let w = if compact { p.sz(56.0) } else { p.sz(158.0) };
        // 窄档里图标乘完缩放后约 48 点，侧栏内再留 4 点边距：56 - 8 ≈ 48。
        let side_pad = if compact { p.sz(4.0) } else { p.sz(12.0) };
        egui::Panel::left("sidebar")
            .resizable(false)
            // 图中侧栏与白色内容区由一条极细的冷灰线划分。
            .show_separator_line(true)
            .exact_size(w)
            .frame(
                egui::Frame::NONE
                    .fill(p.sidebar_bg)
                    .inner_margin(egui::Margin {
                        left: side_pad as i8,
                        right: side_pad as i8,
                        top: p.sz(22.0) as i8,
                        bottom: p.sz(12.0) as i8,
                    }),
            )
            .show(ui, |ui| {
                // 品牌块与设置图一致：使用应用现有的蓝色图标，名称置于下方。
                if let Some(tex) = crate::brand::texture(ui.ctx()) {
                    ui.vertical_centered(|ui| {
                        let h = p.sz(if compact { 34.0 } else { 50.0 });
                        ui.add(
                            egui::Image::from_texture(egui::load::SizedTexture::new(
                                tex.id(),
                                egui::vec2(h, h),
                            ))
                            .fit_to_exact_size(egui::vec2(h, h)),
                        );
                        if !compact {
                            ui.add_space(p.sz(7.0));
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new("净灵 · Cleanling")
                                        .size(p.sz(12.5))
                                        .strong()
                                        .color(p.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                    });
                }
                ui.add_space(p.sz(if compact { 16.0 } else { 26.0 }));

                // 所有导航行从同一轴线开始、间距一致。
                for page in Page::ALL {
                    let badge = badge_for(state, page);
                    if compact {
                        let resp = crate::widgets::nav_icon(
                            ui,
                            &p,
                            page.icon(),
                            page.title(),
                            state.page == page,
                            badge.as_deref(),
                        );
                        if resp.on_hover_text(page.title()).clicked() {
                            acts.push(Action::GoPage(page));
                        }
                    } else {
                        let resp = crate::widgets::sidebar_item(
                            ui,
                            &p,
                            page.icon(),
                            page.title(),
                            state.page == page,
                            badge.as_deref(),
                        );
                        if resp.clicked() {
                            acts.push(Action::GoPage(page));
                        }
                        ui.add_space(p.sz(4.0));
                    }
                }
            });
        self.actions.append(&mut acts);
    }

    /// 执行结果的提示条，悬浮在窗口**底部**。
    ///
    /// 上一版是 `egui::Modal`（带遮罩、点遮罩才关）。执行结果的提示不是需要
    /// 决策的问题——用户刚按下确认，他要知道的是「成了没有、差在哪」，而不是
    /// 被挡在一次必须点掉的对话前面。所以改成底部一条：不遮罩、不阻断操作、
    /// 带一个明确的关闭叉号，`Esc` 也能关。
    fn status_modal(&mut self, ui: &mut egui::Ui) {
        let p = self.theme.palette;
        let state = &self.state;
        let Some(msg) = state.status.clone() else {
            return;
        };
        // 结果卡（`op_report`）已经在首页/临时空间页写了同一句结论，此时 Info 不再提示——
        // 否则「清完 24 项」会同时出现在卡片和底部条里，用户还得关一次。
        // 但只有在那两页上卡片才真的画出来；别的页上没有卡片，Info 就得提示。
        // 出错和警告一律提示：失败必须被看见，不能只留在页面上。
        let card_on_screen =
            matches!(state.page, Page::Workbench | Page::Quarantine) && state.last_op.is_some();
        if msg.kind == StatusKind::Info && card_on_screen {
            return;
        }
        // 「正在扫描…」「正在还原…」这类进行中的提示不是结果，不该占住底部：
        // 而用户这时正等着看进度。进行中的状态由顶栏的活动文字与页面里的
        // 进度条表达，本来就一直看得见。
        if msg.kind == StatusKind::Info && msg.text.ends_with('…') {
            return;
        }

        // 包成组件的 props：这条提示是什么语气、说什么、有没有第二个动作。
        // 组件不认识 `AppState`，所以「要不要显示」的判断留在上面这一层。
        let tone = match msg.kind {
            StatusKind::Info => StatusTone::Ok,
            StatusKind::Warn => StatusTone::Warn,
            StatusKind::Error => StatusTone::Error,
        };
        let mut bar = StatusBar {
            tone,
            text: msg.text.clone(),
            action: None,
        };
        if msg.kind == StatusKind::Error {
            bar = bar.with_action("去设置");
        }

        match bar.show(ui, &p) {
            StatusBarOutcome::None => {}
            StatusBarOutcome::Dismiss => self.actions.push(Action::DismissStatus),
            StatusBarOutcome::Action => self.actions.push(Action::GoSettingsForError),
        }
    }
}

/// 主循环里给提示条用的一次点击应用到状态上。
///
/// 工作台现在发 `Action`，但临时空间页仍在用可变状态；两边都需要的这一个
/// 入口留在这里，避免两个页面各写一遍「按下一步去哪」。
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

impl eframe::App for MoltApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("cleanling.font_scale", self.state.display_scale.to_string());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
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
        paint_h(state, w, 1600.0)
    }

    /// 同上，视口高度由调用方定。底部提示条要按「离下沿多远」断言，得能改高度。
    fn paint_h(state: AppState, w: f32, h: f32) -> Vec<(String, egui::Pos2)> {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut app = MoltApp::headless(&ctx, crate::theme::Mode::Light, state);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w, h),
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

    /// 与 `walk` 相同，但给的是文字矩形的**中心**——点击要落在控件里，
    /// 而 `Shape::Text` 的 pos 是左上角。
    fn walk_centers(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                let r = egui::Rect::from_min_size(t.pos, t.galley.size());
                out.push((t.galley.text().to_string(), r.center()));
            }
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    walk_centers(s, out);
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

    /// 页面快捷键要经过真实绘制和 Action 分发，不只检查键位表。
    #[test]
    fn command_number_switches_sidebar_page() {
        let ctx = egui::Context::default();
        let mut app = MoltApp::headless(
            &ctx,
            crate::theme::Mode::Light,
            AppState::new("/Users/me".into()),
        );
        let modifiers = egui::Modifiers {
            command: true,
            ..Default::default()
        };
        for (key, expected) in [
            (egui::Key::Num5, Page::Settings),
            (egui::Key::Num1, Page::Workbench),
        ] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1120.0, 800.0),
                )),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
            });
            output.textures_delta.clear();
            assert_eq!(app.state.page, expected);
        }
    }

    /// 侧栏把中文页名常显出来：认页不再靠记图标形状。
    ///
    /// 这一条替代了旧顶栏的「一行图标、名字走悬停」断言——导航换到侧栏后，
    /// 要守的正好相反：每个页面名都必须在左侧栏的宽度范围内看得见。
    /// 品牌也搬到了侧栏顶部，一并断言。
    #[test]
    fn nav_names_are_always_visible_in_the_sidebar() {
        let texts = paint(AppState::new("/Users/me".into()), 1120.0);
        let p = crate::theme::Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);
        // 侧栏宽 196 点（乘缩放）再加左内边距；文字中心必须落在栏内。
        let sidebar_right = p.sz(196.0) + p.sz(12.0);

        for page in Page::ALL {
            let title = page.title();
            let hit = texts
                .iter()
                .find(|(t, pos)| t == title && pos.x < sidebar_right);
            let (_, pos) = hit.unwrap_or_else(|| panic!("侧栏要常显「{title}」：{texts:#?}"));
            assert!(
                pos.x < sidebar_right,
                "「{title}」要画在侧栏里，实际 x = {}",
                pos.x
            );
        }

        let (_, brand) = texts
            .iter()
            .find(|(t, _)| t.contains("净灵"))
            .expect("品牌要留着");
        assert!(brand.x < sidebar_right, "品牌要在侧栏顶部：x = {}", brand.x);
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
                    let resp = crate::widgets::nav_icon(
                        ui,
                        &p,
                        Page::Settings.icon(),
                        "设置",
                        false,
                        None,
                    );
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
        let mut app = MoltApp::headless(
            &egui::Context::default(),
            crate::theme::Mode::Light,
            AppState::new("/Users/me".into()),
        );
        for page in Page::ALL {
            app.send(Action::GoPage(page));
            app.dispatch();
            assert_eq!(app.state.page, page, "切到 {page:?} 之后 page 必须跟着变");
        }

        // 一个绝不会出现在任何真实配置里的规则 id
        const SENTINEL: &str = "__go_page_reload_probe__";
        app.state.auto.config.rule_ids = vec![SENTINEL.to_string()];
        app.state.auto.loaded = true; // 内存里这份「已经读过」——不重置就会直接复用
        app.state.auto.draft = None;

        app.send(Action::GoPage(Page::Automation));
        // 关键：切页现在只**声明**要重读（`Effect::EnsureAutoLoaded`），
        // 真正读盘由 dispatch 执行。所以这里必须走完整链路——
        // 只调 `apply` 的话，`loaded=false` 之后的读取根本没发生。
        app.dispatch();

        assert!(
            !app.state.auto.config.rule_ids.iter().any(|r| r == SENTINEL),
            "进自动清理页必须重读配置；标记还在，说明 Effect 没被执行"
        );
        assert!(app.state.auto.loaded, "重读跑完之后 loaded 应回到 true");
        assert!(
            app.state.auto.draft.is_some(),
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

    /// 端到端：往真实的 `MoltApp::draw` 里注入指针事件，沿侧栏从上往下点，
    /// 记录每个 y 点下去落到哪一页，再由落点还原出七个可点区间。
    ///
    /// 旧版扫的是顶栏一条横线；导航换到侧栏后这条线变成一列。起点扫两遍
    /// （设置 / 清理）互补，否则当前页自己的条目测不出来。
    #[test]
    fn clicking_each_sidebar_item_switches_to_its_page() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let w = 1120.0_f32;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 1400.0));
        let mut app = MoltApp::headless(
            &ctx,
            crate::theme::Mode::Light,
            AppState::new("/Users/me".into()),
        );
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // 第一帧定布局
        run_frame(&ctx, &mut app, base.clone());

        // 侧栏内一点：避开品牌区，落在条目的整行命中区里。
        let x = crate::theme::Palette::light().sz(60.0);
        let mut known: std::collections::BTreeMap<i32, Page> = std::collections::BTreeMap::new();
        for start in [Page::Settings, Page::Workbench] {
            let mut y = 60.0_f32;
            while y < 900.0 {
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
                    known.insert((y * 10.0) as i32, app.state.page);
                }
                y += 4.0;
            }
        }

        // 把相邻且落到同一页的采样点合并成一段可点区间
        let mut bands: Vec<(f32, f32, Page)> = Vec::new();
        for (ky, page) in known {
            let y = ky as f32 / 10.0;
            match bands.last_mut() {
                Some((_, hi, p)) if *p == page && y - *hi <= 4.5 => *hi = y,
                _ => bands.push((y, y, page)),
            }
        }

        assert_eq!(
            bands.len(),
            5,
            "侧栏要能点出五个互不重叠的条目，实际 {bands:?}"
        );
        // 视觉顺序必须还是 清理 → 临时空间 → 历史 → 自动清理 → 设置
        let order: Vec<Page> = bands.iter().map(|(_, _, p)| *p).collect();
        assert_eq!(order, Page::ALL.to_vec(), "侧栏顺序不对：{bands:?}");

        for (lo, hi, page) in &bands {
            assert!(
                hi - lo + 4.0 >= 30.0,
                "{page:?} 的可点区域只有 {} 点，低于 30 点的下限",
                hi - lo + 4.0
            );
        }
        assert!(
            bands[0].0 > 40.0,
            "第一个条目不能压在品牌上：起点 {}",
            bands[0].0
        );
    }

    /// 窄窗侧栏收成图标列：名字藏回悬停提示。
    ///
    /// 宽档的名字常显（上一条测试守着）；窄到放不下 196 点侧栏时，
    /// 名字必须还能通过悬停拿到，否则窄窗里认页只能靠猜形状。
    #[test]
    fn compact_sidebar_names_appear_on_hover() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 900.0));
        let mut app = MoltApp::headless(
            &ctx,
            crate::theme::Mode::Light,
            AppState::new("/Users/me".into()),
        );
        // 提示气泡默认有 0.5 秒延迟，测试里调到 0。必须在 `headless` 之后：
        // 它会重装整套样式，之前改的会被覆盖。
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let p = crate::theme::Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);
        let col_right = p.sz(56.0) + p.sz(4.0);

        // 第一帧：窄档侧栏里不该再画任何页名。
        let texts = run_frame(&ctx, &mut app, base.clone());
        for page in Page::ALL {
            let title = page.title();
            assert!(
                !texts.iter().any(|(t, pos)| t == title && pos.x < col_right),
                "窄档侧栏不该再画「{title}」：{texts:#?}"
            );
        }

        // 从侧栏顶部往下扫，指针停在图标上时提示里应出现中文名。
        let x = p.sz(30.0);
        let mut shown = false;
        let mut y = 40.0_f32;
        while y < 700.0 && !shown {
            let pos = egui::Pos2::new(x, y);
            for i in 0..4 {
                let mut raw = base.clone();
                raw.time = Some(i as f64 * 0.2);
                raw.events = vec![egui::Event::PointerMoved(pos)];
                let texts = run_frame(&ctx, &mut app, raw);
                if texts
                    .iter()
                    .any(|(t, _)| Page::ALL.iter().any(|pg| t == pg.title()))
                {
                    shown = true;
                    break;
                }
            }
            y += 8.0;
        }
        assert!(shown, "窄档侧栏悬停图标要弹出中文名");
    }

    /// 端到端：在真实窗口里点底栏的「清理这些（可还原）」，必须真的装上待确认批次。
    ///
    /// 这条是单向数据流的验收凭据：页面只发 `Action`，若 `dispatch` 漏了、
    /// 或者 `apply` 里那条分支写错，这个测试就会挂。
    ///
    /// 首页那颗大按钮已经删掉——底栏是唯一的主操作入口，所以这条测的是
    /// 「清单默认展开、底栏常显」之后用户真正会点的那个位置。
    #[test]
    fn clicking_the_footer_button_prepares_a_batch() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1120.0, 1600.0));
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        // 一份有可清量的假结果，不碰真实磁盘
        let mut state = AppState::new("/Users/me".into());
        state.apply_result(std::sync::Arc::new(crate::worker::ScanResult {
            items: vec![
                molt_core::Candidate::new(
                    "/Users/me/a/target",
                    molt_core::ItemKind::Dir,
                    molt_core::Verdict::Reclaimable,
                )
                .with_size(1000, 1000, 1),
                molt_core::Candidate::new(
                    "/Users/me/b/target",
                    molt_core::ItemKind::Dir,
                    molt_core::Verdict::Reclaimable,
                )
                .with_size(2000, 2000, 1),
            ],
            rule_ids: Vec::new(),
            denied_roots: vec![],
            elapsed_secs: 0.0,
            from_cache: false,
        }));
        let mut app = MoltApp::headless(&ctx, crate::theme::Mode::Light, state);
        // 底栏主按钮在没有勾选时是置灰的，先勾上两条。
        app.state.checked.insert("/Users/me/a/target".into());
        app.state.checked.insert("/Users/me/b/target".into());

        // 按钮文字是稳定定位依据：扫这一帧画出来的文字，取它的中心去点。
        let mut hit = None;
        let mut out = ctx.run_ui(base.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        out.textures_delta.clear();
        {
            let mut texts: Vec<(String, egui::Pos2)> = Vec::new();
            for cs in &out.shapes {
                walk_centers(&cs.shape, &mut texts);
            }
            if let Some((_, pos)) = texts.iter().find(|(t, _)| t.contains("清理这些（可还原）"))
            {
                hit = Some(*pos);
            }
        }
        let click = hit.expect("底栏必须有「清理这些（可还原）」按钮");

        assert!(app.state.pending_clean.is_none(), "点之前不该有待确认批次");

        let modifiers = egui::Modifiers::default();
        let mut down = base.clone();
        down.events = vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
        ];
        let mut o = ctx.run_ui(down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();
        let mut up = base.clone();
        up.events = vec![egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        }];
        let mut o = ctx.run_ui(up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();

        let pending = app
            .state
            .pending_clean
            .as_ref()
            .expect("点底栏主按钮必须装上待确认批次——没装上说明 Action 没走到 apply");
        assert_eq!(pending.items.len(), 2, "应把两条勾选都装进批次");
        assert_eq!(
            pending.mode,
            molt_act::CleanMode::Quarantine,
            "主按钮走的是可还原的路"
        );
    }

    #[test]
    fn clicking_select_all_in_quarantine_checks_every_restorable_row() {
        // 这一页原先直接 `state.select_all_quarantine()`，重构后必须走
        // 点击 → Action → dispatch → apply 才能生效。测试在真实 draw 循环里点，
        // 走不通就说明链路断在某一环。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let p = crate::theme::Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1120.0, 900.0));
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let mut state = AppState::new("/Users/me".into());
        state.page = crate::state::Page::Quarantine;
        state.quarantine = vec![
            crate::state::QuarantineRow {
                id: 1,
                original: "/Users/me/Library/a".into(),
                bytes: 1024,
                at: "2026-10-20T10:00:00Z".into(),
                purge_after: "2026-10-27T10:00:00Z".into(),
                batch: Some("sample-batch".into()),
                restorable: true,
            },
            crate::state::QuarantineRow {
                id: 2,
                original: "/Users/me/Library/b".into(),
                bytes: 2048,
                at: "2026-10-20T10:00:00Z".into(),
                purge_after: "2026-10-27T10:00:00Z".into(),
                batch: Some("sample-batch".into()),
                restorable: true,
            },
        ];
        let mut app = MoltApp::headless(&ctx, crate::theme::Mode::Light, state);

        assert_eq!(app.state.quarantine_checked_count(), 0, "点之前不该有勾选");

        // 先画一帧，拿到「全选」按钮的文字位置
        let mut out = ctx.run_ui(base.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        out.textures_delta.clear();
        let mut hit = None;
        {
            let mut texts: Vec<(String, egui::Pos2)> = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut texts);
            }
            if let Some((_, pos)) = texts.iter().find(|(t, _)| t.trim() == "全选") {
                hit = Some(*pos);
            }
        }
        let pos = hit.expect("临时空间页底部必须有「全选」按钮");
        let click = egui::Pos2::new(pos.x, pos.y - p.sz(4.0));

        let modifiers = egui::Modifiers::default();
        let mut down = base.clone();
        down.events = vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
        ];
        let mut o = ctx.run_ui(down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();
        let mut up = base.clone();
        up.events = vec![egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        }];
        let mut o = ctx.run_ui(up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();

        assert_eq!(
            app.state.quarantine_checked_count(),
            2,
            "点「全选」必须经 Action 勾上两行——勾不上说明链路断了"
        );
    }

    #[test]
    fn clicking_a_history_filter_switches_it() {
        // 历史页的筛选原先直接赋值，现在必须经 Action::SetHistoryFilter。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let p = crate::theme::Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1120.0, 900.0));
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let mut state = AppState::new("/Users/me".into());
        state.page = crate::state::Page::History;
        state.history_filter = crate::state::HistoryFilter::All;
        // 空列表会提前返回，筛选控件根本画不出来——所以先放一条真记录。
        state.history = vec![crate::state::HistoryRow {
            id: 1,
            path: "/Users/me/a/target".into(),
            bytes: 4096,
            at: "2026-10-20T10:00:00Z".into(),
            source: molt_act::CleanSource::Manual,
            action: crate::state::HistoryAction::Quarantined,
        }];
        let mut app = MoltApp::headless(&ctx, crate::theme::Mode::Light, state);

        let mut out = ctx.run_ui(base.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        out.textures_delta.clear();
        let mut hit = None;
        {
            let mut texts: Vec<(String, egui::Pos2)> = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut texts);
            }
            // 分段控件的文案带计数，例如「永久删除 0」——用前缀认它
            if let Some((_, pos)) = texts
                .iter()
                .find(|(t, _)| t.starts_with("永久删除") && !t.contains("已还原"))
            {
                hit = Some(*pos);
            }
        }
        let pos = hit.expect("历史页必须有「永久删除」这个筛选");
        let click = egui::Pos2::new(pos.x, pos.y - p.sz(4.0));

        let modifiers = egui::Modifiers::default();
        let mut down = base.clone();
        down.events = vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
        ];
        let mut o = ctx.run_ui(down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();
        let mut up = base.clone();
        up.events = vec![egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        }];
        let mut o = ctx.run_ui(up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();

        assert_eq!(
            app.state.history_filter,
            crate::state::HistoryFilter::Gone,
            "点「永久删除」筛选必须切过去"
        );
    }

    #[test]
    fn clicking_advanced_in_settings_toggles_it() {
        // 设置页的高级区原先直接翻转字段，现在必须经 Action。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1120.0, 1400.0));
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let mut state = AppState::new("/Users/me".into());
        state.page = crate::state::Page::Settings;
        state.settings_advanced = false;
        let mut app = MoltApp::headless(&ctx, crate::theme::Mode::Light, state);

        let mut out = ctx.run_ui(base.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        out.textures_delta.clear();
        let mut hit = None;
        {
            let mut texts: Vec<(String, egui::Pos2)> = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut texts);
            }
            if let Some((_, pos)) = texts.iter().find(|(t, _)| t.starts_with("高级")) {
                hit = Some(*pos);
            }
        }
        let pos = hit.expect("设置页必须有「高级」这一栏");
        let click = egui::Pos2::new(pos.x, pos.y);

        let modifiers = egui::Modifiers::default();
        let mut down = base.clone();
        down.events = vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
        ];
        let mut o = ctx.run_ui(down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();
        let mut up = base.clone();
        up.events = vec![egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        }];
        let mut o = ctx.run_ui(up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();

        assert!(
            app.state.settings_advanced,
            "点「高级」必须展开——没展开说明 Action 没走到 apply"
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

    /// 点行末「详情」打开抽屉；Esc 关掉。
    ///
    /// 走真实事件：行本身是「点一下勾选」，详情入口必须能从同一个矩形里
    /// 分出另一条路——只在截图上看「画了个详情」说明不了它点得动。
    #[test]
    fn clicking_detail_opens_and_escape_closes_the_drawer() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1120.0, 1200.0));
        let base = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let mut state = AppState::new("/Users/me".into());
        state.apply_result(std::sync::Arc::new(crate::worker::ScanResult {
            items: vec![molt_core::Candidate::new(
                "/Users/me/a/target",
                molt_core::ItemKind::Dir,
                molt_core::Verdict::Reclaimable,
            )
            .with_size(1000, 1000, 1)],
            rule_ids: Vec::new(),
            denied_roots: vec![],
            elapsed_secs: 0.0,
            from_cache: false,
        }));
        let mut app = MoltApp::headless(&ctx, crate::theme::Mode::Light, state);

        // 第一帧：拿到「详情」文字的中心。
        let mut texts: Vec<(String, egui::Pos2)> = Vec::new();
        let mut out = ctx.run_ui(base.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        out.textures_delta.clear();
        for cs in &out.shapes {
            walk_centers(&cs.shape, &mut texts);
        }
        let click = texts
            .iter()
            .find(|(t, _)| t == "详情")
            .map(|(_, p)| *p)
            .expect("清单行要有「详情」入口");

        // 先单独一帧把指针移过去：按钮的 hover 状态要上一帧就建立，
        // 同一帧里「移入 + 按下」在裁剪边缘的控件上不一定算命中。
        let mut hover = base.clone();
        hover.events = vec![egui::Event::PointerMoved(click)];
        run_frame(&ctx, &mut app, hover);

        let modifiers = egui::Modifiers::default();
        let mut down = base.clone();
        down.events = vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
        ];
        let mut o = ctx.run_ui(down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();
        let mut up = base.clone();
        up.events = vec![egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers,
        }];
        let mut o = ctx.run_ui(up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
        });
        o.textures_delta.clear();
        assert!(
            app.state.detail_path.is_some(),
            "点「详情」要打开抽屉，当前 detail_path = {:?}",
            app.state.detail_path
        );

        // 打开后要有抽屉里的内容（不是只改了一个字段）。
        // 画两帧：抽屉是 `Area`，第一帧建立区域、第二帧才稳定出图。
        let mut texts = Vec::new();
        for _ in 0..2 {
            texts.clear();
            let mut out = ctx.run_ui(base.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.draw(ui));
            });
            out.textures_delta.clear();
            for cs in &out.shapes {
                walk_centers(&cs.shape, &mut texts);
            }
        }
        assert!(
            texts.iter().any(|(t, _)| t.contains("本次勾选")),
            "抽屉打开后要画出条目详情：{texts:#?}"
        );

        // Esc 关闭：抽屉不是只能等它自己消失的浮层。
        let mut esc = base.clone();
        esc.events = vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }];
        run_frame(&ctx, &mut app, esc);
        assert!(
            app.state.detail_path.is_none(),
            "Esc 要关掉抽屉，当前 {:?}",
            app.state.detail_path
        );
    }

    /// 执行结果的提示必须看得见，但贴在窗口底部，不挡在屏幕正中。
    ///
    /// 这条断言守三件事：标题在、原文完整、整条落在视口下缘。
    /// 之前是居中模态框，带遮罩、要点掉才能继续操作；现在改成底部一条。
    #[test]
    fn errors_show_in_a_bottom_bar() {
        let h = 1600.0;
        let mut state = AppState::new("/Users/me".into());
        state.status = Some(StatusMessage::error(
            "清理没能完成：/Users/me/project/molt/target 被另一个进程占用。",
        ));
        let texts = paint_h(state, 1120.0, h);
        assert!(
            texts.iter().any(|(t, _)| t.contains("出错了")),
            "提示条要有标题：{texts:#?}"
        );
        assert!(
            texts.iter().any(|(t, _)| t.contains("被另一个进程占用")),
            "错误原文必须完整显示：{texts:#?}"
        );
        // 落在视口下四分之一之内：居中弹窗会落在中间，底栏才落在这一段。
        let (title, pos) = texts
            .iter()
            .find(|(t, _)| t.contains("出错了"))
            .expect("提示条要有标题");
        assert!(
            pos.y > h * 0.75,
            "提示条要贴在窗口底部，实际 y = {}（标题 {title}）",
            pos.y
        );
    }

    /// 关闭叉号必须真的能关掉提示条——「画了但点不动」在截图上完全看不出来。
    ///
    /// 走的是真布局 + 真指针事件：在隔离的 Ui 里点那个叉号，再确认它回报了一次点击。
    /// 位置不写死的坐标，而是从控件自己的 rect 取中心——写死坐标会在字号或内边距
    /// 任何一处变动时悄悄点到别处去，测试照样绿。
    #[test]
    fn close_button_is_clickable() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let p = crate::theme::Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);

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
                    let mut r = egui::Rect::NOTHING;
                    let clicked = crate::widgets::close_button_probe(ui, &p, &mut r);
                    rect.set(r);
                    if clicked {
                        hit.set(true);
                    }
                });
            });
            out.textures_delta.clear();
        }

        assert!(rect.get().is_positive(), "关闭叉号要占一块可点的区域");
        assert!(hit.get(), "点关闭叉号的中心必须算一次点击");
    }
}
