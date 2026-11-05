//! 「工具」面板：顶栏扳手点开的那一块。
//!
//! 它铺的是**当前真能用的工具**，不是一条「新建」的快捷方式：
//! 每个工具一个图标、一句说明，按用途分成几类。工具少的时候就少画几行，
//! 一类里一个都没有就画一块占位——版式不因为「暂时只有一个工具」而塌成一行
//! 菜单项，将来往注册表里加一条就自动出现。
//!
//! 组件契约：只吃 [`ToolsMenuProps`]，只吐 [`ToolId`]。它不认识 `AppState`，
//! 也不知道某个工具背后是弹窗还是线程——「现在能不能按、为什么不能」由调用方
//! 算好塞进来。

use eframe::egui;

use crate::icons;
use crate::theme::Palette;

/// 面板里能按下的工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolId {
    /// 新建文件夹 / 文件（浮窗）
    Create,
    /// 在访达里打开临时空间
    RevealQuarantine,
}

/// 一个工具条目。
pub struct ToolSlot {
    pub id: ToolId,
    pub name: &'static str,
    pub hint: &'static str,
    /// 图标字形（Phosphor 的一个码位，见 [`crate::icons`]）。
    pub glyph: &'static str,
}

/// 一类工具。
pub struct ToolGroup {
    pub title: &'static str,
    pub items: &'static [ToolSlot],
}

/// 工具注册表。加工具就是往这里加一条——面板按它铺，不用改画法。
pub const GROUPS: &[ToolGroup] = &[
    ToolGroup {
        title: "创建",
        items: &[ToolSlot {
            id: ToolId::Create,
            name: "新建文件夹 / 文件",
            hint: "一次写好一批路径，逐条预检后建出来",
            glyph: icons::FOLDER_PLUS,
        }],
    },
    ToolGroup {
        title: "临时空间",
        items: &[ToolSlot {
            id: ToolId::RevealQuarantine,
            name: "打开临时空间",
            hint: "移走的东西都在这里，7 天内可放回去",
            glyph: icons::ARCHIVE_TRAY,
        }],
    },
];

/// 一个工具当下的可用性。`reason` 是「为什么不能按」，画在条目的说明位置。
#[derive(Debug, Clone)]
pub struct ToolState {
    pub enabled: bool,
    pub reason: Option<String>,
}

pub struct ToolsMenuProps<'a> {
    /// 每类工具的分组。传 `GROUPS` 就是全部；测试可以传一份带空分类的。
    pub groups: &'a [ToolGroup],
    /// 查一个工具现在能不能按。不可用时给出理由。
    pub state_of: &'a dyn Fn(ToolId) -> ToolState,
    /// 铺开的方式。同一份注册表、同一套卡片画法，只换排版。
    pub layout: ToolsLayout,
}

/// 工具怎么铺开。
///
/// 同一个组件要服务两种场合：浮窗里地方窄，按条目一行一条往下排；
/// 独立页面宽度大，三栏并排、每件一张卡片。画法（图标底、文字层级、
/// 不可用时的压暗与理由）是同一套——分两种布局而不是分两份组件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsLayout {
    /// 浮窗：横向铺满、每条一行。
    List,
    /// 页面：分类并排成栏，每件一张卡片；窗口窄了自动退回 [`Self::List`] 的纵向。
    Grid,
}

pub struct ToolsMenu;

impl ToolsMenu {
    /// 画一帧。返回这一帧里被按下的那个工具（至多一个）。
    pub fn show(ui: &mut egui::Ui, p: &Palette, props: ToolsMenuProps<'_>) -> Option<ToolId> {
        match props.layout {
            ToolsLayout::List => list(ui, p, &props),
            ToolsLayout::Grid => grid(ui, p, &props),
        }
    }
}

/// 浮窗形态：一行一条。
fn list(ui: &mut egui::Ui, p: &Palette, props: &ToolsMenuProps<'_>) -> Option<ToolId> {
    let mut picked = None;
    let w = ui.available_width();

    for (i, group) in props.groups.iter().enumerate() {
        if i > 0 {
            ui.add_space(p.sz(10.0));
        }
        group_title(ui, p, group.title);
        ui.add_space(p.sz(4.0));

        if group.items.is_empty() {
            empty_slot(ui, p, w);
            continue;
        }
        for slot in group.items {
            let st = (props.state_of)(slot.id);
            if row(ui, p, slot, &st, w) {
                picked = Some(slot.id);
            }
        }
    }
    picked
}

/// 页面形态：分类并排成栏，每件一张卡片。
fn grid(ui: &mut egui::Ui, p: &Palette, props: &ToolsMenuProps<'_>) -> Option<ToolId> {
    let mut picked = None;
    // 分类横着并排，而不是一条一条往下叠：每类眼下只有一件工具时，
    // 纵向叠起来就是三行卡片 + 右边一整片空白，一屏能读到的反而更少。
    // 判据是「一栏至少还得放得下一张卡」，窗口窄了自动退回纵向。
    let gap = p.sz(16.0);
    let min_col = p.sz(300.0);
    let fit = ((ui.available_width() + gap) / (min_col + gap))
        .floor()
        .max(1.0) as usize;
    let cols = props.groups.len().min(fit).max(1);
    for band in props.groups.chunks(cols) {
        if cols == 1 {
            for group in band {
                if let Some(id) = column(ui, p, group, props.state_of) {
                    picked = Some(id);
                }
                ui.add_space(p.sz(12.0));
            }
            continue;
        }
        let widths = split_widths(ui.available_width(), band.len(), gap);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (group, w) in band.iter().zip(widths) {
                let r = ui.allocate_ui_with_layout(
                    egui::vec2(w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(w);
                        column(ui, p, group, props.state_of)
                    },
                );
                if let Some(id) = r.inner {
                    picked = Some(id);
                }
            }
        });
        ui.add_space(p.sz(12.0));
    }
    picked
}

/// 一栏：分类标题 + 这一类里的卡片。
fn column(
    ui: &mut egui::Ui,
    p: &Palette,
    group: &ToolGroup,
    state_of: &dyn Fn(ToolId) -> ToolState,
) -> Option<ToolId> {
    group_title(ui, p, group.title);
    ui.add_space(p.sz(6.0));

    let w = ui.available_width();
    if group.items.is_empty() {
        empty_slot(ui, p, w);
        return None;
    }
    let mut picked = None;
    for slot in group.items {
        let st = state_of(slot.id);
        // 卡片撑满这一栏：栏宽已经由外面分配好，再在里面留白就成了「卡片套卡片」。
        if tile(ui, p, slot, &st, w) {
            picked = Some(slot.id);
        }
        ui.add_space(p.sz(8.0));
    }
    picked
}

fn group_title(ui: &mut egui::Ui, p: &Palette, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(p.sz(12.5))
            .strong()
            .color(p.text_dim),
    );
}

/// 把 `total` 点宽度分成 `n` 栏，栏间留 `gap`，余数补给最后一栏。
///
/// 直接 `total / n` 会因浮点误差在右边漏掉一两个点，肉眼是一条歪的细缝。
fn split_widths(total: f32, n: usize, gap: f32) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    let each = (total - gap * (n as f32 - 1.0)) / n as f32;
    let mut out = vec![each; n];
    if let Some(last) = out.last_mut() {
        *last = total - gap * (n as f32 - 1.0) - each * (n as f32 - 1.0);
    }
    out
}

/// 一类里还没有工具时的那块占位。
///
/// 为什么画出来而不是整类不显示：面板的版式得稳定——今天只有一个工具、
/// 明天加到三个，分类标题和位置不该每天换一次；空着的那一格也让用户看出
/// 「这一类还没做」，而不是以为面板少画了什么。
fn empty_slot(ui: &mut egui::Ui, p: &Palette, w: f32) {
    let h = p.sz(34.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
        p.surface_alt,
    );
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
        egui::Stroke::new(1.0, p.border),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "这一类还没有工具",
        egui::FontId::proportional(p.sz(11.0)),
        p.text_faint,
    );
}

/// 一行「图标 + 名字 + 说明」。整行可点，不可用时压暗并写出理由。
fn row(ui: &mut egui::Ui, p: &Palette, slot: &ToolSlot, st: &ToolState, w: f32) -> bool {
    let h = p.sz(38.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    if st.enabled && resp.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
            p.surface_hover,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // 图标坐在一块圆角软底上：一排文字行里，只有它能让「这是个动作」立住。
    let icon_side = p.sz(24.0);
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + p.sz(6.0), rect.center().y - icon_side / 2.0),
        egui::vec2(icon_side, icon_side),
    );
    ui.painter().rect_filled(
        icon_rect,
        egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
        if st.enabled {
            p.accent_soft
        } else {
            p.surface_alt
        },
    );
    icons::draw(
        ui.painter(),
        slot.glyph,
        icon_rect,
        if st.enabled {
            p.reclaimable
        } else {
            p.text_faint
        },
    );

    let text_x = icon_rect.right() + p.sz(10.0);
    let name_color = if st.enabled { p.text } else { p.text_dim };
    ui.painter().text(
        egui::pos2(text_x, rect.center().y - p.sz(8.0)),
        egui::Align2::LEFT_CENTER,
        slot.name,
        egui::FontId::proportional(p.sz(12.5)),
        name_color,
    );
    // 说明位置放不下的长句直接截断：这是辅助信息，换行会把每行撑成两倍高，
    // 面板就变成一张清单页。
    let (sub, sub_color) = match (&st.reason, st.enabled) {
        (Some(r), _) => (r.clone(), p.costly),
        (None, true) => (slot.hint.to_string(), p.text_faint),
        (None, false) => (slot.hint.to_string(), p.text_faint),
    };
    let avail = rect.right() - p.sz(6.0) - text_x;
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = avail;
    job.wrap.max_rows = 1;
    job.append(
        &sub,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(p.sz(10.5)),
            color: sub_color,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(text_x, rect.center().y + p.sz(1.0)),
        galley,
        sub_color,
    );

    resp.clicked() && st.enabled
}

/// 一张工具卡片（页面形态）：大图标在上，名字与说明竖排在下面。整张可点。
///
/// 图标必须是这一页的主角：之前它和一行清单里的图标一样大（38 点），
/// 卡片再宽也只是一条退回行的清单，放在页面正中显得像没做完。现在软底 112 点、
/// 图形占 69 点，一屏就能认出是哪件工具，名字只是它的注脚。
fn tile(ui: &mut egui::Ui, p: &Palette, slot: &ToolSlot, st: &ToolState, w: f32) -> bool {
    let h = p.sz(212.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let hovered = st.enabled && resp.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(crate::theme::RADIUS_CARD),
        if hovered { p.surface_hover } else { p.surface },
    );
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(crate::theme::RADIUS_CARD),
        egui::Stroke::new(1.0, if hovered { p.border_strong } else { p.border }),
        egui::StrokeKind::Inside,
    );

    // 图标软底：水平居中、垂直略靠上。底本身是一块完整的圆角方块，
    // 图形画在它中央略小的区域——图形贴着软底边沿会被看成「图标没画满」。
    let block = p.sz(112.0);
    let block_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + p.sz(26.0) + block / 2.0),
        egui::vec2(block, block),
    );
    ui.painter().rect_filled(
        block_rect,
        egui::CornerRadius::same((block * 0.28) as u8),
        if st.enabled {
            p.accent_soft
        } else {
            p.surface_alt
        },
    );
    icons::draw(
        ui.painter(),
        slot.glyph,
        egui::Rect::from_center_size(block_rect.center(), egui::vec2(block * 0.62, block * 0.62)),
        if st.enabled {
            p.reclaimable
        } else {
            p.text_faint
        },
    );

    // 名字与说明都居中、跟着图标往下排：竖排的单件卡片，左右对齐的
    // 「图左文右」会把整张卡拉成横向长条，反而让图标又变小。
    let name_color = if st.enabled { p.text } else { p.text_dim };
    let name = ui.painter().layout_no_wrap(
        slot.name.to_owned(),
        egui::FontId::proportional(p.sz(15.5)),
        name_color,
    );
    let name_y = block_rect.bottom() + p.sz(20.0);
    let name_size = name.size();
    ui.painter().galley(
        egui::pos2(rect.center().x - name_size.x / 2.0, name_y),
        name,
        name_color,
    );

    // 说明最多两行：不能按时写理由，那是用户最需要看到的一句。
    let (sub, sub_color) = match (&st.reason, st.enabled) {
        (Some(r), _) => (r.as_str(), p.costly),
        _ => (slot.hint, p.text_faint),
    };
    let mut job = egui::text::LayoutJob {
        halign: egui::Align::Center,
        ..Default::default()
    };
    job.wrap.max_width = rect.width() - p.sz(28.0);
    job.wrap.max_rows = 2;
    job.append(
        sub,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(p.sz(11.0)),
            color: sub_color,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            name_y + name_size.y + p.sz(4.0),
        ),
        galley,
        sub_color,
    );

    resp.clicked() && st.enabled
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 画一帧面板，把画出来的文字收集回来。
    fn paint(groups: &[ToolGroup]) -> Vec<String> {
        paint_with(groups, ToolsLayout::List)
    }

    /// 画一帧指定布局的面板。两种布局都得把同一份注册表完整画出来——
    /// 它们共用画法，只换排版，漏一条就等于那一类工具在页面上消失。
    fn paint_with(groups: &[ToolGroup], layout: ToolsLayout) -> Vec<String> {
        let st = |_: ToolId| ToolState {
            enabled: true,
            reason: None,
        };
        paint_with_state(groups, layout, &st)
    }

    fn paint_with_state(
        groups: &[ToolGroup],
        layout: ToolsLayout,
        st: &dyn Fn(ToolId) -> ToolState,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let p = crate::theme::Palette::light();
        let mut out = Vec::new();
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(420.0, 420.0),
                )),
                ..Default::default()
            };
            let mut texts = Vec::new();
            let mut r = ctx.run_ui(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ToolsMenu::show(
                        ui,
                        &p,
                        ToolsMenuProps {
                            groups,
                            state_of: st,
                            layout,
                        },
                    );
                });
            });
            // 字形纹理没有真的渲染器接手，丢掉前先清空，否则 epaint 会 panic
            r.textures_delta.clear();
            for cs in &r.shapes {
                walk(&cs.shape, &mut texts);
            }
            out = texts;
        }
        out
    }

    fn walk(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    walk(s, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn every_registered_tool_is_drawn_with_its_group() {
        // 「有几个显示几个」：注册表里有几条，面板上就得出几条，
        // 分类标题也要在——只画工具不画分类，等于又退回一条清单。
        // 两种布局各跑一遍：页面用的是 Grid，只验 List 等于没验那一页。
        for layout in [ToolsLayout::List, ToolsLayout::Grid] {
            let texts = paint_with(GROUPS, layout);
            for g in GROUPS {
                assert!(
                    texts.iter().any(|t| t == g.title),
                    "{layout:?}：分类「{}」要画出来，实际 {texts:?}",
                    g.title
                );
                for slot in g.items {
                    assert!(
                        texts.iter().any(|t| t.contains(slot.name)),
                        "{layout:?}：工具「{}」要画出来，实际 {texts:?}",
                        slot.name
                    );
                }
            }
        }
    }

    #[test]
    fn an_unavailable_tool_writes_the_reason_in_both_layouts() {
        // 不能按时必须写出理由，否则用户点了没反应会以为界面坏了。
        // 两种布局都得写——理由的位置不同，但都不能省。
        let st = |_: ToolId| ToolState {
            enabled: false,
            reason: Some("正在扫描…".to_string()),
        };
        for layout in [ToolsLayout::List, ToolsLayout::Grid] {
            let texts = paint_with_state(GROUPS, layout, &st);
            assert!(
                texts.iter().any(|t| t.contains("正在扫描…")),
                "{layout:?}：不可用的条目要写出理由，实际 {texts:?}"
            );
        }
    }

    #[test]
    fn an_empty_group_keeps_its_slot_instead_of_disappearing() {
        // 空分类要占位：版式稳定，也让用户看出「这一类还没做」。
        static NONE: &[ToolSlot] = &[];
        let groups = [
            ToolGroup {
                title: "创建",
                items: NONE,
            },
            ToolGroup {
                title: "临时空间",
                items: GROUPS[1].items,
            },
        ];
        let texts = paint(&groups);
        assert!(
            texts.iter().any(|t| t.contains("这一类还没有工具")),
            "空分类要画出占位，实际 {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("打开临时空间")),
            "另一类里的工具照常画，实际 {texts:?}"
        );
    }
}
