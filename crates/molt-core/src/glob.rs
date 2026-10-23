//! 通配符匹配：只服务于规则库里的路径模式，不追求完整的 glob 语义。
//!
//! 支持 `*`（段内任意串）、`?`（段内单字符）、`**`（跨任意层目录）。
//! 自己实现而不是引入 globset，是因为规则模式的形态很窄，且判定逻辑必须能被单测逐条覆盖。

/// 判断 `path` 是否匹配 `pattern`。两侧都按 `/` 切段，空段忽略（容忍结尾斜杠）。
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&p, &t)
}

fn match_segments(p: &[&str], t: &[&str]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    // `**` 消费零到任意多层目录，回溯到第一个能匹配剩余模式的位置
    if p[0] == "**" {
        for i in 0..=t.len() {
            if match_segments(&p[1..], &t[i..]) {
                return true;
            }
        }
        return false;
    }
    if t.is_empty() {
        return false;
    }
    if !match_one(p[0], t[0]) {
        return false;
    }
    match_segments(&p[1..], &t[1..])
}

/// 单段匹配，`*` 与 `?` 只在段内生效（不会跨过 `/`）。
fn match_one(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let c: Vec<char> = s.chars().collect();
    let (mut pi, mut ci) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_ci = 0usize;

    while ci < c.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == c[ci]) {
            pi += 1;
            ci += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_ci = ci;
            pi += 1;
        } else if let Some(sp) = star {
            // 回溯：让上一个 `*` 多吞一个字符
            pi = sp + 1;
            star_ci += 1;
            ci = star_ci;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// 模式里第一个通配符之前的部分有多长——用来在两条规则同时命中时挑更具体的那条。
pub fn literal_prefix_len(pattern: &str) -> usize {
    match pattern.find(['*', '?']) {
        Some(i) => i,
        None => pattern.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_path_matches() {
        assert!(glob_match(
            "/Users/me/.cargo/registry",
            "/Users/me/.cargo/registry"
        ));
        assert!(!glob_match(
            "/Users/me/.cargo/registry",
            "/Users/me/.cargo/git"
        ));
    }

    #[test]
    fn double_star_spans_directories() {
        assert!(glob_match("/p/**/target", "/p/a/b/target"));
        assert!(glob_match("/p/**/target", "/p/target"));
        assert!(!glob_match("/p/**/target", "/p/a/b/targets"));
    }

    #[test]
    fn single_star_stays_in_segment() {
        assert!(glob_match("/p/*/target", "/p/a/target"));
        assert!(!glob_match("/p/*/target", "/p/a/b/target"));
    }

    #[test]
    fn question_mark_is_one_char() {
        assert!(glob_match("/p/v?", "/p/v2"));
        assert!(!glob_match("/p/v?", "/p/v22"));
    }

    #[test]
    fn trailing_slash_is_ignored() {
        assert!(glob_match("/p/a/**", "/p/a/"));
        assert!(glob_match("/p/a/", "/p/a"));
    }

    #[test]
    fn literal_prefix_orders_specificity() {
        assert!(literal_prefix_len("/a/b/c/**") > literal_prefix_len("/a/**"));
        assert_eq!(literal_prefix_len("/a/b"), 4);
    }
}
