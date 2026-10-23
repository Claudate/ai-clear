//! 设置：AI 配置（含 jev 的完整内容）与规则库信息。
//!
//! 「jev 的配置界面上完全看不到」是这个页面存在的直接原因，所以它做了两件事：
//! 1. 把每家服务、每个模型、密钥来源、协议、地址**原样列出来**，不是只给一个下拉框；
//! 2. 当前生效的那一份（解析结果）单独一块，用户能确认「点分析时实际用的是谁」。
//!
//! 密钥永远只显示「来自哪个环境变量」或「已设置（已隐去）」，绝不显示原值——
//! 这个窗口会被截图、会被共享屏幕。

use eframe::egui;

use crate::state::{AppState, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{badge, card, kv, page_header, secondary_button};

pub fn show(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            page_header(
                ui,
                p,
                "设置",
                "AI 只用来看懂每一条是什么；判定与清理始终由本地规则完成。",
                |ui| {
                    if secondary_button(ui, p, "重新读取配置", !state.is_busy()) {
                        state.ai.reload_config();
                        state.status = Some(StatusMessage::info("已重新读取 AI 配置"));
                    }
                },
            );
            ui.add_space(12.0);

            active_card(ui, state, p);
            ui.add_space(10.0);
            // jev 紧跟「当前生效」：它是删除判断的默认模型，
            // 用户最常来这一页就是确认它是否可用，不该滚到页尾才看到。
            jev_card(ui, state, p);
            ui.add_space(10.0);
            provider_cards(ui, state, p);
            ui.add_space(10.0);
            rules_card(ui, state, p);
        });
}

/// 顶部：当前实际生效的服务与模型。用户配了一个却看到界面在用另一个，
/// 是这类功能最伤信任的失败方式。
fn active_card(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("当前生效").size(p.sz(14.0)).strong());
            match state.ai.resolve() {
                Ok(r) => {
                    badge(ui, p, &r.summary(), p.reclaimable, p.accent_soft);
                    badge(ui, p, r.kind.label(), p.text_dim, p.surface_alt);
                }
                Err(reason) => {
                    badge(ui, p, "未就绪", p.costly, p.costly_soft);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(reason)
                                .size(p.sz(11.0))
                                .color(p.text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
            }
        });

        if let Ok(r) = state.ai.resolve() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                kv(ui, p, "接口地址", &r.base_url);
                kv(ui, p, "模型 id", &r.model);
                kv(ui, p, "上下文", &format!("{} tokens", r.context_tokens));
                kv(ui, p, "JSON 模式", if r.json_mode { "开" } else { "关" });
            });
        }

        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "配置文件：{}（改完点下面「保存」才会写回磁盘）",
                    state.ai.config_path.display()
                ))
                .size(p.sz(10.0))
                .monospace()
                .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        if let Some(err) = &state.ai.config_error {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("配置读取失败：{err}"))
                        .size(p.sz(11.0))
                        .color(p.danger),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
    });
}

/// 每家服务一张卡：协议、地址、密钥来源、可选模型。
///
/// 用卡片而不是下拉框，是因为「你还能配什么」本身是信息：
/// 用户看到 jev 就在列表里、密钥来自 `$TYPESAFE_API_KEY`，才知道该去设哪个变量。
fn provider_cards(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    let providers = state.ai.config.providers.clone();
    let active = state.ai.config.active_provider.clone();
    let mut pick: Option<(String, String)> = None;
    let mut inspect: Option<String> = None;

    for prov in &providers {
        card(ui, p, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(prov.display())
                            .size(p.sz(13.5))
                            .strong(),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                if prov.id == active {
                    badge(ui, p, "使用中", p.reclaimable, p.accent_soft);
                }
                if prov.official {
                    badge(ui, p, "官方", p.text_dim, p.surface_alt);
                }
                badge(ui, p, prov.kind.label(), p.text_dim, p.surface_alt);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&prov.id)
                            .size(p.sz(10.0))
                            .monospace()
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, p, "详情", true) {
                        inspect = Some(prov.id.clone());
                    }
                    if prov.id != active && secondary_button(ui, p, "用这一家", !state.is_busy()) {
                        pick = Some((prov.id.clone(), String::new()));
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                kv(ui, p, "地址", &prov.base_url);
                ui.add_space(10.0);
                // 密钥只显示来源，绝不显示值
                let key = if prov.key_from_env() {
                    format!("密钥来自 ${}", prov.api_key_env)
                } else if prov.api_key.trim().is_empty() {
                    if prov.api_key_env.is_empty() {
                        "还没有密钥".to_string()
                    } else {
                        format!("还没有密钥（可设 ${}，或写在配置里）", prov.api_key_env)
                    }
                } else {
                    "密钥写在配置文件里（已隐去）".to_string()
                };
                kv(ui, p, "密钥", &key);
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("模型")
                        .size(p.sz(11.0))
                        .color(p.text_faint),
                );
                if prov.models.is_empty() {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("这家在配置里没有列出模型")
                                .size(p.sz(11.0))
                                .color(p.costly),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
                for m in &prov.models {
                    let selected = prov.id == active && state.ai.config.active_model == m.id;
                    if crate::widgets::chip(ui, p, &m.label_or_id(), selected)
                        && !state.is_busy()
                    {
                        pick = Some((prov.id.clone(), m.id.clone()));
                    }
                }
            });

            if state.ai.inspect.as_deref() == Some(prov.id.as_str()) {
                ui.add_space(6.0);
                full_provider(ui, p, prov);
            }
        });
        ui.add_space(8.0);
    }

    if let Some(id) = inspect {
        state.ai.inspect = if state.ai.inspect.as_deref() == Some(id.as_str()) {
            None
        } else {
            Some(id)
        };
    }

    if let Some((provider, model)) = pick {
        if state.can_start() {
            state.ai.config.set_provider(&provider);
            if !model.is_empty() {
                state.ai.config.active_model = model;
            }
            match state.ai.config.save(&state.ai.config_path) {
                Ok(()) => {
                    state.ai.config_error = None;
                    state.status = Some(StatusMessage::info(format!(
                        "已切换到 {}",
                        state.ai.resolve().map(|r| r.summary()).unwrap_or(provider)
                    )));
                }
                Err(e) => {
                    state.ai.config_error = Some(e.to_string());
                    state.status = Some(StatusMessage::error(format!("保存失败：{e}")));
                }
            }
        }
    }

    ui.horizontal(|ui| {
        let mut sample = state.ai.config.allow_content_sample;
        if ui
            .checkbox(
                &mut sample,
                egui::RichText::new("允许采样文件开头内容（默认关闭；命中疑似密钥的文件会被跳过）")
                    .size(p.sz(11.0)),
            )
            .changed()
        {
            state.ai.config.allow_content_sample = sample;
            if let Err(e) = state.ai.config.save(&state.ai.config_path) {
                state.ai.config_error = Some(e.to_string());
            }
        }
    });
}

/// 「详情」展开的完整字段。这一块是「jev 的配置要能查到并完整展示」的落点。
fn full_provider(ui: &mut egui::Ui, p: &Palette, prov: &molt_ai::Provider) {
    egui::Frame::NONE
        .fill(p.surface_alt)
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            kv(ui, p, "id", &prov.id);
            kv(ui, p, "显示名", prov.display());
            kv(ui, p, "协议", prov.kind.label());
            kv(ui, p, "base_url", &prov.base_url);
            kv(
                ui,
                p,
                "密钥环境变量",
                if prov.api_key_env.is_empty() {
                    "（未指定）"
                } else {
                    &prov.api_key_env
                },
            );
            kv(
                ui,
                p,
                "密钥来源",
                if prov.key_from_env() {
                    "环境变量（优先）"
                } else if prov.api_key.trim().is_empty() {
                    "未配置"
                } else {
                    "配置文件内（已隐去）"
                },
            );
            kv(ui, p, "官方", if prov.official { "是" } else { "否" });
            for m in &prov.models {
                kv(
                    ui,
                    p,
                    &format!("模型 {}", m.id),
                    &format!(
                        "{} · 上下文 {} · JSON 模式{}",
                        m.label_or_id(),
                        m.context_tokens,
                        if m.json_mode { "开" } else { "关" }
                    ),
                );
            }
        });
}

/// jev 单独一块：它是删除判断的默认模型，用户必须能一眼确认它是否可用。
fn jev_card(ui: &mut egui::Ui, state: &mut AppState, p: &Palette) {
    card(ui, p, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("删除判断默认模型：jev")
                    .size(p.sz(14.0))
                    .strong(),
            );
            let provider = state.ai.jev().cloned();
            match &provider {
                Some(prov) => {
                    if prov.key().is_some() {
                        badge(ui, p, "密钥已就绪", p.reclaimable, p.accent_soft);
                    } else {
                        badge(ui, p, "缺密钥", p.costly, p.costly_soft);
                    }
                    badge(ui, p, prov.kind.label(), p.text_dim, p.surface_alt);
                }
                None => {
                    badge(ui, p, "配置里没有这一家", p.danger, p.danger_soft);
                }
            }
        });
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "没在别处选过模型时，删除判断会用它；你选了别的模型就按你的选择走，不会被改回 jev。",
                )
                .size(p.sz(11.0))
                .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );

        let Some(prov) = state.ai.jev().cloned() else {
            return;
        };
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.y = 3.0;
        kv(ui, p, "服务 id", &prov.id);
        kv(ui, p, "协议", &format!("{}（不是聊天接口）", prov.kind.label()));
        kv(ui, p, "base_url", &prov.base_url);
        kv(
            ui,
            p,
            "请求路径",
            &format!("{}/v1/systemone", prov.base_url.trim_end_matches('/')),
        );
        kv(
            ui,
            p,
            "密钥环境变量",
            if prov.api_key_env.is_empty() {
                "（未指定）"
            } else {
                &prov.api_key_env
            },
        );
        kv(
            ui,
            p,
            "密钥状态",
            if prov.key_from_env() {
                "已从环境变量读到"
            } else if prov.key().is_some() {
                "写在配置文件里（已隐去）"
            } else {
                "未配置：设置环境变量后重新打开本窗口即可"
            },
        );
        for m in &prov.models {
            kv(
                ui,
                p,
                "模型",
                &format!("{}（id={}, 上下文 {}）", m.label_or_id(), m.id, m.context_tokens),
            );
        }

        // 当前是否真的在用 jev：这比「这家是否可用」更贴近用户想问的那件事
        ui.add_space(6.0);
        let using = state
            .ai
            .resolve()
            .map(|r| r.provider_id == molt_ai::config::JEV_PROVIDER_ID)
            .unwrap_or(false);
        badge(
            ui,
            p,
            if using {
                "现在用的就是 jev"
            } else {
                "现在用的是别的模型"
            },
            if using { p.reclaimable } else { p.text_dim },
            if using { p.accent_soft } else { p.surface_alt },
        );

        if !state.is_busy() && secondary_button(ui, p, "改用 jev", state.ai.jev().is_some()) {
            state.ai.config.set_provider(molt_ai::config::JEV_PROVIDER_ID);
            match state.ai.config.save(&state.ai.config_path) {
                Ok(()) => {
                    state.ai.config_error = None;
                    state.status = Some(StatusMessage::info("已改用 jev"));
                }
                Err(e) => state.status = Some(StatusMessage::error(format!("保存失败：{e}"))),
            }
        }
    });
}

fn rules_card(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    card(ui, p, |ui| {
        ui.label(egui::RichText::new("规则库").size(p.sz(13.5)).strong());
        ui.add_space(4.0);
        match &state.rules {
            Some(r) => {
                let auto = r.iter().filter(|x| x.auto).count();
                ui.horizontal_wrapped(|ui| {
                    kv(ui, p, "内置 + 用户规则", &format!("{} 条", r.len()));
                    kv(ui, p, "其中可自动清理", &format!("{auto} 条"));
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "用户规则目录：{}",
                            molt_act::paths::user_rules_dir().display()
                        ))
                        .size(p.sz(10.0))
                        .monospace()
                        .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            None => {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("规则库没有加载成功，扫描会因此失败。")
                            .size(p.sz(11.5))
                            .color(p.danger),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        }

        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "应用数据：{}（隔离区、审计库、定时任务结果都在这里）",
                    molt_act::paths::app_dir().display()
                ))
                .size(p.sz(10.0))
                .monospace()
                .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
    });
}
