//! 集成测试共用的部件：界面点击夹具与临时 home。
//!
//! 放在这里而不是各测试文件各写一份，是因为「同一份界面被两份测试用不同方式
//! 驱动」会让两边的结论没法互相印证——一处改了取文字的方式，另一处还按旧的走，
//! 同一个按钮就会一边显示「点得动」一边显示「点不动」。

#![allow(dead_code)] // 每个测试二进制只用到其中一部分

use egui::epaint::Shape;
use molt_gui::action::Action;
use molt_gui::state::AppState;
use molt_gui::theme::{Palette, Theme};

/// 一帧绘制里所有文字图元的 (文本, 左上角位置)。
pub fn texts_of(out: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
    fn walk(shape: &Shape, out: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos)),
            Shape::Vec(v) => {
                for s in v {
                    walk(s, out);
                }
            }
            _ => {}
        }
    }
    let mut acc = Vec::new();
    for cs in &out.shapes {
        walk(&cs.shape, &mut acc);
    }
    acc
}

/// 把某一页真的画出来，并按文字位置发真实点击。
///
/// 这一层存在的理由：单测能证明「`Action::X` 落到 `Effect::Y`」，但它证不了
/// 「界面上那颗按钮真的会 push `Action::X`」。接缝正在这里——按钮画出来了但
/// `enabled` 是 false、点击被别的控件抢走、或者干脆忘了接线，单测全绿而用户
/// 点下去毫无反应。
pub struct Ui {
    pub ctx: egui::Context,
    pub p: Palette,
    pub base: egui::RawInput,
}

impl Ui {
    /// 默认尺寸给足高度，让整页一次性画完。
    pub fn new(width: f32, height: f32) -> Self {
        let ctx = egui::Context::default();
        molt_gui::theme::install_fonts(&ctx);
        Theme::install(&ctx);
        Self {
            ctx,
            p: Palette::light().with_scale(Theme::DEFAULT_SCALE),
            base: egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                ..Default::default()
            },
        }
    }

    /// 画两帧（第一帧排版，第二帧位置稳定），回传所有文字与位置。
    pub fn paint(
        &self,
        state: &AppState,
        show: fn(&mut egui::Ui, &AppState, &Palette, &mut Vec<Action>),
        acts: &mut Vec<Action>,
    ) -> Vec<(String, egui::Pos2)> {
        let mut texts = Vec::new();
        for _ in 0..2 {
            texts.clear();
            let mut out = self.ctx.run_ui(self.base.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, state, &self.p, acts);
                });
            });
            out.textures_delta.clear();
            texts = texts_of(&out);
        }
        texts
    }

    /// 在文字**完全等于** `label` 的控件上发一次完整点击。
    ///
    /// 找不到时直接失败并列出当前画出的全部文字：「按钮没画出来」和
    /// 「点了没反应」是两种缺陷，测试要能分别指认。
    pub fn click(
        &self,
        state: &AppState,
        show: fn(&mut egui::Ui, &AppState, &Palette, &mut Vec<Action>),
        label: &str,
    ) -> (Vec<Action>, Vec<String>) {
        self.click_where(state, show, |t| t.trim() == label, label)
    }

    /// 在文字**包含** `needle` 的第一个控件上点击。
    ///
    /// 用于带易变数字的文案（「能清 3 项 · 1.2 MB」）：把数字写进断言会在
    /// 夹具一改时全线失效。
    pub fn click_containing(
        &self,
        state: &AppState,
        show: fn(&mut egui::Ui, &AppState, &Palette, &mut Vec<Action>),
        needle: &str,
    ) -> (Vec<Action>, Vec<String>) {
        self.click_where(
            state,
            show,
            |t| t.trim().contains(needle),
            &format!("包含「{needle}」的控件"),
        )
    }

    fn click_where(
        &self,
        state: &AppState,
        show: fn(&mut egui::Ui, &AppState, &Palette, &mut Vec<Action>),
        hit: impl Fn(&str) -> bool,
        what: &str,
    ) -> (Vec<Action>, Vec<String>) {
        let mut acts: Vec<Action> = Vec::new();
        let texts = self.paint(state, show, &mut acts);
        let pos = texts
            .iter()
            .find(|(t, _)| hit(t))
            .unwrap_or_else(|| {
                let all: Vec<String> = texts.iter().map(|(t, _)| t.trim().to_string()).collect();
                panic!("界面上找不到{what}。当前画出的文字：{all:#?}")
            })
            .1;
        // 文字位置是左上角，往右下挪一点落到控件内部。
        let point = egui::Pos2::new(pos.x + self.p.sz(6.0), pos.y + self.p.sz(6.0));

        acts.clear();
        let mut down = self.base.clone();
        down.events = vec![
            egui::Event::PointerMoved(point),
            egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ];
        let mut up = self.base.clone();
        up.events = vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }];
        for input in [down, up] {
            let mut out = self.ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show(ui, state, &self.p, &mut acts);
                });
            });
            out.textures_delta.clear();
        }
        let labels = texts.iter().map(|(t, _)| t.trim().to_string()).collect();
        (acts, labels)
    }

    /// 把当前页画一遍并回传全部文字（不含点击）。
    pub fn labels(
        &self,
        state: &AppState,
        show: fn(&mut egui::Ui, &AppState, &Palette, &mut Vec<Action>),
    ) -> Vec<String> {
        let mut acts = Vec::new();
        self.paint(state, show, &mut acts)
            .iter()
            .map(|(t, _)| t.trim().to_string())
            .collect()
    }
}

/// 一个只属于本次测试的临时 home。
///
/// 破坏性流程（清理 / 还原 / 彻底删除 / 定时任务）一律在这种目录里跑，
/// 碰不到用户真实缓存——2026-10-21 那次误清之后立下的规矩。
pub fn tmp_home(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "molt-test-{}-{}-{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
