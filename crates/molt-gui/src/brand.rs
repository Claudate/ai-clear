//! 品牌 logo：编译进二进制的那一张图，顶栏和窗口图标共用。
//!
//! 图在 `crates/molt-gui/assets/icon.png`，来源是 `public/macos/icon_512x512.png`。
//! 打包进二进制而不是运行时读文件，是因为 app 里没有可靠的相对路径——
//! 用户可能把 .app 拖到任何地方，运行时按路径找图迟早会找不到，那时
//! 顶栏会空一块，而窗口图标会退回系统默认。编译进去就不会有这种情况。
//!
//! 只解码一次：`color_image()` 的结果被 `OnceLock` 记住，之后每帧拿到的
//! 都是同一份像素，纹理上传也只有第一次那一下。

use std::sync::OnceLock;

/// 编译进二进制的 logo 像素。改图只改这一个文件，顶栏与窗口图标同时变。
const LOGO_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// 解码后的像素缓存。`None` 表示这张图解不开——那时界面退回纯文字品牌，
/// 不 panic：图标画不出来是外观问题，不该让整个窗口起不来。
static CACHE: OnceLock<Option<egui::ColorImage>> = OnceLock::new();

/// 解出来的 logo 像素（RGBA，未预乘）。解码失败返回 `None`。
pub fn color_image() -> Option<&'static egui::ColorImage> {
    CACHE.get_or_init(decode).as_ref()
}

fn decode() -> Option<egui::ColorImage> {
    crate::imagedecode::from_png(LOGO_PNG)
}

/// 顶栏里 logo 的高度（点）。与品牌文字的视觉高度对齐：
/// 文字是 17 点，图标再高一点才压得住它，不然看起来像掉在文字旁边。
pub const BAR_LOGO_HEIGHT: f32 = 30.0;

/// 取顶栏用的纹理，第一次调用时上传，之后每帧直接复用。
///
/// 不能每帧 `load_texture`：那样每帧都会新建一张纹理并上传一遍像素，
/// 白白吃掉显存和带宽，而这个图标一辈子都不会变。存在 egui 的临时数据里
/// 而不是 `MoltApp` 的字段里，是为了让 `top_bar` 这种自由函数也能拿到，
/// 不用把纹理一路穿过所有调用层。
pub fn texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique("brand-logo-texture");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = color_image()?.clone();
    let t = ctx.load_texture("brand-logo", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

/// 窗口/Dock 图标要的正方形像素数据。
///
/// 只用于 `ViewportBuilder::with_icon`；和顶栏共用同一张解码结果，
/// 不另读一次文件。
pub fn window_icon() -> Option<egui::IconData> {
    let img = color_image()?;
    Some(egui::IconData {
        rgba: img.as_raw().to_vec(),
        width: img.size[0] as u32,
        height: img.size[1] as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// logo 必须真的解得开：解不开时顶栏会静默退回纯文字，
    /// 那种「看起来正常、其实没换」正是这一轮要避免的。
    #[test]
    fn the_embedded_logo_decodes() {
        let img = color_image().expect("内嵌的 logo 解不开——顶栏会退回纯文字");
        assert!(
            img.size[0] >= 64 && img.size[1] >= 64,
            "尺寸太小：{:?}",
            img.size
        );
        assert_eq!(
            img.size[0], img.size[1],
            "app 图标应当是正方形：{:?}",
            img.size
        );
    }

    /// 四角必须是透明的。不透明的话，白底方块会盖在顶栏的卡片底色上，
    /// 看起来像贴了一张纸。
    #[test]
    fn the_corners_are_transparent() {
        let img = color_image().unwrap();
        let (w, h) = (img.size[0], img.size[1]);
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            let px = img.pixels[y * w + x];
            assert!(px.a() < 16, "({x},{y}) 应当透明，实际 alpha = {}", px.a());
        }
    }

    /// 窗口图标与顶栏必须是同一张图——两处各存一份就会有一处忘了改。
    #[test]
    fn window_icon_uses_the_same_pixels() {
        let img = color_image().unwrap();
        let icon = window_icon().unwrap();
        assert_eq!(icon.width as usize, img.size[0]);
        assert_eq!(icon.height as usize, img.size[1]);
        assert_eq!(icon.rgba.len(), img.as_raw().len());
    }
}
