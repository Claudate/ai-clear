//! `molt-safety`：四道闸，每道一个模块，互不依赖。
//!
//! 拆开的理由：这四件事的失败模式完全不同——
//! - `deny` 失败 → 删了系统文件
//! - `fingerprint` 失败 → 删了带 key 的配置
//! - `locks` 失败 → 删了正在写的文件，构建诡异失败
//! - `symlink` 失败 → 顺着链删到了家目录
//!
//! 合在一个「安全检查器」里，测试和 review 都会变成「大概看了看」。
//!
//! 这一层**只读**：不做任何写操作，也不决定删什么，只回答「这个能不能碰」。

pub mod deny;
pub mod fingerprint;
pub mod locks;
pub mod symlink;

pub use deny::{DenyHit, DENY_RULES};
pub use fingerprint::{Hit as SecretHit, MAX_SCAN_BYTES};
pub use locks::InUse;
pub use locks::LocksSnapshot;
pub use molt_core::Signals;
pub use symlink::{normalize_roots, Resolution};

use std::path::{Path, PathBuf};

/// 一次判定所需的全部安全信号。上层拿到它之后交给 `molt_core::decide`。
///
/// 这个函数刻意不返回 `Verdict`——判定逻辑只在 core 里有一份，
/// 这里只负责**收集事实**。两边职责不重叠，规则改动不会波及到这里。
pub fn collect_signals(path: &Path, home: &str, roots: &[PathBuf], check_locks: bool) -> Signals {
    collect_signals_with(path, home, roots, check_locks, None)
}

/// 同上，但用一次抬到的系统级占用快照代替逐项 `lsof`。
///
/// `None` 表示没抓到快照（没有 lsof、权限不足），此时**不当作未占用**：
/// 与 `check` 的语义保持一致，宁可取消这一项也不误删正在写的文件。
pub fn collect_signals_with(
    path: &Path,
    home: &str,
    roots: &[PathBuf],
    check_locks: bool,
    snapshot: Option<&locks::LocksSnapshot>,
) -> Signals {
    let mut signals = Signals::default();

    // 路径必须能转成字符串才能匹配规则；转不了的话 core 会按无权限处理
    if let Some(s) = path.to_str() {
        if let Some(hit) = deny::check(s, home) {
            signals.hard_deny = Some((hit.rule.to_string(), hit.matched_pattern));
        }
    }

    if let Some(hit) = fingerprint::check_file(path) {
        signals.secret = Some((hit.pattern.to_string(), hit.file));
    }

    if check_locks {
        let info = match snapshot {
            Some(s) => s.in_use(path),
            // 没传快照时退回逐项检查，保住旧调用方的语义
            None => locks::check(path),
        };
        if let Some(info) = info {
            signals.in_use = Some((info.pid, info.name));
        }
    }

    if let Resolution::Escapes { resolved } = symlink::resolve(path, roots) {
        signals.resolved_target = Some(resolved);
    }

    signals
}

/// 扫描器应该直接跳过的子树（硬禁区）。
///
/// 提前剪枝有两个好处：不浪费时间遍历几百 GB 的系统目录，
/// 以及**从源头上让这些路径不出现在结果里**——用户看到 `~/.ssh` 出现在清单里
/// 本身就会不安。
pub fn skip_subtrees(home: &str) -> Vec<PathBuf> {
    deny::all_patterns(home)
        .into_iter()
        .filter_map(|(p, _)| {
            // `~/x/**` 是「x 这棵树整个跳过」的意思，前缀就是把尾部通配去掉
            let prefix = p.strip_suffix("/**").unwrap_or(&p);
            // 前缀里还有通配符就无法当目录用，只能靠逐项判定
            if prefix.contains('*') || prefix.contains('?') {
                None
            } else {
                Some(PathBuf::from(prefix))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const HOME: &str = "/Users/me";

    #[test]
    fn signals_capture_hard_deny() {
        let s = collect_signals(
            Path::new("/Users/me/.ssh/id_rsa"),
            HOME,
            &[PathBuf::from("/Users/me")],
            false,
        );
        assert_eq!(s.hard_deny.as_ref().unwrap().0, "ssh-keys");
    }

    #[test]
    fn normal_cache_has_no_signals() {
        let s = collect_signals(
            Path::new("/Users/me/.cargo/registry"),
            HOME,
            &[PathBuf::from("/Users/me")],
            false,
        );
        assert!(s.hard_deny.is_none());
        assert!(s.secret.is_none());
        assert!(s.in_use.is_none());
        assert!(s.resolved_target.is_none());
    }

    #[test]
    fn secret_in_a_file_is_collected_without_leaking_it() {
        let dir = std::env::temp_dir().join(format!("molt-sig-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("config.yaml.bak.20260923");
        fs::write(&f, "key: sk-proj-AbCdEf0123456789AbCdEf0123456789\n").unwrap();

        let s = collect_signals(&f, HOME, std::slice::from_ref(&dir), false);
        let (pattern, file) = s.secret.as_ref().expect("应当识别出密钥");
        assert_eq!(pattern, "openai_key");
        assert_eq!(file, &f);
        // 信号里没有可以存放密钥原文的字段
        assert!(!format!("{s:?}").contains("AbCdEf"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skip_subtrees_only_contains_literal_paths() {
        let subtrees = skip_subtrees(HOME);
        assert!(subtrees.contains(&PathBuf::from("/Users/me/.ssh")));
        assert!(subtrees.contains(&PathBuf::from("/usr")));
        // 带通配符的不能当作目录前缀，否则会误剪整棵树
        assert!(!subtrees.iter().any(|p| p.to_string_lossy().contains('*')));
    }
}
