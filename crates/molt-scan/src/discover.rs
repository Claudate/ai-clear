//! 从规则库出发，找出**实际存在**的候选路径。
//!
//! 这一步只做两件事：把规则里的模式解析成具体路径，量一下它们多大。
//! 它不判断能不能删——判断在 `molt-core`，安全信号在 `molt-safety`。
//!
//! 为什么按规则去找、而不是遍历整个家目录：家目录里几十万个文件，
//! 绝大多数与清理无关。按规则定位能在一两秒内给出结果，
//! 也能保证「结果里出现的每一项都在规则覆盖范围内」——这本身就是一道安全边界。

use molt_core::{ItemKind, PathMeta};
use std::path::{Path, PathBuf};

use crate::WalkOptions;

/// 一条规则模式解析出来的实际路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub path: PathBuf,
    pub rule_id: String,
    /// 命中的模式原文，方便用户理解「为什么它被列出来」
    pub pattern: String,
}

/// 把一个模式里的 `*` / `**` 展开成真实存在的路径。
///
/// 只支持**以固定前缀开头、后面接通配**的模式（规则库里的写法都是这样）。
/// 不做通用 glob：那需要遍历全盘，且会让「清理范围」变得不可预测。
pub fn expand(pattern: &str, home: &str) -> Vec<PathBuf> {
    // `~` 前缀必须在这里展开。
    //
    // 规则加载（RuleSet::from_toml）时也会展开一次，但 expand 是公开函数，
    // 测试与未来调用方可能直接传带 `~` 的模式。这里不再展开的话，
    // `~/Library/pnpm/store` 会被当成相对路径 `Path::new("~/Library/...")`，
    // 永远返回 0 命中——规则看起来写了，实际一条都没生效，而且不报错。
    let expanded;
    let pattern = if pattern == "~" {
        home
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        expanded = format!("{}/{}", home.trim_end_matches('/'), rest);
        &expanded
    } else {
        pattern
    };

    // 以 `/**` 结尾是「这棵树整体都是候选」的意思。
    // 必须在这里特判：否则 `**` 会把每个后代目录都展开成单独条目，
    // 再对每个条目递归统计体积，复杂度退化为 O(n²)，扫一个 node_modules 就会卡死。
    if let Some(root) = pattern.strip_suffix("/**") {
        if !root.contains('*') && !root.contains('?') {
            let p = PathBuf::from(root);
            return if p.exists() { vec![p] } else { vec![] };
        }
    }

    // 以 `**/` 开头的是「任意层级下叫这个名字的东西」。
    // 这类模式没有固定前缀，不能靠字符串切分定位，必须真的去搜；
    // 所以搜索根必须显式给出（家目录），否则会扫穿整块磁盘。
    if let Some(rest) = pattern.strip_prefix("**/") {
        let name = rest.strip_suffix("/**").unwrap_or(rest);
        if !name.contains('/') && !name.contains('*') && !name.contains('?') {
            return search_by_name(name, home);
        }
        return Vec::new();
    }

    let star = match pattern.find(['*', '?']) {
        Some(i) => i,
        None => {
            let p = PathBuf::from(pattern);
            return if p.exists() { vec![p] } else { vec![] };
        }
    };

    // 前缀必须落在某个目录边界上，否则 `**/target` 这种模式无法定位起点
    let prefix = &pattern[..star];
    let base = match prefix.rfind('/') {
        Some(i) => &prefix[..i],
        None => return vec![],
    };
    if base.is_empty() {
        return vec![];
    }

    let base_path = Path::new(base);
    if !base_path.is_dir() {
        return vec![];
    }

    // 剩余模式（去掉 base 与分隔符）
    let rest = pattern[base.len() + 1..].to_string();
    let mut out = Vec::new();
    collect_matches(base_path, &rest, &mut out);
    out.sort();
    out.dedup();
    out
}

/// 在 `dir` 下按 `rest` 模式逐层匹配。
///
/// 遇到 `**` 时同时尝试「本层就此打住」与「再往下钻」两条路，
/// 这样 `/a/**/target` 既能匹配 `/a/target` 也能匹配 `/a/b/c/target`。
fn collect_matches(dir: &Path, rest: &str, out: &mut Vec<PathBuf>) {
    if rest.is_empty() {
        if dir.exists() {
            out.push(dir.to_path_buf());
        }
        return;
    }

    let (head, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };

    if head == "**" {
        // 零层：`**` 直接不消费，交给剩下的模式匹配当前位置
        if let Some(t) = tail {
            collect_matches(dir, t, out);
        } else if dir.exists() {
            out.push(dir.to_path_buf());
        }
        // 一层或多层：往下走，`**` 保持原样继续
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() && !is_symlink(&p) {
                    collect_matches(&p, rest, out);
                }
            }
        }
        return;
    }

    if head.contains('*') || head.contains('?') {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if molt_core::glob::glob_match(head, &name) {
                    let p = entry.path();
                    match tail {
                        Some(t) => {
                            if p.is_dir() && !is_symlink(&p) {
                                collect_matches(&p, t, out);
                            }
                        }
                        None => out.push(p),
                    }
                }
            }
        }
        return;
    }

    let next = dir.join(head);
    match tail {
        Some(t) => {
            if next.is_dir() && !is_symlink(&next) {
                collect_matches(&next, t, out);
            }
        }
        None => {
            if next.exists() {
                out.push(next);
            }
        }
    }
}

fn is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// 按目录名在家目录下搜索。
///
/// 这是一个**有界**遍历：只在家目录内，且跳过一些明显不可能含目标的大树。
/// 不剪枝的话，一次 `**/node_modules` 会把 `~/.cargo` 里几万个 crate 源码翻一遍，
/// 那既慢又没有意义（那些 crate 里不会有 node_modules）。
fn search_by_name(name: &str, home: &str) -> Vec<PathBuf> {
    search_many_by_name(&[name], home)
        .remove(name)
        .unwrap_or_default()
}

/// 一次遍历同时找多个名字。
///
/// 规则库里有六七个 `**/xxx/**` 模式，逐个调用会让家目录被完整走六遍。
/// 合并成一次走完，是这次扫描从 4 分 40 秒降到可接受范围的关键。
pub fn search_many_by_name(
    names: &[&str],
    home: &str,
) -> std::collections::HashMap<String, Vec<PathBuf>> {
    let mut hits: std::collections::HashMap<String, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for n in names {
        hits.insert((*n).to_string(), Vec::new());
    }
    let root = Path::new(home);
    if !root.is_dir() {
        return hits;
    }
    const MAX_DEPTH: usize = 8;
    walk_for_names(root, names, 0, MAX_DEPTH, &mut hits);
    for v in hits.values_mut() {
        v.sort();
        v.dedup();
    }
    hits
}

fn walk_for_names(
    dir: &Path,
    want: &[&str],
    depth: usize,
    max: usize,
    out: &mut std::collections::HashMap<String, Vec<PathBuf>>,
) {
    if depth > max {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if is_symlink(&p) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if want.contains(&name.as_str()) {
            // 命中即止：node_modules 里不会再有需要关心的 node_modules
            if let Some(v) = out.get_mut(&name) {
                v.push(p);
            }
            continue;
        }

        if should_prune(&p) {
            continue;
        }
        walk_for_names(&p, want, depth + 1, max, out);
    }
}

/// 搜索时不必进入的目录。判据是「这里面不可能出现我们要找的项目级目录」。
fn should_prune(p: &Path) -> bool {
    let name = match p.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    matches!(
        name,
        ".git"
            | ".svn"
            | ".hg"
            | "node_modules"
            | ".cargo"
            | ".rustup"
            | "Library"
            | "Applications"
            | ".Trash"
            | ".Spotlight-V100"
            | ".fseventsd"
            | "System"
    )
}

/// 按规则库找出所有真实存在的目标，并量出体积。
///
/// 只返回**叶子级**目标：如果一条规则命中了 `/a/b`，而另一条更具体的规则命中
/// `/a/b/c`，两者都会出现。谁最终生效由 `molt-core` 的优先级排序决定——
/// 这里不做裁决，只负责把事实摆出来。
pub fn discover(
    rules: &molt_core::RuleSet,
    opts: &WalkOptions,
    home: &str,
) -> Vec<(Discovered, PathMeta)> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut out = Vec::new();

    // 按「模式具体程度」排一遍：越具体的规则越先占位。
    // 否则 `**/node_modules/**` 这类通用规则会先把路径收走，
    // 后面 `**/.wrangler/tmp/**` 这种更具体的规则就再也轮不到——
    // 表现是「明明写了规则却没生效」，而且不会有任何报错。
    let mut entries: Vec<(&molt_core::Rule, &String)> = Vec::new();
    for rule in rules.iter() {
        for pattern in &rule.patterns {
            entries.push((rule, pattern));
        }
    }
    entries.sort_by(|a, b| {
        let al = molt_core::glob::literal_prefix_len(b.1);
        let bl = molt_core::glob::literal_prefix_len(a.1);
        al.cmp(&bl).then_with(|| b.0.priority.cmp(&a.0.priority))
    });

    // 已收录的路径集合，用于识别「父目录 vs 子目录」的重复。
    let mut accepted: Vec<PathBuf> = Vec::new();

    // 先把候选路径全部算出来，再统一量体积。
    //
    // 分成两趟是为了能并行：量体积是全流程里唯一真正耗时的部分
    // （要递归读几十万个 inode），而它只依赖路径本身，与先后顺序无关。
    // 边展开边量的话，线程要等前一条规则算完才能开始，等于没并行。
    let mut pending: Vec<(&molt_core::Rule, &String, PathBuf, ItemKind)> = Vec::new();

    // 先把所有 `**/name/**` 模式的目标名收齐，一次遍历找全。
    // 逐个搜索会让家目录被走六遍——这是整次扫描的主要耗时来源。
    let names: Vec<&str> = entries
        .iter()
        .filter_map(|(_, p)| {
            p.strip_prefix("**/")
                .map(|r| r.strip_suffix("/**").unwrap_or(r))
        })
        .filter(|n| !n.contains('/') && !n.contains('*') && !n.contains('?'))
        .collect();
    let name_hits = search_many_by_name(&names, home);

    for (rule, pattern) in entries {
        let expanded_paths: Vec<PathBuf> = if let Some(rest) = pattern.strip_prefix("**/") {
            let name = rest.strip_suffix("/**").unwrap_or(rest);
            name_hits.get(name).cloned().unwrap_or_default()
        } else {
            expand(pattern, home)
        };
        for path in expanded_paths {
            // 已被更具体的规则收过的路径不重复列
            if !seen.insert(path.clone()) {
                continue;
            }
            // 若某个已收录的路径是本路径的祖先，本路径的空间已经算在祖先里了。
            // 不跳过就会出现「父目录 28 GB + 子目录 12 GB」累加成 40 GB 的假数字。
            if accepted.iter().any(|a| path.starts_with(a) && *a != path) {
                continue;
            }
            if opts.skip.iter().any(|s| path.starts_with(s)) {
                continue;
            }
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let kind = if meta.file_type().is_symlink() {
                ItemKind::Symlink
            } else if meta.is_dir() {
                ItemKind::Dir
            } else {
                ItemKind::File
            };

            accepted.push(path.clone());
            pending.push((rule, pattern, path, kind));
        }
    }

    // 同一路径可能被多条规则命中，但上面的 `seen` 已经去掉了重复；
    // 这里直接按先后顺序量一遍，结果用下标对回候选。
    let paths: Vec<PathBuf> = pending.iter().map(|(_, _, p, _)| p.clone()).collect();
    let sizes = crate::measure_many(&paths, opts);

    for ((rule, pattern, path, kind), r) in pending.into_iter().zip(sizes) {
        let days = last_used_days(&path);
        out.push((
            Discovered {
                path: path.clone(),
                rule_id: rule.id.clone(),
                pattern: pattern.clone(),
            },
            PathMeta {
                path,
                kind,
                bytes: r.bytes,
                alloc_bytes: r.alloc_bytes,
                files: r.files,
                last_used_days: days,
            },
        ));
    }

    // 大的排前面：用户第一眼该看到最占地方的那几项
    out.sort_by_key(|a| std::cmp::Reverse(a.1.alloc_bytes));
    out
}

/// 距上次访问（或修改）多少天。目录用 mtime——它至少能反映「最近有没有动过」。
fn last_used_days(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let elapsed = std::time::SystemTime::now().duration_since(modified).ok()?;
    Some(elapsed.as_secs() / 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 测试用的固定家目录。不读真实 $HOME，避免测试结果随机器变化。
    const HOME: &str = "/Users/placeholder";

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-disc-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn tilde_prefix_is_expanded_by_expand_itself() {
        // 回归测试：这条曾让所有 `~/...` 规则静默失效（返回 0 命中且不报错）
        let home = tmpdir("tilde");
        fs::create_dir_all(home.join("Library/pnpm/store")).unwrap();

        let got = expand("~/Library/pnpm/store/**", &home.to_string_lossy());
        assert_eq!(got.len(), 1, "~ 必须展开，实际 {got:?}");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn leading_double_star_searches_by_name() {
        // 这曾经是最大的一个漏洞：`**/node_modules/**` 根本没展开出任何路径，
        // 导致项目级缓存（node_modules / target / .gocache）完全没被扫到。
        let home = tmpdir("byname");
        let proj = home.join("proj/a");
        fs::create_dir_all(proj.join("node_modules")).unwrap();
        fs::write(proj.join("node_modules/x.js"), b"1").unwrap();

        let got = expand("**/node_modules/**", &home.to_string_lossy());
        assert_eq!(got.len(), 1, "应当找到 node_modules，实际 {got:?}");
        assert!(got[0].ends_with("node_modules"));

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn name_search_does_not_descend_into_cargo_registry() {
        // 搜索必须剪掉 .cargo 这类巨大且不可能含目标的位置，否则会慢到不可用
        let home = tmpdir("prune");
        let deep = home.join(".cargo/registry/src/x");
        fs::create_dir_all(deep.join("node_modules")).unwrap();

        let got = expand("**/node_modules/**", &home.to_string_lossy());
        assert!(got.is_empty(), "不应钻进 .cargo，实际 {got:?}");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn trailing_double_star_returns_only_the_root() {
        // 回归测试：这条曾经会把每个后代都展开成条目，导致 O(n²) 扫描卡死
        let d = tmpdir("tailds");
        fs::create_dir_all(d.join("a/b/c")).unwrap();
        fs::write(d.join("a/b/c/f.txt"), b"x").unwrap();
        let pat = format!("{}/**", d.to_string_lossy());
        let got = expand(&pat, HOME);
        assert_eq!(got, vec![d.clone()], "应当只返回根，实际 {got:?}");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn exact_path_returns_itself() {
        let d = tmpdir("exact");
        let f = d.join("a.txt");
        fs::write(&f, b"x").unwrap();
        assert_eq!(expand(&f.to_string_lossy(), HOME), vec![f.clone()]);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_exact_path_returns_empty() {
        assert!(expand("/no/such/file/xyz", HOME).is_empty());
    }

    #[test]
    fn double_star_matches_zero_or_more_levels() {
        let d = tmpdir("ds");
        fs::create_dir_all(d.join("a/b/target")).unwrap();
        fs::create_dir_all(d.join("target")).unwrap();
        let pat = format!("{}/**/target", d.to_string_lossy());
        let got = expand(&pat, HOME);
        assert_eq!(got.len(), 2, "实际：{got:?}");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn single_star_matches_one_level() {
        let d = tmpdir("ss");
        fs::create_dir_all(d.join("v1/store")).unwrap();
        fs::create_dir_all(d.join("v2/store")).unwrap();
        fs::create_dir_all(d.join("x/y/store")).unwrap();
        let pat = format!("{}/*/store", d.to_string_lossy());
        let got = expand(&pat, HOME);
        assert_eq!(got.len(), 2, "单星不应跨层：{got:?}");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn does_not_follow_symlinks_while_expanding() {
        let d = tmpdir("nosym");
        let outside = tmpdir("nosym-out");
        fs::create_dir_all(outside.join("target")).unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();

        let pat = format!("{}/**/target", d.to_string_lossy());
        let got = expand(&pat, HOME);
        assert!(got.is_empty(), "不应跟随软链接找到外部目标：{got:?}");
        fs::remove_dir_all(&d).ok();
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn discover_reports_size_and_rule() {
        let d = tmpdir("discover");
        let cache = d.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("x.bin"), vec![0u8; 2048]).unwrap();

        let rs = molt_core::RuleSet::from_toml(
            "t.toml",
            &format!(
                r#"
schema_version = 1
[[rule]]
id = "test-cache"
patterns = ["{}/cache"]
what = "测试缓存"
verdict = {{ type = "reclaimable" }}
"#,
                d.to_string_lossy()
            ),
            "/Users/placeholder",
        )
        .unwrap();

        let got = discover(&rs, &WalkOptions::default(), HOME);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.rule_id, "test-cache");
        assert_eq!(got[0].1.bytes, 2048);
        assert!(got[0].1.alloc_bytes >= 2048);

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn discover_skips_denied_subtrees() {
        let d = tmpdir("skip");
        fs::create_dir_all(d.join("secret")).unwrap();
        let rs = molt_core::RuleSet::from_toml(
            "t.toml",
            &format!(
                r#"
schema_version = 1
[[rule]]
id = "s"
patterns = ["{}/secret"]
what = "秘密"
verdict = {{ type = "reclaimable" }}
"#,
                d.to_string_lossy()
            ),
            "/Users/placeholder",
        )
        .unwrap();

        let opts = WalkOptions {
            skip: vec![d.join("secret")],
            ..Default::default()
        };
        assert!(discover(&rs, &opts, HOME).is_empty());
        fs::remove_dir_all(&d).ok();
    }

    /// 把 mtime 拨到 `days` 天前。用 utimensat 而不是 `filetime` crate：
    /// 这里只需要一个系统调用，为此多一个依赖不划算。
    fn set_mtime_days_ago(path: &Path, days: u64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let ts = now - (days as i64) * 86_400;
        let times = [
            libc::timespec {
                tv_sec: ts,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: ts,
                tv_nsec: 0,
            },
        ];
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(
            rc,
            0,
            "设置 mtime 失败：{}",
            std::io::Error::last_os_error()
        );
    }

    fn stale_rules(path: &str) -> molt_core::RuleSet {
        molt_core::RuleSet::from_toml(
            "t.toml",
            &format!(
                r#"
schema_version = 1
[[rule]]
id = "stale-target"
patterns = ["{path}"]
what = "构建产物"
stale_days = 30
verdict = {{ type = "costly", cost = "recompile", note = "全量重编" }}
"#
            ),
            HOME,
        )
        .unwrap()
    }

    /// 端到端：真实目录 + 真实 mtime → 发现 → 判定，看 `suggest_clean`。
    /// 全程在临时目录里，不碰真实家目录。
    #[test]
    fn a_target_untouched_for_60_days_is_suggested_for_cleaning() {
        let d = tmpdir("stale60");
        let target = d.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("artifact.bin"), vec![0u8; 1024]).unwrap();
        set_mtime_days_ago(&target, 60);

        let rs = stale_rules(&target.to_string_lossy());
        let got = discover(&rs, &WalkOptions::default(), HOME);
        assert_eq!(got.len(), 1);

        let (discovered, meta) = &got[0];
        assert_eq!(meta.last_used_days, Some(60), "mtime 应当被读成 60 天前");

        let roots = vec![d.clone()];
        let c = molt_core::decide(meta, &rs, &molt_core::Signals::default(), &roots);
        assert!(
            c.suggest_clean,
            "60 天没动过、阈值 30 天，应当建议清理（路径 {}）",
            discovered.path.display()
        );
        assert!(matches!(c.verdict, molt_core::Verdict::Costly { .. }));

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_target_touched_today_is_not_suggested() {
        let d = tmpdir("fresh");
        let target = d.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("artifact.bin"), vec![0u8; 1024]).unwrap();
        // 刚写过的目录，mtime 就是现在

        let rs = stale_rules(&target.to_string_lossy());
        let got = discover(&rs, &WalkOptions::default(), HOME);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.last_used_days, Some(0), "今天动过的应当是 0 天");

        let roots = vec![d.clone()];
        let c = molt_core::decide(&got[0].1, &rs, &molt_core::Signals::default(), &roots);
        assert!(!c.suggest_clean, "今天还在用的不该被建议清理");

        fs::remove_dir_all(&d).ok();
    }

    /// 读不到时间时返回 None——这是「没数据」，不是「零天」。
    #[test]
    fn a_vanished_path_has_no_last_used_value() {
        let d = tmpdir("gone");
        let target = d.join("target");
        fs::create_dir_all(&target).unwrap();
        let rs = stale_rules(&target.to_string_lossy());
        let got = discover(&rs, &WalkOptions::default(), HOME);
        assert_eq!(got.len(), 1);

        fs::remove_dir_all(&target).unwrap();
        assert_eq!(last_used_days(&target), None);

        // 即使规则写了阈值，没有时间数据也不能建议清理
        let roots = vec![d.clone()];
        let mut meta = got[0].1.clone();
        meta.last_used_days = last_used_days(&target);
        let c = molt_core::decide(&meta, &rs, &molt_core::Signals::default(), &roots);
        assert!(!c.suggest_clean, "没数据不等于很久没用");

        fs::remove_dir_all(&d).ok();
    }
}
