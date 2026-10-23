//! 内置规则库：把 `rules/*.toml` 编进二进制。
//!
//! 编进去而不是运行时读文件，是为了让「装好就能用」成立——
//! 用户不该为了清理缓存先去下载一份规则。同时保留覆盖能力：
//! `~/.config/molt/rules/` 下的同名文件会整体替换内置版本，
//! 这样规则更新不必等发版。

use include_dir::{include_dir, Dir};

static RULES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../rules");

/// 内置规则的文件名与内容。
pub fn builtin_files() -> Vec<(&'static str, &'static str)> {
    RULES
        .files()
        .filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("toml"))
        .filter_map(|f| {
            let name = f.path().file_name()?.to_str()?;
            Some((name, f.contents_utf8()?))
        })
        .collect()
}

/// 加载内置规则 + 用户覆盖，合并成一个规则库。
///
/// 覆盖语义：用户的同名文件**整体替换**内置版本，而不是逐条合并。
/// 逐条合并会让「我删掉了一条规则」这种意图无法表达。
pub fn load(home: &str, user_dir: Option<&std::path::Path>) -> Result<molt_core::RuleSet, String> {
    let mut set = molt_core::RuleSet::default();
    let overrides = user_dir
        .map(|d| {
            std::fs::read_dir(d)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("toml"))
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            std::fs::read_to_string(e.path()).ok().map(|t| (name, t))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let overridden: Vec<&str> = overrides.iter().map(|(n, _)| n.as_str()).collect();

    let builtin: Vec<(String, String)> = builtin_files()
        .into_iter()
        .filter(|(n, _)| !overridden.contains(n))
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect();

    for (name, text) in overrides.iter().chain(builtin.iter()) {
        let rs = molt_core::RuleSet::from_toml(name, text, home)
            .map_err(|e| format!("规则文件 {name} 有误：{e}"))?;
        set.merge(rs);
    }

    if set.is_empty() {
        return Err("规则库为空".to_string());
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::Verdict;

    const HOME: &str = "/Users/me";

    fn set() -> molt_core::RuleSet {
        load(HOME, None).expect("内置规则必须能加载")
    }

    #[test]
    fn builtin_rules_load() {
        let s = set();
        assert!(s.len() >= 30, "规则数偏少，实际 {}", s.len());
    }

    #[test]
    fn every_builtin_rule_has_a_description() {
        for r in set().iter() {
            assert!(!r.what.trim().is_empty(), "规则 {} 缺少说明文字", r.id);
        }
    }

    #[test]
    fn every_costly_rule_explains_its_cost() {
        for r in set().iter() {
            if let molt_core::RuleVerdict::Costly { note, .. } = &r.verdict {
                assert!(
                    !note.trim().is_empty(),
                    "规则 {} 是可重建项但没写代价",
                    r.id
                );
            }
        }
    }

    #[test]
    fn codex_sessions_are_protected() {
        let s = set();
        let r = s
            .best_match("/Users/me/.codex/sessions/2026/10/20/abc.jsonl")
            .expect("应该命中 Codex 会话规则");
        assert!(matches!(
            r.verdict.to_verdict(&r.id),
            Verdict::Protected { .. }
        ));
    }

    #[test]
    fn claude_project_history_is_protected() {
        let s = set();
        let r = s
            .best_match("/Users/me/.claude/file-history/abc/def")
            .expect("应该命中 Claude 历史规则");
        assert!(matches!(
            r.verdict.to_verdict(&r.id),
            Verdict::Protected { .. }
        ));
    }

    #[test]
    fn codex_browser_cache_is_reclaimable() {
        let s = set();
        let r = s
            .best_match("/Users/me/Library/Application Support/Codex/Code Cache/js/x")
            .expect("应该命中 Codex 缓存规则");
        assert_eq!(r.verdict.to_verdict(&r.id), Verdict::Reclaimable);
    }

    #[test]
    fn cursor_history_is_costly_not_reclaimable() {
        // 这是本产品与普通清理器最关键的分歧点之一：
        // Cursor 的 History 看起来像缓存，其实是用户的文件版本记录。
        let s = set();
        let r = s
            .best_match("/Users/me/Library/Application Support/Cursor/User/History/abc/entry")
            .expect("应该命中 Cursor 历史规则");
        assert!(
            matches!(r.verdict.to_verdict(&r.id), Verdict::Costly { .. }),
            "Cursor 历史不能判成可回收"
        );
    }

    #[test]
    fn vscode_cache_and_history_get_different_verdicts() {
        let s = set();
        let cache = s
            .best_match("/Users/me/Library/Application Support/Code/Code Cache/x")
            .expect("缓存规则");
        let history = s
            .best_match("/Users/me/Library/Application Support/Code/User/History/x")
            .expect("历史规则");
        assert_eq!(cache.verdict.to_verdict(&cache.id), Verdict::Reclaimable);
        assert!(matches!(
            history.verdict.to_verdict(&history.id),
            Verdict::Costly { .. }
        ));
    }

    #[test]
    fn rust_target_is_costly_with_a_note() {
        let s = set();
        let r = s
            .best_match("/Users/me/project/x/target/debug/build/a")
            .expect("应该命中 target 规则");
        match r.verdict.to_verdict(&r.id) {
            Verdict::Costly { rebuild } => {
                assert!(!rebuild.note.is_empty());
                assert!(!rebuild.summary().is_empty());
            }
            other => panic!("target 应为有代价项，实际 {other:?}"),
        }
    }

    #[test]
    fn project_go_cache_is_matched() {
        let s = set();
        let r = s
            .best_match("/Users/me/project/web/cpa/CLIProxyAPI/.gocache/0a")
            .expect("应该命中 .gocache 规则");
        assert!(matches!(
            r.verdict.to_verdict(&r.id),
            Verdict::Costly { .. }
        ));
    }

    #[test]
    fn cargo_registry_is_matched() {
        let s = set();
        let r = s
            .best_match("/Users/me/.cargo/registry/cache/index.crates.io/x.crate")
            .expect("应该命中 cargo 规则");
        assert!(matches!(
            r.verdict.to_verdict(&r.id),
            Verdict::Costly { .. }
        ));
    }

    #[test]
    fn pnpm_store_is_not_silently_treated_as_free() {
        // pnpm store 被硬链接共享，删它会影响所有项目——必须有代价说明
        let s = set();
        let r = s
            .best_match("/Users/me/Library/pnpm/store/v3/files/ab/cd")
            .expect("应该命中 pnpm store 规则");
        match r.verdict.to_verdict(&r.id) {
            Verdict::Costly { rebuild } => assert!(
                rebuild.note.contains("硬链接"),
                "命中的是规则 {}，note={:?}",
                r.id,
                rebuild.note
            ),
            other => panic!("pnpm store 应为有代价项，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_path_has_no_rule() {
        assert!(set().best_match("/Users/me/Documents/notes.txt").is_none());
    }
}
