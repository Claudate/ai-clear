//! 矩形树图（treemap）布局：把一组体积映射成一个矩形区域内互不重叠的块。
//!
//! 面积与体积成正比，方块按「squarified」算法排列（Bruls、Huisken、van Wijk，
//! 2000）——它让每个块尽量接近正方形，视觉比简单切片更容易扫读。
//! 本模块只做几何与测试，不碰 UI：颜色、悬停详情由调用方决定。
use eframe::egui;

/// 一块待布局的数据。`bytes` 为 0 的项会被忽略。
pub struct TreemapItem {
    pub label: String,
    pub bytes: u64,
    pub color: egui::Color32,
}

/// 布局结果：原始输入的索引 + 该索引对应的矩形。
#[derive(Debug)]
pub struct TreemapRect {
    pub item_index: usize,
    pub rect: egui::Rect,
}

/// 把 `items` 铺进 `area`。返回的矩形互不重叠且都在 `area` 内。
///
/// 布局按 `bytes` 降序处理；`bytes == 0` 的项不占面积也不返回。
/// 任何单独一项都不可能占满整块（< 1% 时并入「其他」由调用方展示）。
pub fn squarified(items: &[TreemapItem], area: egui::Rect) -> Vec<TreemapRect> {
    let total_size = area.size();
    if total_size.x <= 0.0 || total_size.y <= 0.0 {
        return Vec::new();
    }
    let total: u64 = items.iter().map(|i| i.bytes).sum();
    if total == 0 {
        return Vec::new();
    }

    // 归一化面积，降序，跳过 0
    let mut scaled: Vec<(usize, f64)> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.bytes > 0)
        .map(|(i, it)| {
            (
                i,
                it.bytes as f64 / total as f64 * (total_size.x * total_size.y) as f64,
            )
        })
        .collect();
    scaled.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut out = Vec::with_capacity(scaled.len());
    let mut remaining = area;
    let mut idx = 0;
    while idx < scaled.len() {
        // 取接下来能塞进这一行而不让长宽比变坏的项
        let mut row: Vec<(usize, f64)> = vec![scaled[idx]];
        let mut next = idx + 1;
        // 行的「长边」：当前剩余区域较短的那一维
        let is_wide = remaining.width() >= remaining.height();
        let row_length = if is_wide {
            remaining.width() as f64
        } else {
            remaining.height() as f64
        };
        while next < scaled.len() {
            let mut candidate = row.clone();
            candidate.push(scaled[next]);
            let candidate_area: f64 = candidate.iter().map(|c| c.1).sum();
            let worst_after = worst_aspect(&candidate, row_length, candidate_area);
            let worst_now = worst_aspect(&row, row_length, row.iter().map(|c| c.1).sum());
            if worst_after > worst_now {
                break;
            }
            row = candidate;
            next += 1;
        }
        let row_area: f64 = row.iter().map(|c| c.1).sum();
        let rects = place_row(&row, row_area, row_length, is_wide, remaining);
        out.extend(rects);
        // 收缩剩余区域：占用掉这一行
        let used = (row_area / (remaining.width() as f64 * remaining.height() as f64)).min(1.0);
        if is_wide {
            let h = (remaining.height() as f64 * used) as f32;
            remaining.min.y += h;
        } else {
            let w = (remaining.width() as f64 * used) as f32;
            remaining.min.x += w;
        }
        idx = next;
    }
    out
}

/// 一行内最糟糕的宽高比（越大越差）。
fn worst_aspect(row: &[(usize, f64)], row_length: f64, row_area: f64) -> f64 {
    if row_area <= 0.0 || row.is_empty() {
        return f64::MAX;
    }
    let a_max = row.iter().map(|c| c.1).fold(0.0_f64, f64::max);
    let a_min = row.iter().map(|c| c.1).fold(f64::MAX, f64::min);
    let s = row_length;
    // 经典 squarified 目标：max( s²·amax/s² , s²/(s²·amin) )
    let t1 = s * s * a_max / (row_area * row_area);
    let t2 = row_area * row_area / (s * s * a_min);
    t1.max(t2)
}

/// 把一行块铺进剩余区域。
fn place_row(
    row: &[(usize, f64)],
    row_area: f64,
    row_length: f64,
    is_wide: bool,
    remaining: egui::Rect,
) -> Vec<TreemapRect> {
    let mut out = Vec::with_capacity(row.len());
    // 行厚度：面积 / 行长度
    let thickness = row_area / row_length;
    let mut cursor = 0.0_f64;
    for &(i, a) in row {
        let side = a / thickness; // 这一块占据行长方向的长度
        let rect = if is_wide {
            egui::Rect::from_min_max(
                egui::pos2(remaining.min.x + cursor as f32, remaining.min.y),
                egui::pos2(
                    remaining.min.x + (cursor + side) as f32,
                    remaining.min.y + thickness as f32,
                ),
            )
        } else {
            egui::Rect::from_min_max(
                egui::pos2(remaining.min.x, remaining.min.y + cursor as f32),
                egui::pos2(
                    remaining.min.x + thickness as f32,
                    remaining.min.y + (cursor + side) as f32,
                ),
            )
        };
        out.push(TreemapRect {
            item_index: i,
            rect,
        });
        cursor += side;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, bytes: u64) -> TreemapItem {
        TreemapItem {
            label: label.to_string(),
            bytes,
            color: egui::Color32::BLUE,
        }
    }

    fn area() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0))
    }

    #[test]
    fn empty_and_zero_inputs_produce_nothing() {
        assert!(squarified(&[], area()).is_empty());
        let zeros = [item("a", 0), item("b", 0)];
        assert!(squarified(&zeros, area()).is_empty());
    }

    #[test]
    fn single_item_fills_the_whole_area() {
        let items = [item("only", 100)];
        let rects = squarified(&items, area());
        assert_eq!(rects.len(), 1);
        let r = rects[0].rect;
        assert!((r.width() - 1000.0).abs() < 1.0);
        assert!((r.height() - 600.0).abs() < 1.0);
    }

    #[test]
    fn areas_are_proportional_to_bytes() {
        let items = [item("big", 300), item("mid", 100), item("small", 0)];
        let rects = squarified(&items, area());
        assert_eq!(rects.len(), 2);
        let big = rects.iter().find(|r| r.item_index == 0).unwrap().rect;
        let mid = rects.iter().find(|r| r.item_index == 1).unwrap().rect;
        let big_area = big.width() as f64 * big.height() as f64;
        let mid_area = mid.width() as f64 * mid.height() as f64;
        // 300 : 100 → 3 : 1（允许少量浮点误差）
        let ratio = big_area / mid_area;
        assert!((ratio - 3.0).abs() < 0.1, "面积比应为 3:1，实际 {ratio}");
    }

    #[test]
    fn rects_do_not_overlap_and_stay_inside() {
        let items: Vec<TreemapItem> = (0..12)
            .map(|i| item(&format!("f{i}"), (i as u64 + 1) * 7))
            .collect();
        let rects = squarified(&items, area());
        assert_eq!(rects.len(), items.len());
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                // 收缩 0.5pt 后再判相交：恰好贴边的两块不算重叠。
                let a = rects[i].rect.shrink(0.5);
                let b = rects[j].rect.shrink(0.5);
                assert!(
                    !a.intersects(b),
                    "矩形不应重叠：{:?} vs {:?}",
                    rects[i].rect,
                    rects[j].rect
                );
            }
        }
        for r in &rects {
            assert!(area().contains_rect(r.rect), "矩形必须在区域内：{r:?}");
        }
    }

    #[test]
    fn zero_area_input_is_ignored() {
        let items = [item("a", 10), item("b", 0), item("c", 20)];
        let rects = squarified(&items, area());
        assert_eq!(rects.len(), 2);
        assert!(!rects.iter().any(|r| r.item_index == 1));
    }
}
