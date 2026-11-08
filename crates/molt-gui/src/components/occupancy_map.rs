//! 占用图（treemap）：把一组体积画成面积成正比的色块。
//!
//! 与 `widgets` 里的零件一样，这里只吃显式数据（[`Block`]），**不读 `AppState`**：
//! 颜色由调用方按判定算好传进来，点击命中只回传下标，点开详情还是别的事情
//! 由调用方决定。这样同一份绘制代码既能用在清理页，也能用在别处的体积概览。
//!
//! 标签只在块够大时才写：面积小到放不下字的块，写字只会互相压成一片糊，
//! 而它本来也读不出来——调用方应当把这类碎块并成「其他 N 项」再传进来。

use eframe::egui;

use crate::theme::{Palette, RADIUS_PILL};
use crate::treemap::{squarified, TreemapItem};

/// 一块要画的区域。
pub struct Block {
    /// 块内写的主文字，例如「molt 的构建产物」。
    pub label: String,
    /// 悬停提示的正文（体积、路径、判定理由），由调用方拼好。
    pub detail: String,
    /// 实际占用字节数，决定面积。
    pub bytes: u64,
    /// 底色，通常直接取判定色。
    pub color: egui::Color32,
}

/// 画占用图，返回被点中的块下标。
///
/// `height` 是整块画布的高度；返回 `None` 表示这一帧没有点击落在任何块上。
/// `bytes` 全为 0 时什么都不画（也没有面积可画）。
pub fn occupancy_map(
    ui: &mut egui::Ui,
    p: &Palette,
    blocks: &[Block],
    height: f32,
) -> Option<usize> {
    let width = ui.available_width();
    if width <= 0.0 || height <= 0.0 || blocks.iter().all(|b| b.bytes == 0) {
        return None;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    // 画布底：块之间留出的 1px 缝隙会露出这一层，于是分块边界自然出现，
    // 不必给每块描一圈线（几十个描边会把图压成格纸）。
    painter.rect_filled(rect, egui::CornerRadius::same(RADIUS_PILL), p.surface_alt);

    let items: Vec<TreemapItem> = blocks
        .iter()
        .map(|b| TreemapItem {
            label: b.label.clone(),
            bytes: b.bytes,
            color: b.color,
        })
        .collect();
    // 画布往里收 1px，最外圈的块才不会贴着卡片边。
    let area = rect.shrink(1.0);
    let placed = squarified(&items, area);

    let mut clicked = None;
    for r in &placed {
        let block = &blocks[r.item_index];
        // 每块错开一档明暗：三块都是「有代价」的橙色时，靠颜色本身分不开边界。
        let shade = 1.0 - 0.11 * (r.item_index % 3) as f32;
        let fill = fill_of(block.color, shade);
        let cell = r.rect.shrink(1.0);
        if cell.width() < 1.0 || cell.height() < 1.0 {
            continue;
        }
        let id = ui.scope_id().with(("occupancy-block", r.item_index));
        let resp = ui
            .interact(cell, id, egui::Sense::click())
            .on_hover_ui(|ui| tooltip(ui, p, block));
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let radius = egui::CornerRadius::same(4);
        painter.rect_filled(cell, radius, fill);
        if resp.hovered() {
            // 悬停用焦点描边而不是「变亮」：深色模式下提亮会把浅色判定色洗白，
            // 反而看不出是哪一块。
            painter.rect_stroke(
                cell,
                radius,
                egui::Stroke::new(2.0, p.focus),
                egui::StrokeKind::Inside,
            );
        }
        paint_labels(painter.clone(), p, block, cell, resp.hovered());
        if resp.clicked() {
            clicked = Some(r.item_index);
        }
    }
    clicked
}

/// 块内文字：名字必写（能放下就写），体积在够高时才写第二行。
fn paint_labels(
    painter: egui::Painter,
    p: &Palette,
    block: &Block,
    cell: egui::Rect,
    hovered: bool,
) {
    let pad = p.sz(6.0);
    let inner_w = cell.width() - pad * 2.0;
    if cell.height() < p.sz(20.0) {
        return;
    }
    // 悬停时提亮一档：色块本身不变，只有字变清楚。
    let ink = if hovered {
        ink_on(block.color)
    } else {
        ink_on(block.color).gamma_multiply(0.94)
    };
    // 放不下三个字就不写名字：挤成「本…」比不写更让人困惑，
    // 而颜色本身已经说明了判定，细节在悬停提示里。小色块不写字是树图的常态。
    let probe = egui::text::LayoutJob::simple_singleline(
        "国国国".to_owned(),
        egui::FontId::proportional(p.sz(11.5)),
        ink,
    );
    if inner_w < painter.layout_job(probe).size().x {
        return;
    }
    // 单行 + 溢出省略交给 epaint 排版：自己按字符试宽度会把「本地 AI 模型文件」
    // 这类带空格的名字在空格处误判，只挤出第一个字加省略号。
    let name = one_line(&painter, &block.label, p.sz(11.5), inner_w, ink);
    let clip = cell.shrink2(egui::vec2(pad - 2.0, 1.0));
    let painter = painter.with_clip_rect(clip);
    painter.galley(cell.min + egui::vec2(pad, p.sz(4.0)), name, ink);
    if cell.height() >= p.sz(38.0) {
        let size = molt_scan::human_bytes(block.bytes);
        let value_ink = ink.gamma_multiply(0.78);
        // 体积这一行只在整体放得下时才写：写一半的「1.0 G…」容易被读错数量级。
        let galley = one_line(&painter, &size, p.sz(10.5), inner_w, value_ink);
        if !galley.elided {
            painter.galley(cell.min + egui::vec2(pad, p.sz(20.0)), galley, value_ink);
        }
    }
}

fn tooltip(ui: &mut egui::Ui, p: &Palette, block: &Block) {
    ui.set_max_width(p.sz(320.0));
    ui.add(
        egui::Label::new(
            egui::RichText::new(&block.label)
                .size(p.sz(12.0))
                .strong()
                .color(p.text),
        )
        .wrap(),
    );
    if !block.detail.is_empty() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(&block.detail)
                    .size(p.sz(11.0))
                    .color(p.text_dim),
            )
            .wrap(),
        );
    }
}

/// 按明暗档位调色。用 gamma 乘法而不是直接叠白：深色模式下叠白会把色块
/// 变成灰白，判定色就丢了。
fn fill_of(color: egui::Color32, shade: f32) -> egui::Color32 {
    if shade >= 1.0 {
        color
    } else {
        color.gamma_multiply(shade)
    }
}

/// 色块上该写深色还是浅色字。
///
/// 阈值取 150/255：判定色里只有「可回收」的浅蓝和受保护的浅灰落在亮的一侧，
/// 深色模式那两档必须换成深字，否则字和底一样淡。
fn ink_on(bg: egui::Color32) -> egui::Color32 {
    let luma = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if luma > 150.0 {
        egui::Color32::from_rgb(20, 26, 34)
    } else {
        egui::Color32::WHITE
    }
}

/// 排成恰好一行，放不下就省略。
///
/// `break_anywhere` 必须打开：中文没有空格可断，关掉它时 epaint 会把整串
/// 当成一个不可断的词，宁可一个字都不写。`max_rows = 1` 让它在行尾加省略号。
fn one_line(
    painter: &egui::Painter,
    text: &str,
    size: f32,
    max_w: f32,
    color: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_w,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_flips_on_light_fills() {
        // 深色模式「可回收」是浅蓝 (101,163,255)，必须配深字。
        assert_eq!(ink_on(egui::Color32::from_rgb(101, 163, 255)).r(), 20);
        // 浅色模式是饱和蓝 (24,103,226)，配白字。
        assert_eq!(ink_on(egui::Color32::from_rgb(24, 103, 226)).r(), 255);
    }

    #[test]
    fn shading_keeps_full_colour_at_1() {
        let c = egui::Color32::from_rgb(24, 103, 226);
        assert_eq!(fill_of(c, 1.0), c);
        let darker = fill_of(c, 0.89);
        assert!(darker.b() < c.b(), "调暗应当让通道值下降");
    }
}
