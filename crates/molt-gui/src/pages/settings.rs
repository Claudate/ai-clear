//! 设置：访问范围、可选 AI、显示、规则与关于。
//!
//! 普通用户先看得到实际访问的目录；密钥永远只显示是否已设置，
//! 不回显原值。未发布的政策网址和许可文件不能伪装成已可用。

use eframe::egui;

use crate::action::Action;
use crate::state::{AppState, StatusMessage};
use crate::theme::{Palette, RADIUS_CONTROL};
use crate::widgets::{badge, kv, page_header, primary_button, secondary_button};

/// 重读规则库并回填。规则库只在启动时读一次，用户往 rules 目录里加了规则后，
/// 不重新读一次的话设置页上那些「已知缓存类型 N 种」永远不会变。
pub fn reload_rules(state: &mut AppState) {
    match molt_rules::load(&state.home, Some(&molt_act::paths::user_rules_dir())) {
        Ok(rules) => {
            state.rules = Some(rules);
            state.error = None;
            state.status = Some(StatusMessage::info("规则库已重新读取"));
        }
        Err(e) => {
            // 读失败时保留旧的那份，并说清是哪一步不行、该怎么办。
            state.error = Some(format!("规则库加载失败：{e}"));
            state.status = Some(StatusMessage::error(format!(
                "规则库加载失败：{e}。旧的规则仍在用；请检查用户规则目录里的文件是否为合法 TOML。"
            )));
        }
    }
}

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            page_header(
                ui,
                p,
                "设置",
                "管理访问范围、AI 解释和清理规则",
                |ui| {
                    if secondary_button(ui, p, "重新读取配置", !state.is_busy()) {
                        // 这个按钮的文案承诺的是「配置」，而这一页的配置有两份：
                        // ai.toml 与规则库。以前只重读前者，用户加完规则点它，
                        // 规则卡上的统计一动不动。
                        acts.push(Action::ReloadAiConfig);
                        acts.push(Action::ReloadRules);
                    }
                },
            );
            ui.add_space(p.sz(10.0));
            crate::widgets::hairline(ui, p);
            ui.add_space(p.sz(9.0));

            access_card(ui, state, p, acts);
            ui.add_space(p.sz(20.0));
            model_card(ui, state, p, acts);
            section_body(ui, p, |ui| advanced_card(ui, state, p, acts));
            ui.add_space(p.sz(14.0));
            crate::widgets::hairline(ui, p);
            ui.add_space(p.sz(7.0));

            display_card(ui, p, acts);
            ui.add_space(p.sz(14.0));
            crate::widgets::hairline(ui, p);
            ui.add_space(p.sz(7.0));

            rules_card(ui, state, p);
            ui.add_space(p.sz(14.0));
            crate::widgets::hairline(ui, p);
            ui.add_space(p.sz(7.0));
            about_card(ui, p);
        });
}

/// 分组的图标与文案共用同一个基线；正文与标题文字左缘对齐。
fn section_heading(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, detail: &str) {
    ui.horizontal_top(|ui| {
        ui.add_sized(
            [p.sz(24.0), p.sz(27.0)],
            egui::Label::new(
                egui::RichText::new(icon)
                    .font(crate::icons::font(p.sz(18.0)))
                    .color(p.text),
            ),
        );
        ui.add_space(p.sz(5.0));
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(p.sz(13.5))
                    .strong()
                    .color(p.text),
            );
            if !detail.is_empty() {
                ui.label(
                    egui::RichText::new(detail)
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                );
            }
        });
    });
}

fn section_body(ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.add_space(p.sz(35.0));
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    });
}

/// 左侧固定一列字段标签；内容区窄时垂直排列，避免输入框被挤出窗口。
fn setting_row(ui: &mut egui::Ui, p: &Palette, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() < p.sz(455.0) {
        ui.label(egui::RichText::new(label).size(p.sz(11.5)).color(p.text));
        add(ui);
    } else {
        ui.horizontal_top(|ui| {
            ui.add_sized(
                [p.sz(126.0), p.sz(29.0)],
                egui::Label::new(egui::RichText::new(label).size(p.sz(11.5)).color(p.text)),
            );
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                add(ui);
            });
        });
    }
    ui.add_space(p.sz(5.0));
}

/// 扫描起点是真实家目录，不允许用户误以为这只图标可以改动扫描范围。
fn access_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    section_heading(
        ui,
        p,
        crate::icons::FOLDER,
        "访问范围",
        "从家目录查找缓存，当前路径：",
    );
    ui.add_space(p.sz(7.0));
    section_body(ui, p, |ui| {
        let width = ui.available_width();
        egui::Frame::NONE
            .fill(p.surface)
            .stroke(egui::Stroke::new(1.0, p.border_strong))
            .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
            .inner_margin(egui::Margin::symmetric(9, 3))
            .show(ui, |ui| {
                ui.set_width(width - 18.0);
                ui.horizontal(|ui| {
                    let button_width = p.sz(29.0);
                    let text_width = (ui.available_width() - button_width - p.sz(9.0)).max(40.0);
                    let (text_rect, text_response) = ui.allocate_exact_size(
                        egui::vec2(text_width, p.sz(26.0)),
                        egui::Sense::hover(),
                    );
                    ui.painter().with_clip_rect(text_rect).text(
                        text_rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        &state.home,
                        egui::FontId::proportional(p.sz(11.5)),
                        p.text,
                    );
                    text_response.on_hover_text(&state.home);
                    let icon = egui::RichText::new(crate::icons::FOLDER_OPEN)
                        .font(crate::icons::font(p.sz(16.0)))
                        .color(p.text);
                    if ui
                        .add_sized(
                            [button_width, p.sz(26.0)],
                            egui::Button::new(icon).frame(false),
                        )
                        .on_hover_text("在访达中显示当前路径")
                        .clicked()
                    {
                        acts.push(Action::RevealInFinder(state.home.clone()));
                    }
                });
            });
    });
}

fn display_card(ui: &mut egui::Ui, p: &Palette, acts: &mut Vec<Action>) {
    section_heading(
        ui,
        p,
        crate::icons::HARD_DRIVES,
        "显示",
        "跟随 macOS 浅色与深色，默认大字号。",
    );
    ui.add_space(p.sz(8.0));
    section_body(ui, p, |ui| {
        setting_row(ui, p, "字体大小", |ui| {
            let selected = if p.scale > 1.3 {
                0
            } else if p.scale > 1.1 {
                1
            } else {
                2
            };
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0, p.border_strong))
                .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
                .inner_margin(egui::Margin::same(2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for (i, name) in ["大", "中", "小"].into_iter().enumerate() {
                            let active = selected == i;
                            let text = egui::RichText::new(name)
                                .size(p.sz(11.5))
                                .color(if active { p.on_accent() } else { p.text });
                            let button = egui::Button::new(text)
                                .fill(if active { p.reclaimable } else { p.surface })
                                .corner_radius(egui::CornerRadius::same(RADIUS_CONTROL))
                                .min_size(egui::vec2(p.sz(64.0), p.sz(25.0)));
                            if ui.add(button).clicked() && !active {
                                acts.push(Action::SetFontScale(i as u8));
                            }
                        }
                    });
                });
        });
    });
}

/// 模型功能：当前用哪个模型 + 选择服务与模型。
///
/// 主视线只放用户在意的三件事：现在用的是谁、密钥备好了没有、想换哪家 / 哪个模型。
/// 配置文件里的原始字段（id、base_url、请求路径、密钥环境变量名）收进「高级」——
/// 那是排障时才要的东西，放在主视觉里只会把可做的操作挤走。
fn model_card(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    section_heading(
        ui,
        p,
        crate::icons::SPARKLE,
        "AI 解释（可选）",
        "AI 只解释条目，判定和清理由本地规则完成。",
    );
    ui.add_space(p.sz(9.0));
    section_body(ui, p, |ui| {
        setting_row(ui, p, "当前使用状态", |ui| match state.ai.resolve() {
            Ok(r) => {
                badge(ui, p, "已启用", p.reclaimable, p.accent_soft);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(r.summary())
                            .size(p.sz(10.0))
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            Err(reason) => {
                badge(ui, p, "尚未配置", p.costly, p.costly_soft);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(reason)
                            .size(p.sz(10.0))
                            .color(p.text_dim),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });

        // 服务与模型来自当前配置，不拿设计图里的品牌名充数。
        let providers = &state.ai.config.providers;
        let active = &state.ai.config.active_provider;
        setting_row(ui, p, "服务选择", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = p.sz(12.0);
                for prov in providers {
                    let selected = &prov.id == active;
                    if ui
                        .add_enabled(
                            !state.is_busy(),
                            egui::RadioButton::new(
                                selected,
                                egui::RichText::new(provider_label(prov)).size(p.sz(11.5)),
                            ),
                        )
                        .clicked()
                    {
                        acts.push(Action::SetAiProvider(prov.id.clone()));
                    }
                }
            });
        });

        let models = providers
            .iter()
            .find(|provider| &provider.id == active)
            .map(|provider| &provider.models);
        if let Some(models) = models.filter(|models| !models.is_empty()) {
            let selected_model = &state.ai.config.active_model;
            let current_label = models
                .iter()
                .find(|model| &model.id == selected_model)
                .map(|model| {
                    if model.label.is_empty() {
                        &model.id
                    } else {
                        &model.label
                    }
                })
                .map(String::as_str)
                .unwrap_or("请选择模型");
            setting_row(ui, p, "模型选择", |ui| {
                ui.add_enabled_ui(!state.is_busy(), |ui| {
                    egui::ComboBox::from_id_salt("settings-ai-model")
                        .selected_text(current_label)
                        .width(ui.available_width().max(p.sz(90.0)))
                        .show_ui(ui, |ui| {
                            for model in models {
                                let label = if model.label.is_empty() {
                                    &model.id
                                } else {
                                    &model.label
                                };
                                if ui
                                    .selectable_label(&model.id == selected_model, label)
                                    .clicked()
                                {
                                    acts.push(Action::SetAiModel(model.id.clone()));
                                }
                            }
                        });
                });
            });
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

        setting_row(ui, p, "密钥状态", |ui| {
            if !accepts_key {
                badge(ui, p, "无需密钥", p.text_dim, p.surface_alt);
            } else if has_key {
                badge(ui, p, "已设置", p.reclaimable, p.accent_soft);
            } else {
                badge(ui, p, "未设置", p.costly, p.costly_soft);
            }
        });

        if !accepts_key {
            setting_row(ui, p, "密钥输入", |ui| {
                ui.label(
                    egui::RichText::new("这个服务在本机运行，不需要密钥。")
                        .size(p.sz(10.5))
                        .color(p.text_dim),
                );
            });
        } else {
            setting_row(ui, p, "密钥输入", |ui| {
                let mut draft = state.key_draft.clone();
                let mut save = false;
                ui.horizontal(|ui| {
                    let input_width = (ui.available_width() - p.sz(69.0)).max(p.sz(105.0));
                    let edit = egui::TextEdit::singleline(&mut draft)
                        .password(true)
                        .hint_text(if has_key {
                            "粘贴新密钥可换掉旧的"
                        } else {
                            "请输入 API 密钥"
                        })
                        .desired_width(input_width)
                        .font(egui::FontId::proportional(p.sz(11.5)));
                    let response = ui
                        .add_sized([input_width, p.sz(30.0)], edit)
                        .accessible_name("API 密钥");
                    if response.changed() {
                        acts.push(Action::SetKeyDraft(draft.clone()));
                    }
                    let can_save = !draft.trim().is_empty() && !state.is_busy();
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        && can_save
                    {
                        save = true;
                    }
                    if primary_button(ui, p, "保存", can_save) {
                        save = true;
                    }
                });
                if save {
                    acts.push(Action::SaveApiKey);
                }
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "密钥只存在这台 Mac 的配置文件里，界面不会回显已保存的密钥。",
                    )
                    .size(p.sz(10.0))
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

/// 关于入口只指向已可访问的目标；现有隐私网页内容待更新，页内另行说明真实行为。
fn about_card(ui: &mut egui::Ui, p: &Palette) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new("关于").size(p.sz(13.5)).strong());
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("净灵 · Cleanling")
                .size(p.sz(11.5))
                .color(p.text_dim),
        );
        ui.add_space(7.0);

        ui.collapsing("隐私政策（网页内容待更新）", |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("隐私政策网页已发布，但现有版本仍使用旧名称，且对发送路径的描述与当前实现不符。网页内容待更新；以下说明当前软件实际如何处理数据，不把旧网页当作准确的正式版本。")
                        .size(p.sz(10.5))
                        .color(p.costly),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new("扫描、判定与清理在本机进行。只有你主动使用 AI 分析时，才会把包含家目录路径的扫描摘要发给你选定的模型服务；如果另外开启内容采样，还可能发送部分文件内容。")
                        .size(p.sz(10.5))
                        .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
        ui.add_space(5.0);
        ui.hyperlink_to("帮助与支持 ↗", "https://aiclear.openclawbot.cn/support/");
        ui.add_space(5.0);
        ui.hyperlink_to("查看 MIT 许可条款 ↗", "https://spdx.org/licenses/MIT.html");
        ui.add(
            egui::Label::new(
                egui::RichText::new("第三方组件许可清单尚未随 App 提供。")
                    .size(p.sz(10.0))
                    .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
    });
}

/// 规则库与应用数据的位置。
fn rules_card(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    let summary = match &state.rules {
        Some(rules) => format!(
            "已知缓存类型 {} 种 · 可以自动清理 {} 种",
            rules.len(),
            rules.iter().filter(|rule| rule.auto).count()
        ),
        None => "规则库没有加载成功，扫描会因此失败。".to_string(),
    };
    section_heading(ui, p, crate::icons::FILE, "规则", &summary);
    if state.settings_advanced {
        section_body(ui, p, |ui| {
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实点击设置页的支持链接必须交出可打开的系统 URL 命令。
    #[test]
    fn support_link_emits_open_url_command() {
        fn locate(shape: &egui::epaint::Shape) -> Option<egui::Pos2> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text().contains("帮助与支持") => {
                    Some(text.pos + egui::vec2(12.0, 8.0))
                }
                egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(locate),
                _ => None,
            }
        }
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let state = AppState::new("/Users/me".into());
        let palette = Palette::light().with_scale(crate::theme::Theme::DEFAULT_SCALE);
        let base = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1120.0, 1600.0),
            )),
            ..Default::default()
        };
        let render = |input| {
            let mut output = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, &state, &palette, &mut Vec::new());
                });
            });
            output.textures_delta.clear();
            output
        };
        let output = render(base.clone());
        let pos = output
            .shapes
            .iter()
            .find_map(|clipped| locate(&clipped.shape))
            .expect("关于页必须画出支持链接");
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
        render(down);
        let mut up = base;
        up.events = vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }];
        let output = render(up);
        assert!(output.platform_output.commands.iter().any(|command| matches!(
            command,
            egui::OutputCommand::OpenUrl(url)
                if url.url == "https://aiclear.openclawbot.cn/support/"
        )));
    }
}
