//! ISO 时间戳的短显示。
//!
//! 历史页、定时清理页、临时空间页原先各有一份**完全相同**的实现。
//! 三份都一样还算运气好；只要有一处改动，三个页面就会对同一时刻显示
//! 出不同格式——用户会以为它们是不同的东西。

/// `2026-10-20T10:00:00Z` → `2026-10-20 10:00`。
///
/// 不解析时区、不做本地化：这里显示的是审计库里记下的时刻，
/// 换算成当地时间反而会让「什么时候删的」对不上记录。
///
/// 长度不足或切点不落在字符边界上时原样返回，不 panic——
/// 审计库正常写入的都是 ASCII，但这个函数不该因为一条脏数据就整页崩掉。
pub fn short_time(iso: &str) -> String {
    // 先按字符边界校验，再切。直接 `&iso[0..10]` 遇到多字节字符会 panic。
    if iso.len() >= 16 && iso.is_char_boundary(10) && iso.is_char_boundary(16) {
        format!("{} {}", &iso[0..10], &iso[11..16])
    } else {
        iso.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::short_time;

    #[test]
    fn keeps_the_date_and_minute() {
        assert_eq!(short_time("2026-10-20T10:00:00Z"), "2026-10-20 10:00");
    }

    #[test]
    fn passes_short_strings_through() {
        assert_eq!(short_time("2026-10-20"), "2026-10-20");
        assert_eq!(short_time(""), "");
    }

    #[test]
    fn does_not_panic_on_multibyte() {
        // 按字节切片会在这里炸：切点落在字符中间就是 panic，整页界面跟着崩。
        let s = short_time("二〇二六年十月二十日十点零零分");
        assert!(!s.is_empty());
    }
}
