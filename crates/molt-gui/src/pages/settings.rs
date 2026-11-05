//! 设置：模型功能（选哪家服务、哪个模型）与规则库信息。
//!
//! 普通用户不需要看到配置文件的原始内容（服务 id、base_url、请求路径、
//! 密钥环境变量名这些是给配置作者看的，不是给使用者看的），
//! 所以这一页只回答两件事：
//! 1. 现在用的是哪个模型、密钥备好了没有；
//! 2. 规则库和应用数据放在哪里。
//!
//! 密钥永远只显示「已设置」或「未设置」，绝不显示原值——
//! 这个窗口会被截图、会被共享屏幕。

use eframe::egui;

use crate::action::Action;
use crate::state::{AppState, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{badge, kv, page_header, plain_section, secondary_button};

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            page_header(
                ui,
                p,
                "设置",
                "AI 解释是可选的；不配置也能正常扫描和清理。",
                |ui| {
                    if secondary_button(ui, p, "重新读取配置", !state.is_busy()) {
                        acts.push(Action::ReloadAiConfig);
                    }
                },
            );
            ui.add_space(12.0);

            // 纵向分组而不是卡片堆：设置页是「一条一条读下去」的，
            // 每块套一张白卡会把它切成互不相干的纸片。
            plain_section(ui, p, |ui| model_card(ui, state, p, acts));
            plain_section(ui, p, |ui| advanced_card(ui, state, p, acts));
            plain_section(ui, p, |ui| rules_card(ui, state, p));
        });
}

/// 模型功能：当前用哪个模型 + 选择服务与模型。
///
/// 主视线只放用户在意的三件事：现在用的是谁、密钥备好了没有、想换哪家 / 哪个模型。
/// 配置文件里的原始字段（id、base_url、请求路径、密钥环境变量名）收进「高级」——
/// 那是排障时才要的东西，放在主视觉里只会把可做的操作挤走。
fn model_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new("模型功能").size(p.sz(13.5)).strong());
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "AI 只用来解释每一条是什么；判定与清理始终由本地规则完成。不配置也能正常扫描与手动清理。",
                )
                .size(p.sz(11.0))
                .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(10.0);

        // 当前生效：一句话说清现在用的是谁、密钥备好没有。
        match state.ai.resolve() {
            Ok(r) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("当前使用")
                            .size(p.sz(11.0))
                            .color(p.text_faint),
                    );
                    badge(ui, p, &r.summary(), p.reclaimable, p.accent_soft);
                });
            }
            Err(reason) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("当前使用")
                            .size(p.sz(11.0))
                            .color(p.text_faint),
                    );
                    badge(ui, p, "尚未配置", p.costly, p.costly_soft);
                });
                ui.add_space(4.0);
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
        ui.add_space(8.0);

        // 选哪家服务。只显示名称与密钥是否就绪，不显示任何原始字段。
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("服务")
                    .size(p.sz(11.0))
                    .color(p.text_faint),
            );
            let providers = state.ai.config.providers.clone();
            let active = state.ai.config.active_provider.clone();
            let mut pick: Option<String> = None;
            for prov in &providers {
                let selected = prov.id == active;
                if crate::widgets::chip(ui, p, &provider_label(prov), selected) && !state.is_busy()
                {
                    pick = Some(prov.id.clone());
                }
            }
            if let Some(id) = pick {
                acts.push(Action::SetAiProvider(id));
            }
        });
        ui.add_space(8.0);

        // 选哪个模型。之前只到「服务」这一层，用户换了服务就等于被迫用它家的
        // 第一个模型，而配置里其实列了好几个（例如 jev 与 Claude Haiku 4.5）。
        let providers = state.ai.config.providers.clone();
        let active = state.ai.config.active_provider.clone();
        let selected_model = state.ai.config.active_model.clone();
        let models: Vec<(String, String)> = providers
            .iter()
            .find(|p| p.id == active)
            .map(|p| {
                p.models
                    .iter()
                    .map(|m| {
                        (
                            m.id.clone(),
                            if m.label.is_empty() {
                                m.id.clone()
                            } else {
                                m.label.clone()
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        if !models.is_empty() {
            let mut pick_model: Option<String> = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("模型")
                        .size(p.sz(11.0))
                        .color(p.text_faint),
                );
                for (id, label) in &models {
                    if crate::widgets::chip(ui, p, label, *id == selected_model) && !state.is_busy()
                    {
                        pick_model = Some(id.clone());
                    }
                }
            });
            if let Some(id) = pick_model {
                acts.push(Action::SetAiModel(id));
            }
            ui.add_space(8.0);
        }

        // 密钥：直接在软件里填。
        //
        // 旧版这里的文案是「请设置环境变量 TYPESAFE_API_KEY」——普通用户做不到，
        // 而且改完环境变量还得重启 App 才读得到。现在写进配置文件就立刻生效
        // （`Provider::key()` 环境变量优先、其次 `api_key`），所以输入框就是正路，
        // 环境变量只在「高级」里作为脚本场景的一句说明保留。
        let providers = state.ai.config.providers.clone();
        let active = state.ai.config.active_provider.clone();
        let current = providers.iter().find(|x| x.id == active).cloned();
        let has_key = current.as_ref().map(|p| p.key().is_some()).unwrap_or(false);
        // jev 是本地判定引擎，不需要密钥；除此之外每一家都能填。
        let accepts_key = current
            .as_ref()
            .map(|p| {
                p.models
                    .iter()
                    .any(|m| m.id != molt_ai::config::JEV_MODEL_ID)
            })
            .unwrap_or(false);

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("密钥")
                    .size(p.sz(11.0))
                    .color(p.text_faint),
            );
            if has_key {
                badge(ui, p, "已设置", p.reclaimable, p.accent_soft);
            } else {
                badge(ui, p, "未设置", p.costly, p.costly_soft);
            }
        });

        if !accepts_key {
            // 本地引擎没有密钥可填，写一句原因，免得用户以为输入框被吞了。
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new("这个服务在本机运行，不需要密钥。")
                        .size(p.sz(10.5))
                        .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        } else {
            ui.add_space(6.0);
            let mut save = false;
            ui.horizontal(|ui| {
                let mut draft = state.key_draft.clone();
                let edit = egui::TextEdit::singleline(&mut draft)
                    .password(true)
                    .hint_text(if has_key {
                        "粘贴新密钥可换掉旧的"
                    } else {
                        "把密钥粘贴到这里"
                    })
                    .desired_width(p.sz(300.0))
                    .font(egui::FontId::proportional(p.sz(12.5)));
                let resp = ui.add(edit);
                if resp.changed() {
                    acts.push(Action::SetKeyDraft(draft.clone()));
                }
                // 回车即保存：粘贴完接着按回车是最自然的动作，
                // 让它失效就等于逼用户去找按钮。
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    save = true;
                }
                let can_save = !state.key_draft.trim().is_empty() && !state.is_busy();
                if secondary_button(ui, p, "保存", can_save) {
                    save = true;
                }
            });
            if save {
                acts.push(Action::SaveApiKey);
            }
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "存在这台 Mac 的配置文件里，只有你的账户能读；界面不会回显已保存的密钥。",
                    )
                    .size(p.sz(10.5))
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }

        // 配置文件位置——只写「在哪」，不写里面的内容。只在使用者主动展开
        // 「高级」时才出现：要填密钥的人不需要知道有个 toml。
        if state.settings_advanced {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("配置文件：{}", state.ai.config_path.display()))
                        .size(p.sz(10.0))
                        .monospace()
                        .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
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

/// 把输入框里的密钥写进当前服务的配置并落盘。
///
/// 三个必须一起做到的点：
/// 1. 写 `providers[i].api_key` 而不是改环境变量——前者立刻生效、不需要重启，
///    后者要用户会改 shell 配置，而这正是这一版要消灭的门槛；
/// 2. 落盘后把文件权限收到 `0600`，密钥不进世界可读的文件；
/// 3. 无论成没成都清空输入框——留着它就有被下一次截图拍到的风险。
pub fn save_api_key(state: &mut AppState) {
    let key = state.key_draft.trim().to_string();
    if key.is_empty() {
        return;
    }
    let active = state.ai.config.active_provider.clone();
    let Some(prov) = state
        .ai
        .config
        .providers
        .iter_mut()
        .find(|p| p.id == active)
    else {
        state.status = Some(StatusMessage::error("没有选中的服务，密钥没有保存"));
        return;
    };
    prov.api_key = key;
    let name = prov.display().to_string();

    let path = state.ai.config_path.clone();
    match state.ai.config.save(&path) {
        Ok(()) => {
            state.ai.config_error = None;
            state.key_draft.clear();
            // 权限收紧失败不拦保存：密钥已经写进去了，这时报错只会让用户
            // 以为保存失败、再存一次。真正要紧的是告诉他结果。
            let tightened = tighten_permissions(&path);
            let tail = if tightened {
                ""
            } else {
                "（文件权限收紧失败，建议检查一下该目录）"
            };
            state.status = Some(StatusMessage::info(format!(
                "{name} 的密钥已保存，下次解释就会用它{tail}"
            )));
        }
        Err(e) => {
            state.ai.config_error = Some(e.to_string());
            state.status = Some(StatusMessage::error(format!("密钥保存失败：{e}")));
        }
    }
}

/// 把配置文件权限收到 `0600`。返回是否成功。
///
/// Unix 上这是「只有我能读」的唯一实际保证；`AiConfig::save` 自己不管权限，
/// 因为它写的是普通配置，不该替调用方决定。
fn tighten_permissions(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).is_ok()
}

/// 服务名：有显示名用显示名，没有退回 id——但绝不展示 id 的原始值以外的技术细节。
fn provider_label(prov: &molt_ai::Provider) -> String {
    prov.display().to_string()
}

/// 把当前的模型选择落盘，并把它成没成说清楚。
///
/// 换服务与换模型是同一件事的两步，落盘与报错也就该只有一份实现——
/// 两处各写一遍最容易出现「一处记住了失败、另一处忘了」。
pub fn save_ai_config(state: &mut AppState) {
    match state.ai.config.save(&state.ai.config_path) {
        Ok(()) => {
            state.ai.config_error = None;
            let what = state
                .ai
                .resolve()
                .map(|r| r.summary())
                .unwrap_or_else(|_| "已经保存，但还缺密钥".to_string());
            state.status = Some(StatusMessage::info(format!("已切换到 {what}")));
        }
        Err(e) => {
            state.ai.config_error = Some(e.to_string());
            state.status = Some(StatusMessage::error(format!("保存失败：{e}")));
        }
    }
}

/// 高级：配置文件里的原始字段。
///
/// 默认折起来。普通用户不需要看到 base_url 与环境变量名，但排障时必须能拿到——
/// 之前这些字段直接被删掉了，用户遇到「为什么调不通」时无处可查。
/// 密钥只报「已设置 / 未设置」，从不显示原值：这个窗口会被截图、会被共享屏幕。
fn advanced_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let open = state.settings_advanced;
    ui.vertical(|ui| {
        let head = if open { "高级  ▾" } else { "高级  ▸" };
        if ui
            .add(
                egui::Label::new(egui::RichText::new(head).size(p.sz(12.5)).color(p.text_dim))
                    .sense(egui::Sense::click()),
            )
            .clicked()
        {
            acts.push(Action::ToggleSettingsAdvanced);
        }
        if !open {
            return;
        }
        ui.add_space(8.0);

        let active = state.ai.config.active_provider.clone();
        let prov = state
            .ai
            .config
            .providers
            .iter()
            .find(|x| x.id == active)
            .cloned();

        let Some(prov) = prov else {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("还没有选服务，所以没有可展示的配置。")
                        .size(p.sz(11.0))
                        .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            return;
        };

        ui.horizontal_wrapped(|ui| {
            kv(ui, p, "服务 id", &prov.id);
            kv(ui, p, "协议", prov.kind.label());
        });
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("base_url：{}", prov.base_url))
                    .size(p.sz(10.5))
                    .monospace()
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );

        // 密钥来源：只说是环境变量还是写在配置里，绝不给值。
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let source = if !prov.api_key.trim().is_empty() {
                "配置文件里直接写了密钥"
            } else if !prov.api_key_env.is_empty() {
                "从环境变量读取"
            } else {
                "没有配置密钥"
            };
            kv(ui, p, "密钥来源", source);
            if !prov.api_key_env.is_empty() {
                // 环境变量在这里只作为「脚本场景」的一句说明。
                // 主路径已经是在上面直接填密钥，不再要求用户去改 shell 配置。
                kv(
                    ui,
                    p,
                    "也可用环境变量",
                    &format!("{}（适合写在脚本里）", prov.api_key_env),
                );
            }
            kv(
                ui,
                p,
                "模型数",
                &format!(
                    "{} 个（当前 {}）",
                    prov.models.len(),
                    state.ai.config.active_model
                ),
            );
        });

        // jev 的兜底入口：删除判断在用户没指定模型时落到 jev，
        // 但界面上得有一条「我现在就想用它」的路，而不是靠运气撞上默认值。
        // 服务 id 不写死：jev 属于哪个服务由配置决定，换一份配置也不该指错。
        let has_jev_provider = state
            .ai
            .config
            .providers
            .iter()
            .find(|x| {
                x.models
                    .iter()
                    .any(|m| m.id == molt_ai::config::JEV_MODEL_ID)
            })
            .map(|_| ())
            .is_some();
        let on_jev = state.ai.config.active_model == molt_ai::config::JEV_MODEL_ID;
        if has_jev_provider && !on_jev {
            ui.add_space(8.0);
            if secondary_button(ui, p, "改用 jev（本地判定引擎）", !state.is_busy()) {
                acts.push(Action::UseJev);
            }
        }
    });
}

/// 规则库与应用数据的位置。
fn rules_card(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new("清理规则").size(p.sz(13.5)).strong());
        ui.add_space(4.0);
        match &state.rules {
            Some(r) => {
                let auto = r.iter().filter(|x| x.auto).count();
                ui.horizontal_wrapped(|ui| {
                    kv(ui, p, "已知的缓存类型", &format!("{} 种", r.len()));
                    kv(ui, p, "可以自动清理", &format!("{auto} 种"));
                });
                if state.settings_advanced {
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

        if state.settings_advanced {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "应用数据：{}（临时空间、审计库、定时任务结果都在这里）",
                        molt_act::paths::app_dir().display()
                    ))
                    .size(p.sz(10.0))
                    .monospace()
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
    });
}
