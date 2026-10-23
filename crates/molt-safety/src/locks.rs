//! 进程占用检查：正在被打开的目录不能删。
//!
//! 这条在实际使用里最容易被忽略、后果也最烦：删掉一个正在写入的构建缓存，
//! 下一次构建会以「文件找不到」的形式失败，而用户根本联想不到是清理工具干的。
//! SQLite 的 `-wal`/`-shm` 更严重——被占用时移动它等于让数据库打不开。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 占用信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InUse {
    pub pid: u32,
    pub name: String,
}

/// 查询一个路径是否被进程打开。
///
/// 用 `lsof` 而不是 `libproc`：`libproc` 需要 unsafe FFI 与平台头文件，
/// 而这里每次只查寥寥几个路径，`lsof` 的启动开销可以接受。
/// 拿不到结果（lsof 缺失、权限不足）时返回 `None`，**不阻塞清理**——
/// 查不到占用不等于安全，所以上层会把「查不到」当作「未确认」单独处理。
pub fn check(path: &Path) -> Option<InUse> {
    let out = Command::new("/usr/sbin/lsof")
        .arg("-nP")
        .arg("-F")
        .arg("pcn")
        .arg("--")
        .arg(path)
        .output()
        .or_else(|_| {
            Command::new("lsof")
                .arg("-nP")
                .arg("-F")
                .arg("pcn")
                .arg("--")
                .arg(path)
                .output()
        })
        .ok()?;

    if !out.status.success() {
        // lsof 在「没有进程打开」时返回非 0，这是正常的「未占用」
        return None;
    }
    parse_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// 解析 `-F pcn` 的输出。格式是多行记录，每行以字段字母开头：
/// `p<pid>`、`c<command>`、`n<path>`。
fn parse_lsof(text: &str) -> Option<InUse> {
    let mut pid: Option<u32> = None;
    let mut name: Option<String> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            // 已经记满了就停：后面的记录属于别的进程，继续读会把 pid 覆盖掉
            if pid.is_some() && name.is_some() {
                break;
            }
            pid = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix('c') {
            if name.is_none() {
                name = Some(rest.trim().to_string());
            }
        }
    }

    pid.map(|p| InUse {
        pid: p,
        name: name.unwrap_or_else(|| "未知进程".to_string()),
    })
}

/// 批量检查。返回第一个命中，因为对用户来说「有人在用」这个事实
/// 比「有哪几个进程在用」更重要，而后者会让清单变得难读。
pub fn check_any(paths: &[PathBuf]) -> Option<(PathBuf, InUse)> {
    for p in paths {
        if let Some(info) = check(p) {
            return Some((p.clone(), info));
        }
    }
    None
}

/// 一次 `lsof` 拿到的全部「被打开的文件 → 进程」映射。
///
/// 为什么需要它：`check` 每调一次就 fork 一个 lsof。一次扫描有一两百个候选，
/// 那就是一两百次进程启动——实测占掉整次扫描六秒以上，而其中绝大多数路径
/// 根本没被任何进程打开。全量 lsof 只要 70 毫秒，一次抓完再查表就够。
pub struct LocksSnapshot {
    /// 被打开的文件路径，连同占用它的进程
    open: Vec<(PathBuf, InUse)>,
}

impl LocksSnapshot {
    /// 抓一次系统级的打开文件表。拿不到时返回 `None`——
    /// 与 `check` 一致：查不到不等于安全，但也不该让扫描失败。
    pub fn capture() -> Option<Self> {
        let out = Command::new("/usr/sbin/lsof")
            .args(["-nP", "-F", "pcn"])
            .output()
            .or_else(|_| Command::new("lsof").args(["-nP", "-F", "pcn"]).output())
            .ok()?;
        // lsof 只要有哪怕一个进程读不了就会返回非 0，但 stdout 仍然有效，
        // 所以这里不看 status，只看有没有内容。
        Some(Self {
            open: parse_lsof_full(&String::from_utf8_lossy(&out.stdout)),
        })
    }

    /// 这个路径（或它下面的任何文件）是否被进程打开。
    pub fn in_use(&self, path: &Path) -> Option<InUse> {
        self.open
            .iter()
            .find(|(p, _)| p == path || p.starts_with(path))
            .map(|(_, info)| info.clone())
    }

    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

/// 解析全量 `-F pcn` 输出：`p<pid>` / `c<command>` / `n<path>` 三类行，
/// 每个 path 归属最近的 pid 与 command。
fn parse_lsof_full(text: &str) -> Vec<(PathBuf, InUse)> {
    let mut out = Vec::new();
    let mut pid: Option<u32> = None;
    let mut name: Option<String> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            pid = rest.trim().parse().ok();
            name = None;
        } else if let Some(rest) = line.strip_prefix('c') {
            name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix('n') {
            if let Some(p) = pid {
                out.push((
                    PathBuf::from(rest),
                    InUse {
                        pid: p,
                        name: name.clone().unwrap_or_else(|| "未知进程".to_string()),
                    },
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_lsof_into_path_to_process_map() {
        // 全量输出里同一个进程有多条 n 行，切换进程时 pid/命令要跟着换
        let text = "p100\ncRustc\nn/Users/me/proj/target\nn/Users/me/proj/target/debug/a\n\
                    p200\ncCode\nn/Users/me/Library/Application Support/Code/Cache\n";
        let map = parse_lsof_full(text);
        assert_eq!(map.len(), 3);
        assert_eq!(map[0].1.pid, 100);
        assert_eq!(map[0].1.name, "Rustc");
        assert_eq!(map[2].1.pid, 200);
        assert_eq!(map[2].1.name, "Code");
    }

    #[test]
    fn snapshot_matches_descendants_but_not_siblings() {
        // 语义：候选路径**本身或它下面的任何文件**被打开，就算这一项在用。
        // 占用的是 target/debug/a，于是 proj/target 判为在用；
        // 同级但不相关的目录不能误伤。
        let snap = LocksSnapshot {
            open: vec![
                (
                    PathBuf::from("/Users/me/proj/target/debug/a"),
                    InUse {
                        pid: 7,
                        name: "Rustc".into(),
                    },
                ),
                (
                    PathBuf::from("/Users/me/other/x"),
                    InUse {
                        pid: 8,
                        name: "X".into(),
                    },
                ),
            ],
        };
        assert_eq!(
            snap.in_use(Path::new("/Users/me/proj/target")).unwrap().pid,
            7
        );
        // 前缀相同但不是同一层目录，不能被当成占用
        assert!(snap
            .in_use(Path::new("/Users/me/proj/targetting"))
            .is_none());
        // 反方向不算：占用的是 proj 下面的文件时，other 不受影响
        assert!(snap.in_use(Path::new("/Users/me/other/y")).is_none());
        // 祖先目录也会命中——它下面确实有文件在用，此时保守跳过是对的
        assert_eq!(snap.in_use(Path::new("/Users/me/proj")).unwrap().pid, 7);
    }

    #[test]
    fn empty_snapshot_is_not_in_use() {
        let snap = LocksSnapshot { open: vec![] };
        assert!(snap.is_empty());
        assert!(snap.in_use(Path::new("/Users/me/proj/target")).is_none());
    }

    #[test]
    fn parses_lsof_single_record() {
        let text = "p4242\ncCodex\nn/Users/me/.codex/sessions\n";
        assert_eq!(
            parse_lsof(text),
            Some(InUse {
                pid: 4242,
                name: "Codex".into()
            })
        );
    }

    #[test]
    fn takes_the_first_process_only() {
        let text = "p1\ncAlpha\nn/x\np2\ncBeta\nn/y\n";
        let got = parse_lsof(text).unwrap();
        assert_eq!(got.pid, 1);
        assert_eq!(got.name, "Alpha");
    }

    #[test]
    fn missing_pid_yields_none() {
        assert_eq!(parse_lsof("cnothing\nn/x\n"), None);
        assert_eq!(parse_lsof(""), None);
    }

    #[test]
    fn pid_without_command_still_reports() {
        let got = parse_lsof("p99\nn/x\n").unwrap();
        assert_eq!(got.pid, 99);
        assert_eq!(got.name, "未知进程");
    }

    #[test]
    fn nonexistent_path_is_not_in_use() {
        assert_eq!(check(Path::new("/definitely/not/a/real/path/xyz")), None);
    }

    #[test]
    fn empty_batch_returns_none() {
        assert_eq!(check_any(&[]), None);
    }
}
