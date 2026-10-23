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

use crate::schedule::{self, Frequency};
use crate::state::{AppState, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{
    badge, card, danger_ghost_button, page_header, primary_button, secondary_button, state_block,
};

/// 进入这一页时读一次配置与状态。读文件有开销，不能每帧做。
pub fn ensure_loaded(state: &mut AppState) {
    if state.auto.loaded {
        return;
    }
    match schedule::load_config() {
        Ok(mut cfg) => {
            // 首次进入：把「全部可自动清理的类型」勾上。
            // 空列表更危险——用户会以为开了就会清，实际一个都不清。
            if cfg.rule_ids.is_empty() && !cfg.enabled {
                if let Some(rules) = &state.rules {
                    cfg.rule_ids = schedule::all_rule_ids(rules);
                }
            }
            state.auto.config = cfg;
            state.auto.error = None;
        }
        Err(e) => state.auto.error = Some(e),
    }
    state.auto.last_run = schedule::load_last_run();
    state.auto.installed = Some(schedule::is_loaded());
    state.auto.loaded = true;
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    ensure_loaded(state);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            page_header(
                ui,
                p,
                "自动清理",
                "到点自动清你勾选的那几类。只移入隔离区、不直接删除，清完还能还原。",
                |ui| {
                    if secondary_button(ui, p, "立即试跑一次", state.can_start()) {
                        run_now(state);
                    }
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
                        state.auto.loaded = false;
                    }
                });
                ui.add_space(10.0);
            }

            status_card(ui, state, p);
            ui.add_space(10.0);
            schedule_card(ui, state, p);
            ui.add_space(10.0);
            kinds_card(ui, state, p);
            ui.add_space(10.0);
            last_run_card(ui, state, p);
        });
}

/// 顶部：开没开、什么时候跑、launchd 里到底有没有。
fn status_card(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let cfg = state.auto.config.clone();
    let installed = state.auto.installed;
    let running = state.is_scheduled();

    card(ui, p, |ui| {
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
                badge(ui, p, "正在试跑", p.costly, p.costly_soft);
            }
        });

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
            let enabled = state.can_start() && !state.auto.saving;
            if primary_button(ui, p, if cfg.enabled { "保存并启用" } else { "保存" }, enabled) {
                save(state, cfg.enabled);
            }
            if cfg.enabled && danger_ghost_button(ui, p, "关闭并卸载", enabled) {
                let mut off = cfg.clone();
                off.enabled = false;
                save(state, false);
                let _ = off;
            }
            if secondary_button(ui, p, "刷新状态", enabled) {
                state.auto.installed = Some(schedule::is_loaded());
                state.auto.last_run = schedule::load_last_run();
            }
            if state.is_busy() {
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

fn save(state: &mut AppState, enable: bool) {
    let mut cfg = state.auto.config.clone();
    cfg.enabled = enable;

    if let Err(e) = schedule::save_config(&cfg) {
        state.auto.error = Some(e.clone());
        state.status = Some(StatusMessage::error(format!("保存定时配置失败：{e}")));
        return;
    }

    if enable {
        // 0 个类型时启用是没意义的：到点会跑一次、什么都不清，
        // 用户第二天看到「已执行」会以为清理生效了
        if cfg.rule_ids.is_empty() {
            state.status = Some(StatusMessage::warn(
                "还没有选中任何类型，已保存但不会安装任务——先勾选要清理的类型",
            ));
            state.auto.config = cfg;
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
    state.auto.config = cfg;
}

/// 频率与时间。
fn schedule_card(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let mut cfg = state.auto.config.clone();
    let mut changed = false;

    card(ui, p, |ui| {
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
        state.auto.config = cfg;
        // 改时间后若已启用，立即重装：只改文件不重装的话，
        // 系统里跑的还是旧时间，而界面显示的是新时间
        if state.auto.config.enabled && state.can_start() {
            if let Err(e) = schedule::save_config(&state.auto.config) {
                state.auto.error = Some(e);
            } else if let Err(e) = schedule::install(&state.auto.config) {
                state.auto.error = Some(e.clone());
                state.status = Some(StatusMessage::error(format!("重装定时任务失败：{e}")));
            } else {
                state.auto.installed = Some(true);
            }
        }
    }
}

/// 清哪些类型。列的是规则库里的真实规则，不是另编的分类。
fn kinds_card(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let kinds = state
        .rules
        .as_ref()
        .map(schedule::kinds)
        .unwrap_or_default();
    let mut cfg = state.auto.config.clone();
    let mut changed = false;

    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("清哪些类型")
                    .size(p.sz(13.5))
                    .strong(),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new("只列出规则里标了「可自动清理」且删了能自动重建的类型")
                        .size(p.sz(10.5))
                        .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if secondary_button(ui, p, "全不选", !kinds.is_empty()) {
                    cfg.rule_ids.clear();
                    changed = true;
                }
                if secondary_button(ui, p, "全选", !kinds.is_empty()) {
                    cfg.rule_ids = kinds.iter().map(|k| k.rule_id.clone()).collect();
                    changed = true;
                }
            });
        });
        ui.add_space(8.0);

        if kinds.is_empty() {
            state_block(
                ui,
                p,
                "○",
                "规则库里没有可自动清理的类型",
                "这意味着所有规则都需要有人看着才能清。可以到「设置」查看规则库是否加载成功。",
                p.text_faint,
            );
            return;
        }

        // 按组列，组内逐条勾选。勾的是规则本身，界面上显示的是规则里的 what，
        // 所以文案不会和规则库漂移。
        let mut last_group = "";
        for k in &kinds {
            if k.group != last_group {
                if !last_group.is_empty() {
                    ui.add_space(6.0);
                }
                ui.label(
                    egui::RichText::new(k.group)
                        .size(p.sz(11.0))
                        .strong()
                        .color(p.text_dim),
                );
                last_group = k.group;
            }
            let mut on = cfg.is_selected(&k.rule_id);
            if ui
                .checkbox(
                    &mut on,
                    egui::RichText::new(&k.what).size(p.sz(11.5)),
                )
                .changed()
            {
                if on && !cfg.is_selected(&k.rule_id) {
                    cfg.rule_ids.push(k.rule_id.clone());
                } else if !on {
                    cfg.rule_ids.retain(|r| r != &k.rule_id);
                }
                changed = true;
            }
        }
    });

    if changed {
        state.auto.config = cfg;
        // 已启用时把选择立刻落盘：否则用户勾完以为生效了，
        // 夜里那次跑的还是上一份选择
        if state.auto.config.enabled {
            if let Err(e) = schedule::save_config(&state.auto.config) {
                state.auto.error = Some(e.clone());
                state.status = Some(StatusMessage::error(format!("保存失败：{e}")));
            }
        }
    }
}

/// 上次执行结果。
fn last_run_card(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    card(ui, p, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("上次执行")
                    .size(p.sz(13.5))
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if secondary_button(ui, p, "刷新", true) {
                    state.auto.last_run = schedule::load_last_run();
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
                    egui::RichText::new(short_time(&run.at))
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
                        "按 {} 个类型扫描（勾选 {} 个）· 命中 {} 项 · 处理 {} 项 · {}",
                        run.types_effective,
                        run.types,
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
                        egui::Label::new(
                            egui::RichText::new(why).size(p.sz(10.0)).color(p.costly),
                        )
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

fn short_time(iso: &str) -> String {
    if iso.len() >= 16 {
        format!("{} {}", &iso[0..10], &iso[11..16])
    } else {
        iso.to_string()
    }
}

/// 就地试跑一次。走的是定时任务本身那条路径，所以「试跑成功」等于
/// 「到点也会这样做」——不是另一套演示逻辑。
fn run_now(state: &mut AppState) {
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    let Some(rules) = state.rules.clone() else {
        state.status = Some(StatusMessage::error("规则库没有加载成功，无法试跑"));
        return;
    };
    let cfg = state.auto.config.clone();
    if cfg.rule_ids.is_empty() {
        state.status = Some(StatusMessage::warn("还没有选中任何类型，试跑不会清理任何内容"));
    }
    state.status = Some(StatusMessage::info("正在执行定时清理…"));
    state.schedule_job = Some(crate::schedule::ScheduleJob::start(
        rules,
        state.home.clone(),
        cfg,
    ));
}

/// 每帧推进试跑任务。由主循环调用。
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
