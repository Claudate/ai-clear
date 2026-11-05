//! 把一张 PNG 字节解成 egui 能贴的像素。
//!
//! 两个调用方：品牌 logo（编译进二进制的那张）与应用图标（从 `.app` 里读出来的）。
//! 两处都需要同一件事——解出 RGBA——而且都对「没有 alpha 的图」有同样的处理：
//! 补成不透明。各写一份的话，补 alpha 的那几行迟早只有一处被改到，
//! 另一处就会在某个不带 alpha 的图标上出现黑底。

/// 解一张 PNG。RGBA 与 RGB 都收，其余颜色类型返回 `None`。
///
/// 灰度与索引色在 logo 和应用图标里都不出现，就不为它们写分支了：
/// 真遇到时退回调用方的兜底（纯文字品牌 / 首字母方块），比猜一个颜色更安全。
pub fn from_png(bytes: &[u8]) -> Option<egui::ColorImage> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;

    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let rgba = match info.color_type {
        // 我们的源图就是 RGBA，这条是主路径。
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        // 没有 alpha 的图补成不透明，省得调用方各写一遍。
        png::ColorType::Rgb => {
            let src = &buf[..info.buffer_size()];
            let mut out = Vec::with_capacity(w * h * 4);
            for px in src.as_chunks::<3>().0 {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        _ => return None,
    };

    Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(w: u32, h: u32, color: png::ColorType) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(color);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            let bpp = if color == png::ColorType::Rgba { 4 } else { 3 };
            writer
                .write_image_data(&vec![128u8; (w * h * bpp) as usize])
                .unwrap();
        }
        out
    }

    #[test]
    fn rgba_png_keeps_its_alpha() {
        let img = from_png(&encode(8, 8, png::ColorType::Rgba)).unwrap();
        assert_eq!(img.size, [8, 8]);
        assert_eq!(img.pixels[0].a(), 128, "alpha 必须原样保留");
    }

    #[test]
    fn rgb_png_comes_out_opaque() {
        // 补 alpha 是这个函数存在的理由：漏了它，图标会在深色底上发黑
        let img = from_png(&encode(4, 4, png::ColorType::Rgb)).unwrap();
        assert_eq!(img.size, [4, 4]);
        assert_eq!(img.pixels[0].a(), 255);
    }

    #[test]
    fn garbage_is_none_not_a_panic() {
        assert!(from_png(b"not a png").is_none());
        assert!(from_png(&[]).is_none());
    }
}
