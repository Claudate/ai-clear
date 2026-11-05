//! 把 `.app` 的图标解码成界面能贴的纹理。
//!
//! 卸载页要按图 2 的样子排——一排排图标 + 名字，像 Finder 和启动台那样。
//! 只写名字的清单让用户逐行读路径，认不出那是哪个应用；而图标是人一眼就
//! 认得的那个东西。
//!
//! 三个来源，按可靠性排序：
//!
//! 1. `Contents/Resources/<CFBundleIconFile>.icns` —— 最常见，绝大多数应用走这条
//! 2. `Contents/Resources/` 里任意一个 `.icns` —— 有些应用 Info.plist 没写键名
//! 3. `Contents/Resources/*.png` —— 少数跨平台应用的图标直接就是 PNG
//!
//! `.icns` 是容器格式，里面装的是若干张 PNG 或老式 RLE 压缩位图。
//! 现代 macOS 应用（10.7 之后）几乎全是 PNG，所以这里只认 PNG 那几种类型：
//! 遇到纯 RLE 的老图标就返回 `None`，界面上退回「首字母方块」——
//! 自己实现 RLE 解码只为一个早已不常见的格式，不值得。
//!
//! 纹理按路径缓存在 `egui::Context` 里，而且**每帧只解一次**：一次占用几千个
//! 图标的应用列表，每帧重新解码会让界面卡死。

use std::path::{Path, PathBuf};

use eframe::egui;

/// 一个应用图标的解码结果。
pub struct AppIcon {
    pub image: egui::ColorImage,
}

/// 图标的显示边长（逻辑点）。
///
/// 按启动台的观感定：图标要明显大于名字，整页才像「一堆应用」而不是
/// 「一张带小图的表」。44 点太小——缩略图上像一串彩色小方块，认不出是哪个应用。
pub const ICON_SIDE: f32 = 64.0;

/// 从 `.app` 里解出图标。
///
/// `bundle_icon` 是 Info.plist 里的 `CFBundleIconFile`（可能不带扩展名）。
pub fn load_icon(app_path: &Path, bundle_icon: Option<&str>) -> Option<AppIcon> {
    let res = app_path.join("Contents/Resources");
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(name) = bundle_icon {
        let name = name.trim();
        if !name.is_empty() {
            candidates.push(res.join(name));
            if !name.ends_with(".icns") && !name.ends_with(".png") {
                candidates.push(res.join(format!("{name}.icns")));
                candidates.push(res.join(format!("{name}.png")));
            }
        }
    }
    // 兜底：资源目录里任意一个 .icns。按体积降序试——大的通常含更高分辨率。
    if let Ok(entries) = std::fs::read_dir(&res) {
        let mut found: Vec<(u64, PathBuf)> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "icns").unwrap_or(false))
            .filter_map(|p| {
                let n = std::fs::metadata(&p).ok()?.len();
                Some((n, p))
            })
            .collect();
        found.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        candidates.extend(found.into_iter().map(|(_, p)| p));
    }

    for path in candidates {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if path.extension().map(|x| x == "png").unwrap_or(false) {
            if let Some(img) = decode_png(&bytes) {
                return Some(AppIcon { image: img });
            }
            continue;
        }
        if let Some(img) = decode_icns(&bytes) {
            return Some(AppIcon { image: img });
        }
    }
    None
}

/// 解析 `.icns` 容器，取出里面最大的一张 PNG。
///
/// 选最大的那张而不是「最接近 44 点」的：纹理之后要按 44 点画，
/// 大图缩小比小图放大清楚得多，而缩放在 GPU 上是免费的。
fn decode_icns(bytes: &[u8]) -> Option<egui::ColorImage> {
    if bytes.len() < 8 || &bytes[0..4] != b"icns" {
        return None;
    }
    // 只要 PNG 类型的条目。这些是 icns 里装 PNG 的全部类型码：
    // ic04/05 = 16/32pt、ic07..ic10 = 128/256/512/1024、
    // ic11..ic14 = 带 @2x 的那批。老式的 is32/il32/ih32/it32 是 RLE，跳过。
    const PNG_TYPES: [&[u8; 4]; 11] = [
        b"ic04", b"ic05", b"ic07", b"ic08", b"ic09", b"ic10", b"ic11", b"ic12", b"ic13", b"ic14",
        b"icp4",
    ];

    let mut best: Option<(u32, egui::ColorImage)> = None;
    let mut off = 8usize;
    while off + 8 <= bytes.len() {
        let ty = &bytes[off..off + 4];
        // 容器内部长度是大端 u32
        let len = u32::from_be_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        if len < 8 || off + len > bytes.len() {
            break;
        }
        if PNG_TYPES.iter().any(|t| t.as_slice() == ty) {
            if let Some(img) = decode_png(&bytes[off + 8..off + len]) {
                let area = (img.size[0] * img.size[1]) as u32;
                if best.as_ref().map(|(a, _)| area > *a).unwrap_or(true) {
                    best = Some((area, img));
                }
            }
        }
        off += len;
    }
    best.map(|(_, img)| img)
}

/// 解一张 PNG。
///
/// 走 `imagedecode::from_png`，和品牌 logo 同一份实现：两处都要「RGB 补成不透明」，
/// 各写一份的话迟早只有一处被改到，另一处会在某个不带 alpha 的图标上出现黑底。
fn decode_png(bytes: &[u8]) -> Option<egui::ColorImage> {
    crate::imagedecode::from_png(bytes)
}

/// 取某个应用的图标纹理，第一次出现时解码并上传，之后每帧复用。
///
/// `ctx.data` 里存而不是存进 `AppState`：这一层是纯展示，两个地方（列表与详情）
/// 都要用同一张纹理，走缓存比把纹理句柄穿过一整条调用链清楚。
pub fn texture(
    ctx: &egui::Context,
    app_path: &Path,
    bundle_icon: Option<&str>,
) -> Option<egui::TextureHandle> {
    // `Id::unique` 会按类型+值做 hash，拿路径字符串做盐正好一个应用一张纹理
    let id = egui::Id::unique(("app-icon", app_path.display().to_string()));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let icon = load_icon(app_path, bundle_icon)?;
    let t = ctx.load_texture(
        format!("icon-{}", app_path.file_stem()?.to_string_lossy()),
        icon.image,
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个只有一个 PNG 条目的最小 icns。
    fn make_icns(png: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"icns");
        let total = (8 + 8 + png.len()) as u32;
        out.extend_from_slice(&total.to_be_bytes());
        out.extend_from_slice(b"ic09");
        out.extend_from_slice(&((8 + png.len()) as u32).to_be_bytes());
        out.extend_from_slice(png);
        out
    }

    /// 造一张纯色 PNG。
    fn solid_png(w: u32, h: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut buf, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            // 一次传整张图：`write_image_data` 要的是完整缓冲，不是一行
            let row: Vec<u8> = [10u8, 20, 30, 255]
                .iter()
                .copied()
                .cycle()
                .take((w * 4) as usize)
                .collect();
            let mut all = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..h {
                all.extend_from_slice(&row);
            }
            writer.write_image_data(&all).unwrap();
        }
        buf
    }

    #[test]
    fn icns_png_payload_decodes() {
        let icns = make_icns(&solid_png(64, 64));
        let img = decode_icns(&icns).expect("ic09 里的 PNG 应当能解出来");
        assert_eq!(img.size, [64, 64]);
        assert_eq!(img.pixels[0].a(), 255);
    }

    #[test]
    fn the_largest_embedded_image_wins() {
        // 小图在前、大图在后：取到大的才对。取到小的会糊。
        let mut small = Vec::new();
        {
            let mut e = png::Encoder::new(&mut small, 16, 16);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&vec![255u8; 16 * 16 * 4]).unwrap();
        }
        let mut buf = Vec::new();
        buf.extend_from_slice(b"icns");
        let payload = 8 + 8 + small.len() + 8 + solid_png(128, 128).len();
        buf.extend_from_slice(&(payload as u32).to_be_bytes());
        buf.extend_from_slice(b"ic07");
        buf.extend_from_slice(&((8 + small.len()) as u32).to_be_bytes());
        buf.extend_from_slice(&small);
        let big = solid_png(128, 128);
        buf.extend_from_slice(b"ic08");
        buf.extend_from_slice(&((8 + big.len()) as u32).to_be_bytes());
        buf.extend_from_slice(&big);

        let img = decode_icns(&buf).unwrap();
        assert_eq!(img.size, [128, 128], "应当取最大那张");
    }

    #[test]
    fn garbage_returns_none_instead_of_panicking() {
        // 图标坏了只能退回首字母方块，绝不能让整页崩掉
        assert!(decode_icns(b"not an icns file at all").is_none());
        assert!(decode_icns(&[]).is_none());
        assert!(decode_png(b"\x89PNG\r\n\x1a\nbroken").is_none());
    }

    #[test]
    fn a_real_app_icon_decodes() {
        // 拿本机真实存在的应用验一次：icns 的实际条目顺序与类型码，
        // 只有真文件能证明 `PNG_TYPES` 那张表是对的。
        let apps = Path::new("/Applications");
        let Ok(entries) = std::fs::read_dir(apps) else {
            return; // 没有 /Applications 的环境跳过
        };
        let mut tried = 0;
        for e in entries
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "app").unwrap_or(false))
        {
            tried += 1;
            if load_icon(&e.path(), None).is_some() {
                return;
            }
            if tried >= 12 {
                break;
            }
        }
        panic!("试了 {tried} 个真实应用，一张图标都没解出来——PNG_TYPES 表很可能不对");
    }
}
