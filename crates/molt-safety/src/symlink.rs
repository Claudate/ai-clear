//! 符号链接逃逸检查。
//!
//! 这是最容易被低估的一道闸：用户选的是「清理项目缓存」，但如果项目里有一个
//! 指向 `~/` 或 `/etc` 的软链接，去掉链接本身没问题，**递归进去删内容就会出事**。
//! 所以扫描时必须先 `canonicalize`，再确认解析结果仍在授权根之内。

use std::path::{Path, PathBuf};

/// 链接的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// 不是链接，或是链接但仍在授权范围内
    Inside { resolved: PathBuf },
    /// 链接指向授权根之外
    Escapes { resolved: PathBuf },
    /// 链接断了（目标不存在），无法判定
    Broken,
}

/// 解析一个可能是指向链接的路径。
///
/// `roots` 必须**已经规范化**（调用方传进来的真实绝对路径）。
/// 这里再规范化一次目标，是因为目标本身也可能是链式的。
pub fn resolve(path: &Path, roots: &[PathBuf]) -> Resolution {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        // 路径本身都不存在，当作断链处理：不删、不报错
        Err(_) => return Resolution::Broken,
    };

    if !meta.file_type().is_symlink() {
        return Resolution::Inside {
            resolved: path.to_path_buf(),
        };
    }

    match std::fs::canonicalize(path) {
        Ok(resolved) => {
            if roots.iter().any(|r| resolved.starts_with(r)) {
                Resolution::Inside { resolved }
            } else {
                Resolution::Escapes { resolved }
            }
        }
        Err(_) => Resolution::Broken,
    }
}

/// 规范化授权根。相对路径与 `~` 在这里一次性展开，
/// 避免每个调用点都自己处理一遍——那才是这类 bug 的温床。
pub fn normalize_roots(raw: &[PathBuf]) -> Vec<PathBuf> {
    raw.iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// 测试专用临时目录。带进程 id，避免并行测试互相踩。
    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-symlink-test-{}-{}-{}",
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
    fn regular_file_is_inside() {
        let dir = tmpdir("regular");
        let f = dir.join("a.txt");
        fs::write(&f, b"x").unwrap();
        let root = fs::canonicalize(&dir).unwrap();
        assert!(matches!(resolve(&f, &[root]), Resolution::Inside { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn link_staying_inside_is_allowed() {
        let dir = tmpdir("inside");
        let target = dir.join("real");
        fs::create_dir_all(&target).unwrap();
        let link = dir.join("link");
        symlink(&target, &link).unwrap();
        let root = fs::canonicalize(&dir).unwrap();
        match resolve(&link, &[root]) {
            Resolution::Inside { resolved } => {
                assert!(resolved.ends_with("real"));
            }
            other => panic!("应当允许，实际 {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn link_pointing_to_system_is_an_escape() {
        let dir = tmpdir("escape");
        let link = dir.join("evil");
        symlink("/etc", &link).unwrap();
        let root = fs::canonicalize(&dir).unwrap();
        match resolve(&link, &[root]) {
            // macOS 上 /etc 是 /private/etc 的软链接，canonicalize 会如实给出后者，
            // 所以断言「不在根内」而不是断言字面路径。
            Resolution::Escapes { resolved } => {
                assert!(resolved.starts_with("/private/etc") || resolved.starts_with("/etc"));
            }
            other => panic!("应当判定为逃逸，实际 {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_link_is_reported_as_broken_not_dangerous() {
        let dir = tmpdir("broken");
        let link = dir.join("dangling");
        symlink("/nope/does/not/exist", &link).unwrap();
        let root = fs::canonicalize(&dir).unwrap();
        assert_eq!(resolve(&link, &[root]), Resolution::Broken);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn normalize_roots_drops_nonexistent_paths() {
        let dir = tmpdir("normalize");
        let roots = normalize_roots(&[dir.clone(), PathBuf::from("/nope/xyz")]);
        assert_eq!(roots.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }
}
