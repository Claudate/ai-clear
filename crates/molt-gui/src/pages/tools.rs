//! 工具页：随手用的功能自成一页，而不是从顶栏弹一个浮窗。
//!
//! 为什么是页面：浮窗只在鼠标按着的时候存在，切走就没了，用户还得记住
//! 「那个东西藏在一颗扳手里」；而工具本身就是可以停留、可以慢慢看的地方。
//! 顶栏那颗扳手现在和其它图标一样**切页**，形状与位置不变。
//!
//! 画法不在这里：卡片、大图标软底、不可用时的压暗与理由，全部由
//! [`crate::components::tools_menu`] 按同一份注册表画出来——这一页只负责
//! 「现在能不能按、按下去算什么动作」。同一份注册表换一个 `layout`
//! 就是浮窗形态，两边不会各自长出一套画法。

use eframe::egui;

use crate::action::Action;
use crate::components::tools_menu::{
    ToolId, ToolState, ToolsLayout, ToolsMenu, ToolsMenuProps, GROUPS,
};
use crate::state::AppState;
use crate::theme::Palette;
use crate::widgets::page_header;

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    page_header(
        ui,
        p,
        "工具",
        "偶尔要用一次的几件事。它们不属于清理主线，所以收在这里，不占顶栏的位置。",
        |_ui| {},
    );
    ui.add_space(12.0);

    let busy = state.is_busy();
    let blocking = state.activity().blocking_hint().to_string();
    let state_of = move |id: ToolId| -> ToolState {
        match id {
            // 新建要写盘，别的任务在跑时不能并开——理由写在条目上，
            // 不能让用户点了没反应。
            ToolId::Create => ToolState {
                enabled: !busy,
                reason: busy.then(|| blocking.clone()),
            },
            // 打开一个目录不动任何数据，任何时候都能按。
            ToolId::RevealQuarantine => ToolState {
                enabled: true,
                reason: None,
            },
        }
    };

    let picked = egui::ScrollArea::vertical()
        .id_salt("tools_page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ToolsMenu::show(
                ui,
                p,
                ToolsMenuProps {
                    groups: GROUPS,
                    state_of: &state_of,
                    layout: ToolsLayout::Grid,
                },
            )
        })
        .inner;

    if let Some(id) = picked {
        acts.push(action_for(id));
    }
}

fn action_for(id: ToolId) -> Action {
    match id {
        ToolId::Create => Action::OpenCreate,
        ToolId::RevealQuarantine => Action::RevealQuarantine,
    }
}
