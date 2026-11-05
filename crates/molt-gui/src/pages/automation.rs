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
            if let Some(rules) = &state.rules {
                cfg.rule_ids = schedule::all_rule_ids(rules);
            }
            state.auto.config = cfg;
            state.auto.draft = Some(state.auto.config.clone());
            state.auto.error = None;
        }
        Err(e) => state.auto.error = Some(e),
    }
    state.auto.last_run = schedule::load_last_run();
    state.auto.installed = Some(schedule::is_loaded());
    state.auto.loaded = true;
}

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    // 保存条固定在底部：改动后不管滚到哪里都能看见「有一份没保存」。
    save_bar(ui, state, p, acts);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            page_header(
                ui,
                p,
                "自动清理",
                "到点自动清规则库里标了「可自动清理」的那几类。只移入隔离区、不直接删除，清完还能还原。",
                |ui| {
                    ui.vertical(|ui| {
                        if secondary_button(ui, p, "立即执行一次", state.can_start()) {
                            acts.push(Action::RunScheduledNow);
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("按已保存的配置移入临时空间，可还原")
                                    .size(p.sz(9.5))
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                    });
                },
            );
            ui.add_space(12.0);

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

/// 顶部：开没开、什么时候跑、launchd 里到底有没有。
fn status_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let cfg = state.auto.config.clone();
    let installed = state.auto.installed;
    let running = state.is_scheduled();

    ui.vertical(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new("状态").size(p.sz(14.0)).strong())
                    .wrap_mode(egui::TextWrapMode::Extend),
            );

            let (label, fg, bg) = match (cfg.enabled, installed) {
                (false, _) => ("已关闭", p.text_dim, p.surface_alt),
                (true, Some(true)) => ("已启用，系统里已加载", p.reclaimable, p.accent_soft),
                (true, Some(false)) => ("已启用，但系统里没加载", p.costly, p.costly_soft),
                (true, None) => ("已启用，正在查询任务状态", p.text_dim, p.surface_alt),
            };
            badge(ui, p, label, fg, bg);
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
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "配置文件说已启用，但 launchd 里没有这个任务，到点不会执行。点「保存并启用」可以重新装载。",
                    )
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
                        let text = if loaded == Some(false) {
                            format!("配置已启用，但系统里没加载：{}", cfg.describe())
                        } else {
                            format!("已按这个时间在跑：{}", cfg.describe())
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
                    let primary_label = match (state.auto.config.enabled, dirty) {
                        (false, _) => "保存并启用",
                        (true, true) => "保存更改",
                        (true, false) => "保存",
                    };
                    if primary_button(
                        ui,
                        p,
                        primary_label,
                        can && (dirty || !state.auto.config.enabled),
                    ) {
                        acts.push(Action::SaveDraft);
                    }
                    if dirty && secondary_button(ui, p, "放弃更改", can) {
                        acts.push(Action::DiscardAutoDraft);
                    }
                    // 这里曾经写「关闭并卸载」——「卸载」指的是卸掉 launchd 里的
                    // 定时任务，但在自动清理页上它读起来像「卸载软件」，和卸载页
                    // 撞在一起。按钮只说用户关心的那件事：关掉自动清理。
                    // 底下的 plist 仍然要移除，否则关不掉，但那不是按钮该说的。
                    if state.auto.config.enabled && danger_ghost_button(ui, p, "关闭自动清理", can)
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

    state.auto.saving = true;

    if let Err(e) = schedule::save_config(&cfg) {
        state.auto.saving = false;
        state.auto.error = Some(e.clone());
        state.status = Some(StatusMessage::error(format!("保存定时配置失败：{e}")));
        return;
    }

    if enable {
        // 0 个类型时启用是没意义的：到点会跑一次、什么都不清，
        // 用户第二天看到「已执行」会以为清理生效了
        if cfg.rule_ids.is_empty() {
            state.auto.saving = false;
            state.status = Some(StatusMessage::warn(
                "还没有选中任何类型，已保存但不会安装任务——先勾选要清理的类型",
            ));
            state.auto.config = cfg.clone();
            state.auto.draft = Some(cfg);
            return;
        }
        match schedule::install(&cfg) {
            Ok(()) => {
                state.auto.installed = Some(schedule::is_loaded());
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
    // 落盘成功与否都要让「生效的那份」与草稿重新对齐：
    // 失败时草稿留着，用户还能重试，而不是白改一遍。
    if state.auto.error.is_none() {
        state.auto.config = cfg.clone();
        state.auto.draft = Some(cfg);
    } else {
        state.auto.config = cfg;
        state.auto.draft = Some(state.auto.config.clone());
    }
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
    state.auto.installed = Some(schedule::is_loaded());
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
        state.status = Some(StatusMessage::warn(
            "还没有选中任何类型，执行一次不会清理任何内容",
        ));
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
            state.status = Some(StatusMessage {
                kind: if run.ok {
                    crate::state::StatusKind::Info
                } else {
                    crate::state::StatusKind::Warn
                },
                text: run.message.clone(),
            });
            state.auto.last_run = Some(run.clone());
            state.auto.installed = Some(schedule::is_loaded());
            state.schedule_job = None;
            // 定时清理动了隔离区与磁盘，界面必须跟着刷新
            crate::pages::quarantine::reload(state);
            crate::pages::history::reload(state);
            state.refresh_disk();
            crate::pages::workbench::start_scan(state);
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
}
