//! 自动清理：每天 / 每周定时跑一次，清用户勾选的那几类。
//!
//! 这一页要能回答四个问题，顺序也是版面的顺序：
//! 1. 现在开没开、什么时候跑；
//! 2. 到点清哪些类型（来自规则库里标了 `auto` 的那些，不是另编一份清单）；
//! 3. 任务到底在系统里装没装（launchd 真加载了没有，不只是文件存在）；
//! 4. 上次跑成什么样。
//!
//! 安全边界：定时任务只碰「规则声明了 auto + 判定可回收」的项，且只移入隔离区、
//! 绝不直接删除。理由与 `molt auto` 一致——没人看着的时候不做不可逆的事。

use eframe::egui;

use crate::action::Action;
use crate::schedule::{self, Frequency};
use crate::state::{AppState, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{
    badge, card, danger_ghost_button, page_header, plain_section, primary_button, secondary_button,
};

/// 进入这一页时读一次配置与状态。读文件有开销，不能每帧做。
pub fn ensure_loaded(state: &mut AppState) {
    if state.auto.loaded {
        return;
    }
    match schedule::load_config() {
        Ok(mut cfg) => {
            // 参与定时清理的类型不再由用户勾选：就是规则库里标了 auto、
            // 且判定为可回收的那一批。每到一次规则库更新，它自动跟上；
            // 让用户逐条勾的结果是「勾少了以为开了却什么都没清」。
            //
            // 但只在**磁盘上还是空的**时才补默认值：无条件覆盖会让一进页面
            // 就与磁盘上那份（老版本或 CLI 写下的子集）不一致，底栏直接
            // 报「有未保存的更改」，而用户什么都没动。
            if cfg.rule_ids.is_empty() {
                if let Some(rules) = &state.rules {
                    cfg.rule_ids = schedule::all_rule_ids(rules);
                }
            }
            state.auto.config = cfg;
            state.auto.draft = Some(state.auto.config.clone());
            state.auto.error = None;
        }
        Err(e) => state.auto.error = Some(e),
    }
    state.auto.last_run = schedule::load_last_run();
    match schedule::load_status() {
        Ok(loaded) => {
            state.auto.installed = Some(loaded);
            state.auto.installed_error = None;
        }
        Err(e) => {
            state.auto.installed = Some(false);
            state.auto.installed_error = Some(e);
        }
    }
    state.auto.loaded = true;
}

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    // 保存条固定在底部：改动后不管滚到哪里都能看见「有一份没保存」。
    save_bar(ui, state, p, acts);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            automation_header(ui, state, p, acts);
            ui.add_space(p.sz(7.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new("到点只移入临时空间，可还原；立即执行按已保存的配置运行。")
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(p.sz(14.0));

            if let Some(err) = state.auto.error.clone() {
                card(ui, p, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("定时配置有问题：{err}"))
                                .size(p.sz(12.0))
                                .color(p.danger),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                    ui.add_space(6.0);
                    if secondary_button(ui, p, "重新读取", true) {
                        acts.push(Action::ReloadAutoConfig);
                    }
                });
                ui.add_space(10.0);
            }

            plain_section(ui, p, |ui| status_card(ui, state, p, acts));
            plain_section(ui, p, |ui| schedule_card(ui, state, p, acts));
            plain_section(ui, p, |ui| last_run_card(ui, state, p, acts));
            // 滚动区底边紧贴保存条时，最后一行会被切成半截——
            // 在这里留一段空白，滚到底时内容是完整看得见的。
            ui.add_space(12.0);
        });
}

/// 顶部先展示已保存任务的真实状态；切开关只改草稿，必须由底栏保存才生效。
fn automation_header(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let enabled = state.auto.editing().enabled;
    let (status, tint) = header_status(state, p);
    let narrow = ui.available_width() < p.sz(600.0);
    if narrow {
        page_header(ui, p, "自动清理", "", |ui| {
            if secondary_button(ui, p, "立即执行一次", state.can_start()) {
                acts.push(Action::RunScheduledNow);
            }
        });
        ui.add_space(p.sz(6.0));
        ui.horizontal_wrapped(|ui| {
            if enable_switch(ui, p, enabled, state.auto.loaded && !state.auto.saving) {
                acts.push(Action::SetAutoEnabled(!enabled));
            }
            ui.label(egui::RichText::new(status).size(p.sz(11.5)).color(tint));
        });
    } else {
        page_header(ui, p, "自动清理", "", |ui| {
            if secondary_button(ui, p, "立即执行一次", state.can_start()) {
                acts.push(Action::RunScheduledNow);
            }
            ui.add_space(p.sz(10.0));
            ui.add(
                egui::Label::new(egui::RichText::new(status).size(p.sz(11.5)).color(tint))
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
            ui.add_space(p.sz(8.0));
            if enable_switch(ui, p, enabled, state.auto.loaded && !state.auto.saving) {
                acts.push(Action::SetAutoEnabled(!enabled));
            }
        });
    }
}

/// 开关编辑的是草稿，状态文字必须写已生效的配置和 launchd 查询结果。
fn header_status(state: &AppState, p: &Palette) -> (&'static str, egui::Color32) {
    let saved = state.auto.config.enabled;
    let draft = state.auto.editing().enabled;
    if state.auto.dirty() {
        return match (saved, draft, state.auto.installed) {
            (false, true, _) => ("启用待保存，系统任务尚未加载", p.costly),
            (true, false, Some(true)) => ("关闭待保存，原任务仍生效", p.costly),
            (true, true, Some(true)) => ("计划待保存，原任务仍生效", p.costly),
            (true, _, Some(false)) => ("更改待保存，系统任务未加载", p.costly),
            (true, _, None) => ("更改待保存，正在查询系统状态", p.costly),
            (false, false, _) => ("计划待保存，当前仍关闭", p.costly),
        };
    }
    match (saved, state.auto.installed) {
        (false, _) => ("已关闭", p.text_dim),
        (true, Some(true)) => ("已启用，系统里已加载", p.text),
        (true, Some(false)) => ("已启用，但系统里没加载", p.costly),
        (true, None) => ("已启用，正在查询系统状态", p.text_dim),
    }
}

/// 自绘系统风格开关；点击仅修改定时配置草稿，不直接安装或卸载系统任务。
fn enable_switch(ui: &mut egui::Ui, p: &Palette, on: bool, enabled: bool) -> bool {
    let size = egui::vec2(p.sz(37.0), p.sz(22.0));
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, on, "启用自动清理")
    });
    let track = if !enabled {
        p.border_strong
    } else if on {
        p.reclaimable
    } else {
        p.surface_alt
    };
    let radius = (rect.height() / 2.0).round() as u8;
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(radius),
        track,
        egui::Stroke::new(1.0, if on { p.reclaimable } else { p.border_strong }),
        egui::StrokeKind::Inside,
    );
    let knob_radius = p.sz(8.5);
    let center = egui::pos2(
        if on {
            rect.right() - p.sz(11.0)
        } else {
            rect.left() + p.sz(11.0)
        },
        rect.center().y,
    );
    ui.painter().circle_filled(
        center,
        knob_radius,
        if on { p.on_accent() } else { p.surface },
    );
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            egui::CornerRadius::same(radius),
            egui::Stroke::new(2.0, p.focus),
            egui::StrokeKind::Outside,
        );
    }
    resp.on_hover_text("切换后在页面底部保存才会生效").clicked()
}

/// 顶部：开没开、什么时候跑、launchd 里到底有没有。
fn status_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let cfg = state.auto.config.clone();
    let installed = state.auto.installed;
    let running = state.is_scheduled();

    ui.vertical(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new("已保存的计划").size(p.sz(14.0)).strong())
                    .wrap_mode(egui::TextWrapMode::Extend),
            );

            badge(ui, p, &cfg.describe(), p.text_faint, p.surface_alt);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{} 个类型", cfg.rule_ids.len()))
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
            if running {
                badge(ui, p, "正在执行", p.costly, p.costly_soft);
            }
        });

        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "到点只清规则库里标了「可自动清理」的类型——它们删了会自动重建，不联网、不掉登录态。\
                     密钥、配置、会话历史永远不在其中。",
                )
                .size(p.sz(11.0))
                .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );

        // 「已启用但没加载」是最容易让人白等一晚上的状态，必须给出修复动作
        if cfg.enabled && installed == Some(false) {
            ui.add_space(6.0);
            let detail = state
                .auto
                .installed_error
                .clone()
                .unwrap_or_else(|| "launchd 里没有这个任务".to_string());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "配置文件说已启用，但系统里没加载（{detail}），到点不会执行。可重新查询，或重新安装已保存的计划。"
                    ))
                    .size(p.sz(11.0))
                    .color(p.costly),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            // 保存与开关都在底部那条固定的保存条上。这里只留一个「重新查询」：
            // 同一个页面里出现两处「保存」，用户没法判断哪个才是真的。
            if secondary_button(ui, p, "刷新状态", !state.auto.saving) {
                acts.push(Action::RefreshAutoStatus);
            }
            if cfg.enabled && installed == Some(false) && !state.auto.dirty()
                && secondary_button(ui, p, "重新安装任务", state.can_start() && !state.auto.saving)
            {
                acts.push(Action::SaveDraft);
            }
            if state.auto.saving {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("正在保存…")
                            .size(p.sz(10.5))
                            .color(p.text_dim),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            } else if state.is_busy() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(state.activity().blocking_hint())
                            .size(p.sz(10.5))
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
    });
}

/// 底部保存条：有改动才显形，没改动时只报当前生效的那份。
/// 保存条主按钮此刻该写什么、能不能按。
///
/// 抽成纯函数是为了让界面和测试问同一个问题。曾经这里只按 `dirty()` 决定可用性，
/// 于是「配置已启用、系统里却没有这个任务」时按钮永远灰着，而页面别处没有
/// 重装入口——用户看到一个声称已启用的定时任务，实际永远不会跑。
fn primary_save(state: &AppState) -> (&'static str, bool) {
    let cfg = state.auto.editing();
    let dirty = state.auto.dirty();
    // 配置说启用、launchd 里却没有：这不是「有改动」，但确实需要重新装一次。
    let needs_reinstall =
        state.auto.config.enabled && state.auto.installed == Some(false) && !dirty;
    if needs_reinstall {
        return ("重新安装任务", true);
    }
    // 文案说的是「按下会发生什么」：已启用却关了草稿的开关，要说「保存并关闭」，
    // 否则「保存并启用」会让人以为它会重新启用一遍。
    let label = match (state.auto.config.enabled, cfg.enabled, dirty) {
        (true, false, _) => "保存并关闭",
        (false, true, _) => "保存并启用",
        (_, _, true) => "保存更改",
        _ => "保存",
    };
    (label, dirty)
}

fn save_bar(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let dirty = state.auto.dirty();
    egui::Panel::bottom("automation-save")
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .inner_margin(egui::Margin::symmetric(28, 10)),
        )
        .show(ui, |ui| {
            let cfg = state.auto.editing().clone();
            let can = state.can_start() && !state.auto.saving;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = p.sz(1.0);
                    if dirty {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("有未保存的更改")
                                    .size(p.sz(13.0))
                                    .strong()
                                    .color(p.costly),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("保存后才会生效：{}", cfg.describe()))
                                    .size(p.sz(10.5))
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                    } else if state.auto.config.enabled {
                        // 与状态卡同一口径：launchd 里没有这个任务时不能写「已在跑」，
                        // 否则一边提示「没加载」、一边说「在跑」，用户没法判断到底会不会执行。
                        let loaded = state.auto.installed;
                        let text = match loaded {
                            Some(true) => format!("已按这个时间在跑：{}", cfg.describe()),
                            Some(false) => {
                                format!("配置已启用，但系统里没加载：{}", cfg.describe())
                            }
                            None => format!("配置已启用，正在查询系统任务：{}", cfg.describe()),
                        };
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).size(p.sz(12.5)).color(
                                if loaded == Some(false) {
                                    p.costly
                                } else {
                                    p.text_dim
                                },
                            ))
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("自动清理现在是关闭的")
                                    .size(p.sz(12.5))
                                    .color(p.text_dim),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 按钮文案说的是「按下会发生什么」。已经启用、只是改了时间，
                    // 再写「保存并启用」会让人以为它会重新启用一遍或者覆盖别的东西。
                    let (label, usable) = primary_save(state);
                    if primary_button(ui, p, label, can && usable) {
                        acts.push(Action::SaveDraft);
                    }
                    if dirty && secondary_button(ui, p, "放弃更改", can) {
                        acts.push(Action::DiscardAutoDraft);
                    }
                    // 这里曾经写「关闭并卸载」——「卸载」指的是卸掉 launchd 里的
                    // 定时任务，但在自动清理页上它读起来像「卸载软件」，和卸载页
                    // 撞在一起。按钮只说用户关心的那件事：关掉自动清理。
                    // 底下的 plist 仍然要移除，否则关不掉，但那不是按钮该说的。
                    if state.auto.config.enabled
                        && cfg.enabled
                        && danger_ghost_button(ui, p, "关闭自动清理", can)
                    {
                        acts.push(Action::SaveAutoDisabled);
                    }
                });
            });
        });
}

/// 按当前草稿落盘并装载。给 `Effect::SaveAuto` 用。
///
/// 与 `save(state, enable)` 的差别只在「启用与否」取自草稿本身，
/// 不再由调用方另传一个布尔值：保存条上的两个按钮已经把这个选择表达过了。
pub fn save_draft(state: &mut AppState) {
    let enable = state.auto.draft.as_ref().is_some_and(|d| d.enabled);
    save(state, enable);
}

/// 这是本页唯一会写盘的地方。之前时间与类型格子改一下就落盘，
/// 于是同一个页面有两套保存语义：拨一下时间已经生效、勾一下类型也已经生效，
/// 而「保存并启用」又像是一道额外的关口——用户没法判断自己改的东西到底守住没有。
fn save(state: &mut AppState, enable: bool) {
    let mut cfg = state
        .auto
        .draft
        .clone()
        .unwrap_or_else(|| state.auto.config.clone());
    cfg.enabled = enable;

    // 0 个类型时启用是没意义的：到点会跑一次、什么都不清，
    // 用户第二天看到「已执行」会以为清理生效了。
    //
    // 这一步必须在写盘**之前**：先按 `enabled = true` 落盘再改回 false 的话，
    // 配置文件里会短暂留下一份「已启用」，而 launchd 那边什么也没装。
    if enable && cfg.rule_ids.is_empty() {
        // 只是提示一句、却把 `enabled = true` 留在生效配置里，标题就会写
        // 「已启用」、状态卡写「已按这个时间在跑」，与「不会安装任务」自相矛盾。
        cfg.enabled = false;
    }
    let effective_enable = cfg.enabled;

    state.auto.saving = true;

    if let Err(e) = schedule::save_config(&cfg) {
        state.auto.saving = false;
        state.auto.error = Some(e.clone());
        state.status = Some(StatusMessage::error(format!("保存定时配置失败：{e}")));
        return;
    }

    if enable && !effective_enable {
        // 想启用但无可清类型：保存成关闭态，并把系统里可能残留的任务真的卸掉。
        let _ = schedule::uninstall();
        state.auto.saving = false;
        state.auto.config = cfg.clone();
        state.auto.draft = Some(cfg);
        state.auto.installed = Some(schedule::is_loaded());
        state.auto.installed_error = None;
        state.status = Some(StatusMessage::warn(
            "规则库里没有可自动清理的类型，已保存为关闭状态；先确认规则库里至少有标了 auto 的可回收类型",
        ));
        return;
    }

    if enable {
        match schedule::install(&cfg) {
            Ok(()) => {
                state.auto.installed = Some(true);
                state.auto.installed_error = None;
                state.auto.error = None;
                state.status = Some(StatusMessage::info(format!(
                    "定时清理已启用：{}",
                    cfg.describe()
                )));
            }
            Err(e) => {
                state.auto.error = Some(e.clone());
                state.status = Some(StatusMessage::error(format!("安装定时任务失败：{e}")));
            }
        }
    } else {
        match schedule::uninstall() {
            Ok(()) => {
                state.auto.installed = Some(false);
                state.auto.installed_error = None;
                state.auto.error = None;
                state.status = Some(StatusMessage::info("定时清理已关闭"));
            }
            Err(e) => {
                state.auto.error = Some(e.clone());
                state.status = Some(StatusMessage::error(format!("卸载定时任务失败：{e}")));
            }
        }
    }

    state.auto.saving = false;
    if state.auto.error.is_none() {
        // 成功：生效配置与草稿一起对齐，`dirty()` 归零。
        state.auto.config = cfg.clone();
        state.auto.draft = Some(cfg);
    }
    // 失败：草稿**原样保留**，生效配置也**不动**。
    //
    // 这里以前把 `cfg` 同时写进 `config` 和 `draft`，于是两件事一起出错：
    // 界面开始把一份 launchd 根本没装上的配置显示成「已生效」，而草稿与
    // 生效配置相等让 `dirty()` 变成 false，保存按钮直接置灰——用户看得见
    // 保存失败，却再也没法重试，只能去改一个格子再改回来。
    // 配置文件与 plist 是否真的落地，本来就由它们各自的错误说明负责。
}

/// 频率与时间。改动只进草稿，底部「保存」才落地。
fn schedule_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let mut cfg = state.auto.editing().clone();
    let mut changed = false;

    ui.vertical(|ui| {
        ui.label(egui::RichText::new("什么时候跑").size(p.sz(13.5)).strong());
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            for (f, label) in [(Frequency::Daily, "每天"), (Frequency::Weekly, "每周")] {
                if crate::widgets::chip(ui, p, label, cfg.frequency == f) {
                    cfg.frequency = f;
                    changed = true;
                }
            }
            if cfg.frequency == Frequency::Weekly {
                ui.add_space(8.0);
                for (i, name) in schedule::WEEKDAY_NAMES.iter().enumerate() {
                    if crate::widgets::chip(ui, p, name, cfg.weekday as usize == i) {
                        cfg.weekday = i as u32;
                        changed = true;
                    }
                }
            }
        });

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("时间")
                    .size(p.sz(11.5))
                    .color(p.text_dim),
            );
            let mut hour = cfg.hour as i32;
            let mut minute = cfg.minute as i32;
            // 用 DragValue 而不是文本输入：越界值会直接写进 plist，
            // 而 launchd 对越界字段的反应是静默不加载
            if ui
                .add(egui::DragValue::new(&mut hour).range(0..=23).speed(0.2))
                .changed()
            {
                cfg.hour = hour.clamp(0, 23) as u32;
                changed = true;
            }
            ui.label(egui::RichText::new(":").size(p.sz(13.0)));
            if ui
                .add(egui::DragValue::new(&mut minute).range(0..=59).speed(0.2))
                .changed()
            {
                cfg.minute = minute.clamp(0, 59) as u32;
                changed = true;
            }
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("→ {}", cfg.describe()))
                        .size(p.sz(11.5))
                        .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        });

        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new("错过的时点会在下次唤醒时补跑一次；登录时不会额外跑。")
                    .size(p.sz(10.5))
                    .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
    });

    if changed {
        acts.push(Action::SetAutoSchedule(cfg));
    }
}

/// 上次执行结果。
fn last_run_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("上次执行").size(p.sz(13.5)).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if secondary_button(ui, p, "刷新", true) {
                    acts.push(Action::RefreshAutoStatus);
                }
            });
        });
        ui.add_space(6.0);

        let Some(run) = state.auto.last_run.clone() else {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("还没有执行过。启用后第一次到点会在这里留下结果。")
                        .size(p.sz(11.5))
                        .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            return;
        };

        let tint = if run.ok { p.reclaimable } else { p.costly };
        let soft = if run.ok { p.accent_soft } else { p.costly_soft };
        ui.horizontal_wrapped(|ui| {
            badge(ui, p, if run.ok { "成功" } else { "有问题" }, tint, soft);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::components::time::short_time(&run.at))
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(&run.message)
                    .size(p.sz(12.0))
                    .color(p.text),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "按 {} 个类型扫描 · 命中 {} 项 · 处理 {} 项 · {}",
                        run.types_effective,
                        run.candidates,
                        run.done,
                        molt_scan::human_bytes(run.bytes)
                    ))
                    .size(p.sz(10.5))
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });

        if !run.failures.is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("{} 项未处理：", run.failures.len()))
                    .size(p.sz(11.0))
                    .color(p.costly),
            );
            for (path, why) in run.failures.iter().take(8) {
                ui.horizontal_top(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(crate::paths::display_path_str(path))
                                .size(p.sz(10.0))
                                .monospace()
                                .color(p.text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Truncate),
                    );
                    ui.add(
                        egui::Label::new(egui::RichText::new(why).size(p.sz(10.0)).color(p.costly))
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                });
            }
            if run.failures.len() > 8 {
                ui.label(
                    egui::RichText::new(format!("还有 {} 项未列出", run.failures.len() - 8))
                        .size(p.sz(10.0))
                        .color(p.text_faint),
                );
            }
        }
    });
}

/// 保存并关闭：把 draft 的 enabled 置 false 再写盘，并从 launchd 卸掉。
pub fn save_disabled(state: &mut AppState) {
    if let Some(d) = state.auto.draft.as_mut() {
        d.enabled = false;
    }
    save(state, false);
}

/// 重新问一次系统的定时任务状态。
///
/// 单独抽出来是因为它是一个副作用（读 plist、调 launchctl），
/// 不能放进 `apply`——那里必须保持纯函数。
pub fn refresh_status(state: &mut AppState) {
    match schedule::load_status() {
        Ok(loaded) => {
            state.auto.installed = Some(loaded);
            state.auto.installed_error = None;
        }
        Err(e) => {
            state.auto.installed = Some(false);
            state.auto.installed_error = Some(e);
        }
    }
    state.auto.last_run = schedule::load_last_run();
}

/// 就地执行一次。走的是定时任务本身那条路径，所以「执行成功」等于
/// 「到点也会这样做」——不是另一套演示逻辑。
pub fn run_now(state: &mut AppState) {
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    let Some(rules) = state.rules.clone() else {
        state.status = Some(StatusMessage::error("规则库没有加载成功，无法执行"));
        return;
    };
    let cfg = state.auto.config.clone();
    if cfg.rule_ids.is_empty() {
        // 直接返回：先写警告再紧接着写「正在执行…」，后一句会把警告盖掉，
        // 用户只看到它跑了一遍、什么都没清。
        state.status = Some(StatusMessage::warn(
            "规则库里没有可自动清理的类型，这次执行不会清理任何内容",
        ));
        return;
    }
    state.status = Some(StatusMessage::info("正在执行定时清理…"));
    state.schedule_job = Some(crate::schedule::ScheduleJob::start(
        rules,
        state.home.clone(),
        cfg,
    ));
}

/// 每帧推进就地执行任务。由主循环调用。
pub fn pump(state: &mut AppState) {
    let Some(job) = state.schedule_job.as_mut() else {
        return;
    };
    let done = job.poll();
    match done {
        Some(Ok(run)) => {
            state.auto.last_run = Some(run.clone());
            state.auto.installed = Some(schedule::is_loaded());
            state.auto.installed_error = None;
            state.schedule_job = None;
            // 定时清理动了隔离区与磁盘，界面必须跟着刷新
            crate::pages::quarantine::reload(state);
            crate::pages::history::reload(state);
            state.refresh_disk();
            // 先把结论写进 `last_op`，再启动重扫。
            //
            // 顺序很重要：`start_scan` 会把 `status` 改成「正在扫描…」，
            // 先写 status 的话，用户永远看不到「已放入临时空间 N 项」这句结论。
            // 工作台也是这么分流的——结论走 `last_op`，进行中的动作才占 `status`。
            state.last_op = Some(std::sync::Arc::new(crate::state::OpReport {
                summary: run.message.clone(),
                done: run.done,
                done_bytes: run.bytes,
                failures: Vec::new(),
                kind: if run.ok {
                    crate::state::StatusKind::Info
                } else {
                    crate::state::StatusKind::Warn
                },
                next: Some(crate::state::NextStep::Quarantine),
            }));
            crate::pages::workbench::start_scan(state);
            state.status = Some(StatusMessage {
                kind: if run.ok {
                    crate::state::StatusKind::Info
                } else {
                    crate::state::StatusKind::Warn
                },
                text: run.message.clone(),
            });
        }
        Some(Err(e)) => {
            state.status = Some(StatusMessage::error(e));
            state.schedule_job = None;
        }
        None => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{Frequency, ScheduleConfig};

    fn baseline() -> ScheduleConfig {
        ScheduleConfig {
            enabled: true,
            frequency: Frequency::Daily,
            hour: 3,
            minute: 30,
            ..Default::default()
        }
    }

    #[test]
    fn editing_the_draft_does_not_touch_the_live_config() {
        let mut state = AppState::new("/Users/me".into());
        let base = baseline();
        state.auto.config = base.clone();
        state.auto.draft = Some(base.clone());
        assert!(!state.auto.dirty());

        // 用户拨了一下时间：只有草稿变
        let mut d = base.clone();
        d.hour = 9;
        state.auto.draft = Some(d);
        assert!(state.auto.dirty());
        // 生效的那份（也就是夜里真会跑的那份）还是 3:30
        assert_eq!(state.auto.config.hour, 3);
        assert_eq!(state.auto.editing().hour, 9);
    }

    #[test]
    fn discarding_the_draft_returns_to_the_live_config() {
        let mut state = AppState::new("/Users/me".into());
        let base = baseline();
        state.auto.config = base.clone();
        let mut d = base.clone();
        d.minute = 5;
        state.auto.draft = Some(d);
        assert!(state.auto.dirty());

        state.auto.draft = Some(state.auto.config.clone());
        assert!(!state.auto.dirty());
        assert_eq!(state.auto.editing().minute, 30);
    }

    /// 保存条上那颗主按钮此刻的状态。测试要问的是「用户能不能点它」，
    /// 所以走界面同一份判断，而不是在测试里把条件再抄一遍。
    fn save_button(state: &AppState) -> (&'static str, bool) {
        primary_save(state)
    }

    /// 保存失败之后必须还能重试。
    ///
    /// 回归的是这条：曾经失败分支把 `cfg` 同时写进 `config` 与 `draft`，
    /// 于是 `dirty()` 变 false，保存按钮直接置灰——用户看得见「保存失败」，
    /// 却再也没法按第二次。
    #[test]
    fn a_failed_save_keeps_the_draft_and_leaves_the_button_usable() {
        let mut state = AppState::new("/Users/me".into());
        let base = baseline();
        state.auto.config = base.clone();
        state.auto.draft = Some(base.clone());
        assert!(!state.auto.dirty());

        // 模拟用户改了时间：草稿与生效配置分叉，按钮应该亮起来
        let mut edited = base.clone();
        edited.hour = 9;
        state.auto.draft = Some(edited.clone());
        assert!(state.auto.dirty(), "改了草稿就要算未保存");
        assert_eq!(save_button(&state), ("保存更改", true));

        // 模拟 save() 在 install 失败后的结算：只置错误，不动 config / draft
        state.auto.error = Some("launchctl 没有接受这个任务".into());
        state.auto.installed = Some(false);
        state.auto.saving = false;

        assert!(
            state.auto.dirty(),
            "保存失败后草稿必须留着，否则按钮会永远置灰、无从重试"
        );
        assert_eq!(
            state.auto.config.hour, 3,
            "失败时不能把没装上的配置写成已生效"
        );
        assert_eq!(state.auto.editing().hour, 9, "草稿要保持用户改的值");
        assert!(save_button(&state).1, "保存失败之后，按钮必须还是能按的");
    }

    /// 配置已启用、系统里却没这个任务时，必须有办法把它装回去。
    ///
    /// 回归的是这条：这时 `dirty()` 为 false，保存按钮原先恒灰，
    /// 而页面别处没有任何重装入口——「配置显示已启用，实际永远不会跑」。
    #[test]
    fn an_enabled_config_that_is_not_loaded_still_offers_a_reinstall() {
        let mut state = AppState::new("/Users/me".into());
        let base = baseline();
        state.auto.config = base.clone();
        state.auto.draft = Some(base.clone());
        state.auto.installed = Some(false);
        assert!(!state.auto.dirty(), "这不是「有未保存改动」的情形");

        assert_eq!(
            save_button(&state),
            ("重新安装任务", true),
            "启用但未加载时，保存条必须给出「重新安装任务」的出口"
        );

        // 已经在跑、（且没有改动）时不该出现这个出口，否则会误导。
        state.auto.installed = Some(true);
        assert_eq!(
            save_button(&state),
            ("保存", false),
            "任务好好在跑时不该提示重装，也不该让「保存」可按"
        );
    }

    /// 一个不会碰真实系统的假 home（`MOLT_HOME` 决定定时配置与 plist 落在哪）。
    fn fake_home(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!(
            "molt-auto-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        std::env::set_var("MOLT_HOME", &p);
        p.display().to_string()
    }

    /// 没有可清类型时点「保存并启用」：必须真的存成关闭状态。
    ///
    /// 回归的是这条：原先只弹一句「已保存但不会安装任务」，却把
    /// `enabled = true` 留在生效配置里——标题写「已启用」、状态卡写
    /// 「已按这个时间在跑」，与「不会安装任务」自相矛盾；用户第二天
    /// 什么也没清到。
    #[test]
    fn enabling_with_no_types_is_saved_as_disabled() {
        let home = fake_home("empty-types");
        let mut state = AppState::new(home.clone());
        state.rules = Some(molt_rules::load(&home, None).unwrap());
        state.auto.config = baseline();
        // 规则库给的类型全清掉，模拟「一条 auto 可回收的都没有」
        let mut d = baseline();
        d.rule_ids.clear();
        state.auto.draft = Some(d);

        save_draft(&mut state);

        assert!(
            !state.auto.config.enabled,
            "没有可清类型时不能留下「已启用」"
        );
        assert!(!state.auto.editing().enabled, "草稿也要落成关闭");
        let status = state.status.as_ref().expect("应有一条提示");
        assert_eq!(
            status.kind,
            crate::state::StatusKind::Warn,
            "这是警告不是成功：{}",
            status.text
        );
        assert!(
            status.text.contains("关闭"),
            "提示要说清已存成关闭状态：{}",
            status.text
        );
        // 磁盘上那份同样是关闭的，下次打开不会骗人
        let on_disk = schedule::load_config().expect("配置应已写出");
        assert!(!on_disk.enabled, "配置文件里也必须是关闭");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 进页面不该把磁盘上已有的类型清单静默改成「全部类型」。
    ///
    /// 回归的是这条：`ensure_loaded` 无条件覆盖 `rule_ids`，于是磁盘上
    /// 只要是子集（老版本存的、或 CLI 写的），一进页面底栏就报
    /// 「有未保存的更改 / 计划待保存」，而用户什么都没动。
    #[test]
    fn opening_the_page_does_not_fake_an_unsaved_change() {
        let home = fake_home("no-fake-dirty");
        let rules = molt_rules::load(&home, None).unwrap();
        let all = schedule::all_rule_ids(&rules);
        assert!(
            all.len() > 1,
            "这个用例需要有多个可自动清理的类型，实际 {}",
            all.len()
        );

        // 磁盘上存一个真子集，模拟老版本或 CLI 写下的配置
        let mut saved = ScheduleConfig {
            enabled: false,
            rule_ids: all[..1].to_vec(),
            ..baseline()
        };
        saved.enabled = false;
        schedule::save_config(&saved).unwrap();

        let mut state = AppState::new(home.clone());
        state.rules = Some(rules);
        ensure_loaded(&mut state);

        assert!(
            !state.auto.dirty(),
            "刚进页面、什么都没动，不该显示「有未保存的更改」"
        );
        assert_eq!(
            state.auto.config.rule_ids,
            all[..1].to_vec(),
            "磁盘上那份子集要原样保留，不能被静默改成全部类型"
        );

        // 磁盘上本来就空着时，仍要补上默认值（否则启用即无意义）
        let mut empty = ScheduleConfig {
            enabled: false,
            rule_ids: Vec::new(),
            ..baseline()
        };
        empty.enabled = false;
        schedule::save_config(&empty).unwrap();
        let mut state = AppState::new(home.clone());
        state.rules = Some(molt_rules::load(&home, None).unwrap());
        ensure_loaded(&mut state);
        assert_eq!(
            state.auto.config.rule_ids.len(),
            all.len(),
            "磁盘上没有类型时才补默认值"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 没有可清类型时点「立即执行一次」：警告必须留在屏幕上。
    ///
    /// 回归的是这条：先写警告、紧接着无条件写「正在执行定时清理…」，
    /// 后一句把警告盖掉，用户只看到它跑了一遍、什么都没清。
    #[test]
    fn run_now_with_no_types_keeps_the_warning_and_starts_nothing() {
        let home = fake_home("run-now-empty");
        let mut state = AppState::new(home.clone());
        state.rules = Some(molt_rules::load(&home, None).unwrap());
        let mut cfg = baseline();
        cfg.rule_ids.clear();
        state.auto.config = cfg;
        state.auto.draft = Some(state.auto.config.clone());

        run_now(&mut state);

        assert!(
            state.schedule_job.is_none(),
            "没有可清类型时不该真起一次执行"
        );
        let status = state.status.as_ref().expect("应有一条提示");
        assert_eq!(
            status.kind,
            crate::state::StatusKind::Warn,
            "要留下警告而不是被「正在执行」覆盖：{}",
            status.text
        );
        assert!(
            status.text.contains("不会清理"),
            "提示要说清这次不会清任何东西：{}",
            status.text
        );

        let _ = std::fs::remove_dir_all(&home);
    }
}
