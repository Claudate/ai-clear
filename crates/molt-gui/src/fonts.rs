//! macOS 系统字体加载。
//!
//! 自绘 GUI 最容易翻车的地方就是中文：egui 自带字体没有一套完整的中文字形，
//! 不装系统字体的话，界面上每个汉字都会变成豆腐块。
//!
//! 这里读的是系统里真实存在的字体文件，而不是把字体打包进二进制：
//! 后者会让二进制涨十几 MB，而 macOS 上这些字体一定在。

use std::path::{Path, PathBuf};

/// 中文字体候选，按优先级排列。
///
/// 顺序有讲究：前两个是 macOS 自带、覆盖完整 CJK 的字体；
/// `PingFang.ttc` 虽然在但它是 TTC 集合，egui 只能取第一个字面，
/// 而且 macOS 上它偶尔被系统替换，所以作为兜底而不是首选。
const CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/PingFang.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
];

/// 找到第一个可读的中文字体文件。
pub fn find_cjk_font() -> Option<PathBuf> {
    CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file() && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false))
}

/// 读出字体字节。失败时返回 `None`，界面会退化成英文可用、中文缺字——
/// 这比整个程序起不来要好，至少用户还能看到发生了什么。
pub fn load_cjk_font() -> Option<(String, Vec<u8>)> {
    let path = find_cjk_font()?;
    let bytes = read(&path)?;
    Some((path.display().to_string(), bytes))
}

fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_cjk_font_on_this_machine() {
        // 这条断言在 macOS 上必须成立：找不到中文字体的 GUI 是不可用的
        let found = find_cjk_font();
        assert!(
            found.is_some(),
            "没有找到任何中文字体，候选：{CANDIDATES:?}"
        );
    }

    #[test]
    fn loaded_font_is_not_empty() {
        if let Some((name, bytes)) = load_cjk_font() {
            assert!(!bytes.is_empty(), "{name} 读出来是空的");
            assert!(bytes.len() > 100_000, "{name} 太小，不像字体文件");
        }
    }
}
