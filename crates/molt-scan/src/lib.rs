//! 目录树遍历：只读、可剪枝、带 inode 去重。
//!
//! 这一层唯一的职责是把「磁盘上有什么、各占多大」量准。它**不做判定**，
//! 也不知道什么该删——判定全部在 `molt-core`。
//!
//! 为什么单独拆出来：量的口径错了，后面所有结论都是错的。而口径错得很隐蔽——
//! 硬链接会让同一个 inode 被重复计数，APFS 稀疏文件会让「逻辑大小」远大于「实际占用」。
//! BuhoCleaner 那张图里「发现 11.74 GB / 选中 10.16 GB」对不上，多半就是这里没算对。

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub mod discover;
pub mod disk;

pub use discover::{discover, Discovered};

/// 一个目录/文件量出来的体积。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Measured {
    /// 逻辑大小：各文件 `st_size` 之和
    pub bytes: u64,
    /// 实际占用：各文件 `st_blocks * 512` 之和，已按 inode 去重
    pub alloc_bytes: u64,
    /// 计入的文件数（去重后）
    pub files: u64,
}

/// 遍历时的限制。默认值面向「扫一个缓存目录」，不是「扫全盘」。
#[derive(Debug, Clone, Default)]
pub struct WalkOptions {
    /// 最多下钻几层。0 表示只看这一层，`None` 表示不限制。
    pub max_depth: Option<usize>,
    /// 遇到这些前缀直接跳过（硬禁区）
    pub skip: Vec<PathBuf>,
    /// 是否跟随符号链接。默认 false——跟随就等于把清理范围扩到了链接目标
    pub follow_symlinks: bool,
}

/// 遍历结果。`denied` 记录因权限不足跳过的条目数——
/// 用户需要知道「这次没扫全」，而不是以为磁盘就这么点东西。
#[derive(Debug, Clone, Default)]
pub struct WalkResult {
    pub measured: Measured,
    pub denied: u64,
}

/// 量一个路径（文件或目录）的体积。
pub fn measure(path: &Path, opts: &WalkOptions) -> WalkResult {
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut result = WalkResult::default();
    walk(path, opts, 0, &mut seen, &mut result);
    result
}

/// 一次量多个路径，用标准库的 scope 并行。
///
/// 为什么不用 rayon：只为这一处并行引入一个依赖不划算，而 `thread::scope`
/// 足够——各路径之间完全独立，合并结果时没有任何共享可变状态。
///
/// **按任务抢，不按块切。** 一开始写的是把路径均分成 N 段、每段一个线程，
/// 实测反而更慢：候选里有一个 13 GB / 2.8 万文件的 `target`，它落在哪一段，
/// 那段就成了唯一的长尾，其余线程早早空转。改成用原子下标抢任务后，
/// 大目录谁空谁接，负载自己就平了。
pub fn measure_many(paths: &[PathBuf], opts: &WalkOptions) -> Vec<Measured> {
    if paths.is_empty() {
        return Vec::new();
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(paths.len());

    if threads <= 1 {
        return paths.iter().map(|p| measure(p, opts).measured).collect();
    }

    let next = std::sync::atomic::AtomicUsize::new(0);
    // 每个线程独占一个槽位写自己的结果，收尾时按槽位拼接——
    // 这样热循环里不需要锁，也不会有两个线程抢同一格。
    let mut slots: Vec<Vec<(usize, Measured)>> = vec![Vec::new(); threads];
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for (slot, out) in slots.iter_mut().enumerate() {
            let _ = slot;
            let next = &next;
            handles.push(s.spawn(move || loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(p) = paths.get(i) else { break };
                out.push((i, measure(p, opts).measured));
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    });

    let mut results: Vec<Option<Measured>> = vec![None; paths.len()];
    for (i, m) in slots.into_iter().flatten() {
        results[i] = Some(m);
    }
    results.into_iter().map(|m| m.unwrap_or_default()).collect()
}

fn walk(
    path: &Path,
    opts: &WalkOptions,
    depth: usize,
    seen: &mut HashSet<(u64, u64)>,
    out: &mut WalkResult,
) {
    if opts.skip.iter().any(|s| path.starts_with(s)) {
        return;
    }
    if let Some(max) = opts.max_depth {
        if depth > max {
            return;
        }
    }

    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            out.denied += 1;
            return;
        }
        Err(_) => return,
    };

    // 符号链接只算链接自身的体积，绝不穿过去——穿过去可能连到家目录
    if meta.file_type().is_symlink() {
        out.measured.bytes += meta.len();
        out.measured.alloc_bytes += meta.blocks() * 512;
        return;
    }

    if meta.is_file() {
        add_file(&meta, seen, out);
        return;
    }

    if !meta.is_dir() {
        return;
    }

    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            out.denied += 1;
            return;
        }
        Err(_) => return,
    };

    for entry in entries {
        let Ok(entry) = entry else { continue };
        let child = entry.path();
        // 不跟随链接时，用 symlink_metadata 判断；跟随则用 metadata
        if opts.follow_symlinks {
            if let Ok(m) = fs::metadata(&child) {
                if m.is_file() {
                    add_file(&m, seen, out);
                    continue;
                }
            }
        }
        walk(&child, opts, depth + 1, seen, out);
    }
}

/// 按 (dev, inode) 去重后累加。
///
/// 为什么要去重：APFS 上 `pnpm store` 与多个项目的 `node_modules` 是硬链接，
/// 同一个数据块被 N 个路径引用。不去重就会报出 N 倍的空间，
/// 而用户真正能释放的只有一份——这正是「总数对不上」的根源。
fn add_file(meta: &fs::Metadata, seen: &mut HashSet<(u64, u64)>, out: &mut WalkResult) {
    let key = (meta.dev(), meta.ino());
    if !seen.insert(key) {
        return;
    }
    out.measured.bytes += meta.len();
    out.measured.alloc_bytes += meta.blocks() * 512;
    out.measured.files += 1;
}

/// 列出目录下的直接子项，用于「先看这一层有什么」。
///
/// 返回的每一项都已经量过体积，这样上层可以直接按大小排序，
/// 而不必再对每个子项各跑一次遍历。
pub fn children(path: &Path, opts: &WalkOptions) -> Vec<(PathBuf, Measured)> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if opts.skip.iter().any(|s| p.starts_with(s)) {
            continue;
        }
        let r = measure(&p, opts);
        out.push((p, r.measured));
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.1.alloc_bytes));
    out
}

/// 人类可读的体积。用 1024 进制，与 Finder 和 `du -h` 一致。
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-scan-{}-{}-{}",
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
    fn measure_many_matches_serial_results() {
        // 并行的前提是「各路径独立」：并行版与逐个量必须给出完全一样的数字，
        // 否则界面上「大头是哪几个」会随线程数变化，而这没人能复现。
        let d = tmpdir("many");
        let mut paths = Vec::new();
        for i in 0..7 {
            let p = d.join(format!("p{i}"));
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("f.bin"), vec![0u8; 1000 * (i + 1)]).unwrap();
            paths.push(p);
        }

        let serial: Vec<Measured> = paths
            .iter()
            .map(|p| measure(p, &WalkOptions::default()).measured)
            .collect();
        let parallel = measure_many(&paths, &WalkOptions::default());
        assert_eq!(parallel, serial);

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn measure_many_handles_empty_input() {
        assert!(measure_many(&[], &WalkOptions::default()).is_empty());
    }

    #[test]
    fn counts_files_recursively() {
        let d = tmpdir("basic");
        fs::write(d.join("a.txt"), vec![0u8; 100]).unwrap();
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("sub/b.txt"), vec![0u8; 200]).unwrap();

        let r = measure(&d, &WalkOptions::default());
        assert_eq!(r.measured.files, 2);
        assert_eq!(r.measured.bytes, 300);
        assert!(r.measured.alloc_bytes >= 300);

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn hard_links_are_counted_once() {
        let d = tmpdir("hardlink");
        let a = d.join("a.bin");
        fs::write(&a, vec![0u8; 4096]).unwrap();
        let b = d.join("b.bin");
        fs::hard_link(&a, &b).unwrap();

        let r = measure(&d, &WalkOptions::default());
        // 两个路径指向同一个 inode，只能算一份
        assert_eq!(r.measured.files, 1);
        assert_eq!(r.measured.bytes, 4096);

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn symlinks_are_not_followed_by_default() {
        let d = tmpdir("symlink");
        let outside = tmpdir("symlink-target");
        fs::write(outside.join("big.bin"), vec![0u8; 8192]).unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();
        fs::write(d.join("small.txt"), vec![0u8; 10]).unwrap();

        let r = measure(&d, &WalkOptions::default());
        // 只算 small.txt 与链接自身的 st_size（软链接的大小=目标路径字符串长度），
        // 关键是绝不能把目标目录里那 8192 字节算进来。
        assert_eq!(r.measured.files, 1);
        assert!(r.measured.bytes < 8192, "不应跟随链接到目标目录");
        assert!(r.measured.bytes >= 10);

        fs::remove_dir_all(&d).ok();
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn skip_prefix_prunes_the_subtree() {
        let d = tmpdir("skip");
        fs::write(d.join("keep.txt"), vec![0u8; 10]).unwrap();
        fs::create_dir_all(d.join("secret")).unwrap();
        fs::write(d.join("secret/x.txt"), vec![0u8; 999]).unwrap();

        let opts = WalkOptions {
            skip: vec![d.join("secret")],
            ..Default::default()
        };
        let r = measure(&d, &opts);
        assert_eq!(r.measured.bytes, 10);

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn max_depth_limits_descent() {
        let d = tmpdir("depth");
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/b/deep.txt"), vec![0u8; 500]).unwrap();

        let opts = WalkOptions {
            max_depth: Some(1),
            ..Default::default()
        };
        let r = measure(&d, &opts);
        assert_eq!(r.measured.files, 0);

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn children_are_sorted_by_alloc_size() {
        let d = tmpdir("children");
        fs::create_dir_all(d.join("small")).unwrap();
        fs::write(d.join("small/a.txt"), vec![0u8; 10]).unwrap();
        fs::create_dir_all(d.join("big")).unwrap();
        fs::write(d.join("big/a.txt"), vec![0u8; 100_000]).unwrap();

        let list = children(&d, &WalkOptions::default());
        assert_eq!(list[0].0.file_name().unwrap(), "big");

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn human_bytes_matches_du_style() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(human_bytes(1024u64.pow(3)), "1.0 GB");
    }

    #[test]
    fn missing_path_is_zero_not_a_panic() {
        let r = measure(Path::new("/no/such/path/xyz"), &WalkOptions::default());
        assert_eq!(r.measured.bytes, 0);
        assert_eq!(r.denied, 0);
    }
}
