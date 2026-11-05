//! 图标：一套正经的开源图标库，而不是手画的线条。
//!
//! 之前工具页那几枚图标是拿 `painter.line_segment` 一条条凑出来的——线条
//! 粗细、圆角、比例全靠手调，放大到 69 点时曲率不对、接缝处还会露出来，
//! 看起来就像随手画的草图。这里换成 [Phosphor Icons](https://phosphoricons.com)
//! 的 Regular 字重，MIT / Apache-2.0 双许可，2600+ 枚同族图标，粗细和圆角是
//! 一个人画的一套——这才是「好看的图标」和「手画图标」的实际差别。
//!
//! 用法：一个图标就是写出它的文字字符，用 [`font`] 作字号。
//!
//! ```text
//! ui.painter().text(pos, egui::Align2::CENTER_CENTER, icons::FOLDER_PLUS, icons::font(p.sz(46.0)), color);
//! ```
//!
//! 为什么不引 `egui-phosphor`：那个 crate 最新版（0.14）绑的是 egui 0.36，
//! 本项目是 0.37，装进来会同时链两份 egui，字体与纹理都各走一套。字体文件
//! 本身没有版本概念，直接带上它的 TTF 更稳，也少一个版本跟着升的依赖。

use crate::theme::Palette;

/// 字体数据。放在 `assets/` 里、`include_bytes!` 编进二进制：
/// 这是一个要分发的 `.app`，运行时不能指望资源目录还在原位。
pub const FONT_BYTES: &[u8] = include_bytes!("../assets/Phosphor.ttf");

/// 字体在 [`egui::FontDefinitions`] 里的键名。
pub const FONT_KEY: &str = "phosphor-regular";

/// 图标字体自己的家族。不混进 `Proportional`，见 `theme::install_fonts` 里的说明。
pub fn family() -> egui::FontFamily {
    egui::FontFamily::Name(FONT_KEY.into())
}

/// 图标字号。传进来的已经乘过 `Palette` 的缩放。
pub fn font(size: f32) -> egui::FontId {
    egui::FontId::new(size, family())
}

/// 画一枚图标（居中在 `rect` 的中心，边长取短边）。
///
/// 直接给矩形的版本，省得每处调用都自己算中心点：图标在软底方块里居中，
/// 差一两个像素在 100 点的大图标上看得出来。
pub fn draw(painter: &egui::Painter, glyph: &str, rect: egui::Rect, color: egui::Color32) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        font(rect.width().min(rect.height())),
        color,
    );
}

/// 一枚图标 + 可选颜色的简写，供按钮等地方使用。
pub fn rich(glyph: &str, size: f32, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(glyph).font(font(size)).color(color)
}

/// 用当前主题的前景色画一枚图标的大小（`RichText`），给 label 类控件拼字用。
pub fn rich_for(p: &Palette, glyph: &str, size: f32) -> egui::RichText {
    rich(glyph, size, p.text)
}

// ─── 图标清单 ─────────────────────────────────────────────────────────
//
// 码点取自 @phosphor-icons/web 2.1.2 的 regular/style.css。
// 不在代码里走连字（写 "folder-plus" 让字体自己整形）：egui 的文本层不做
// 连字替换，写名字只会得到一串字母。

/// 新建文件夹 / 文件
pub const FOLDER_PLUS: &str = "\u{e258}";
/// 打开（外部程序）
pub const ARROW_SQUARE_OUT: &str = "\u{e5de}";
/// 在访达中显示
pub const FOLDER_OPEN: &str = "\u{e256}";
/// 文件夹
pub const FOLDER: &str = "\u{e24a}";
/// 文件
pub const FILE: &str = "\u{e230}";
/// 新建文件
pub const FILE_PLUS: &str = "\u{e236}";
/// 删除
pub const TRASH: &str = "\u{e4a6}";
/// 扫描
pub const MAGNIFYING_GLASS: &str = "\u{e30c}";
/// 时钟
pub const CLOCK: &str = "\u{e19a}";
/// 重来 / 历史
pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{e1a0}";
/// 立方体（卸载页）
pub const CUBE: &str = "\u{e1da}";
/// 设置
pub const GEAR: &str = "\u{e270}";
/// 清理 / 扫帚
pub const BROOM: &str = "\u{ec54}";
/// 闪亮 / AI
pub const SPARKLE: &str = "\u{e6a2}";
/// 加号
pub const PLUS: &str = "\u{e3d4}";
/// 撤销
pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{e038}";
/// 上行右箭头（详情）
pub const ARROW_UP_RIGHT: &str = "\u{e092}";
/// 磁盘 / 存储
pub const HARD_DRIVES: &str = "\u{e2a0}";

// ─── 顶栏导航图标 ─────────────────────────────────────────────────

/// 仪表盘：清理（首页那颗圆环的缩影）
pub const GAUGE: &str = "\u{e628}";
/// 托盘：临时空间（东西放在盘里，不是箭头向下——箭头向下会读成「下载」）
pub const ARCHIVE_TRAY: &str = "\u{e010}";
/// 日历：自动清理
pub const CALENDAR_BLANK: &str = "\u{e10a}";
/// 扳手：工具
pub const WRENCH: &str = "\u{e5d4}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_file_is_a_real_ttf() {
        // 0x00010000 是 TrueType 的 sfnt 版本标记；从 CDN 抓错文件时
        // （比如回了个 HTML 错误页）这里立刻会挂，而不是等到运行时图标
        // 全变成方框。
        assert!(
            FONT_BYTES.len() > 100_000,
            "字体文件太小：{}",
            FONT_BYTES.len()
        );
        assert_eq!(&FONT_BYTES[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn glyphs_are_single_private_use_chars() {
        // 每个常量都必须正好是一个私用区码位（U+E000–U+F8FF）：
        // 这是图标字体的约定，写成两个字符或普通字母都会在界面上显示成文字。
        for g in [
            FOLDER_PLUS,
            ARCHIVE_TRAY,
            ARROW_SQUARE_OUT,
            FOLDER_OPEN,
            FOLDER,
            FILE,
            FILE_PLUS,
            TRASH,
            MAGNIFYING_GLASS,
            CLOCK,
            CLOCK_COUNTER_CLOCKWISE,
            CUBE,
            GEAR,
            BROOM,
            SPARKLE,
            PLUS,
            ARROW_COUNTER_CLOCKWISE,
            ARROW_UP_RIGHT,
            HARD_DRIVES,
            GAUGE,
            CALENDAR_BLANK,
            WRENCH,
        ] {
            let chars: Vec<char> = g.chars().collect();
            assert_eq!(chars.len(), 1, "图标应是单字符，实际 {g:?}");
            let cp = chars[0] as u32;
            assert!(
                (0xE000..=0xF8FF).contains(&cp),
                "应落在私用区，实际 U+{cp:04X}"
            );
        }
    }
}
