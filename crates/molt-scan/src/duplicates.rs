//! 重复文件检测：找出内容完全相同的一组文件。
//!
//! 三阶段筛选，避免对大文件盲目整体哈希：
//! 1. 按大小分组（大小不同不可能相同）；
//! 2. 同组内比较头尾各 4 KiB 采样，不一致直接排除；
//! 3. 剩下的候选才计算完整 SHA-256，完全一致才归为一组。
//!
//! 硬链接按 (dev, inode) 去重：两个路径指向同一数据块并不是「重复」，
//! 释放不出空间。无法读取或扫描过程中变化的文件被跳过并计数。
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::WalkOptions;

/// 采样长度：文件头尾各读这么多。小于等于两倍长度的文件直接整体读。
const SAMPLE: usize = 4096;
/// 完整哈希的流式缓冲区。
const BUF: usize = 64 * 1024;

/// 重复组里的一个文件。
#[derive(Debug, Clone)]
pub struct DuplicateFile {
    pub path: PathBuf,
    pub bytes: u64,
    /// 修改时间（秒精度）。扫描途中文件被改，靠它识别并跳过。
    pub modified: Option<SystemTime>,
}

/// 一组内容完全相同的文件。`bytes` 是单个文件的大小。
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub files: Vec<DuplicateFile>,
    pub bytes: u64,
}

/// 一次重复扫描的结果。
#[derive(Debug, Clone, Default)]
pub struct DuplicateScan {
    /// 内容重复的分组（每组至少 2 个文件）。
    pub groups: Vec<DuplicateGroup>,
    /// 因不可读 / 扫描中变化而跳过的文件数。
    pub skipped: u64,
    /// 时间预算用尽，结果只是部分遍历的下界。
    pub truncated: bool,
    /// 扫到的普通文件总数（去重后）。界面上用它回答「到底看了多少」。
    pub files_seen: u64,
}

/// 扫描当前处在哪个阶段。界面必须能说清「现在在干什么」——
/// 只说「正在查找」时，用户看不出它是在走目录还是在算哈希。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicatePhase {
    /// 正在遍历目录，收集文件
    Walking,
    /// 正在比对同大小文件的头尾样本
    Sampling,
    /// 正在计算完整哈希，确认内容一致
    Hashing,
}

impl DuplicatePhase {
    /// 界面上直接显示的一句说明。
    pub fn label(self) -> &'static str {
        match self {
            DuplicatePhase::Walking => "正在遍历目录…",
            DuplicatePhase::Sampling => "正在比对大小相同的文件…",
            DuplicatePhase::Hashing => "正在校验内容完全一致的文件…",
        }
    }
}

/// 扫描过程中的快照。`total` 为 0 表示这一阶段的总量还不知道。
#[derive(Debug, Clone, Copy)]
pub struct DuplicateTick {
    pub phase: DuplicatePhase,
    pub done: usize,
    pub total: usize,
    pub files_seen: u64,
}

/// 找出 `root` 下的重复文件。
///
/// `opts` 里的 `skip` 会被当作硬禁区；`budget` 用于保护遍历不无限挂起。
pub fn find_duplicates(root: &Path, opts: &WalkOptions) -> DuplicateScan {
    find_duplicates_with_progress(root, opts, |_| {})
}

/// 同 `find_duplicates`，但在每个阶段与每批文件后回调一次进度。
///
/// 回调是只读的观察者：它不能改变扫描结果，也不能让扫描提前结束——
/// 收手只由 `budget` 决定。这样界面层就没法通过回调影响「算了什么」。
pub fn find_duplicates_with_progress<F>(
    root: &Path,
    opts: &WalkOptions,
    mut on_tick: F,
) -> DuplicateScan
where
    F: FnMut(&DuplicateTick),
{
    let mut scan = DuplicateScan::default();
    let started = std::time::Instant::now();

    // 第一阶段：收集普通文件，按大小分组，硬链接去重。
    let mut by_size: HashMap<u64, Vec<(PathBuf, fs::Metadata)>> = HashMap::new();
    let mut seen_inode: HashSet<(u64, u64)> = HashSet::new();
    collect_files(
        root,
        opts,
        &mut seen_inode,
        &mut by_size,
        &mut scan,
        started,
    );
    scan.files_seen = seen_inode.len() as u64;
    on_tick(&DuplicateTick {
        phase: DuplicatePhase::Walking,
        done: scan.files_seen as usize,
        total: scan.files_seen as usize,
        files_seen: scan.files_seen,
    });

    // 第二阶段：同大小组内采样匹配。
    //
    // 超时在三个阶段都要查：budget 原本只保护遍历，而真正吃时间的是后两个
    // 阶段（家目录里大量同大小文件要逐个采样、逐个全量哈希）。只守遍历时，
    // 界面会永远停在「正在查找重复文件…」上——本机实测 3 分钟仍未结束。
    let mut by_sample: HashMap<Vec<u8>, Vec<(PathBuf, fs::Metadata)>> = HashMap::new();
    // 需要比对的候选总数：只统计「同大小且有同伴」的文件，
    // 界面上的进度分母用这个，而不是全部文件——后者会让进度看起来永远不动。
    let sample_total: usize = by_size
        .values()
        .filter(|v| v.len() > 1)
        .map(|v| v.len())
        .sum();
    let mut sample_done = 0usize;
    for (size, files) in by_size {
        if scan.truncated {
            break;
        }
        if files.len() < 2 {
            continue;
        }
        for (path, meta) in files {
            if over_budget(opts, started, &mut scan) {
                break;
            }
            match sample_head_tail(&path, size) {
                Some(sample) => {
                    by_sample.entry(sample).or_default().push((path, meta));
                }
                None => scan.skipped += 1,
            }
            sample_done += 1;
            on_tick(&DuplicateTick {
                phase: DuplicatePhase::Sampling,
                done: sample_done,
                total: sample_total,
                files_seen: scan.files_seen,
            });
        }
    }

    // 第三阶段：完整 SHA-256。
    let mut by_hash: HashMap<[u8; 32], Vec<(PathBuf, fs::Metadata)>> = HashMap::new();
    let hash_total: usize = by_sample
        .values()
        .filter(|v| v.len() > 1)
        .map(|v| v.len())
        .sum();
    let mut hash_done = 0usize;
    for files in by_sample.into_values() {
        if scan.truncated {
            break;
        }
        if files.len() < 2 {
            continue;
        }
        for (path, meta) in files {
            if over_budget(opts, started, &mut scan) {
                break;
            }
            match sha256_file(&path, &meta) {
                Some(hash) => {
                    by_hash.entry(hash).or_default().push((path, meta));
                }
                None => scan.skipped += 1,
            }
            hash_done += 1;
            on_tick(&DuplicateTick {
                phase: DuplicatePhase::Hashing,
                done: hash_done,
                total: hash_total,
                files_seen: scan.files_seen,
            });
        }
    }

    for files in by_hash.into_values() {
        if files.len() < 2 {
            continue;
        }
        let bytes = files[0].1.len();
        let files = files
            .into_iter()
            .map(|(path, meta)| DuplicateFile {
                path,
                bytes,
                modified: Some(meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
            })
            .collect();
        scan.groups.push(DuplicateGroup { files, bytes });
    }
    scan.groups.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    scan
}

/// 时间预算到了就置 `truncated`，返回 true 让调用方收手。
///
/// 收手不是失败：已经找到的分组仍然有效，只是后面的候选没比完——
/// 这必须能和「完全没重复」分开，所以 `truncated` 是结果里的一等公民。
fn over_budget(opts: &WalkOptions, started: std::time::Instant, scan: &mut DuplicateScan) -> bool {
    match opts.budget {
        Some(b) if !scan.truncated && started.elapsed() > b => {
            scan.truncated = true;
            true
        }
        _ => scan.truncated,
    }
}

/// 遍历收集普通文件。遵循 `WalkOptions` 的 skip / budget，不跟随符号链接。
fn collect_files(
    path: &Path,
    opts: &WalkOptions,
    seen_inode: &mut HashSet<(u64, u64)>,
    by_size: &mut HashMap<u64, Vec<(PathBuf, fs::Metadata)>>,
    scan: &mut DuplicateScan,
    started: std::time::Instant,
) {
    if scan.truncated {
        return;
    }
    if let Some(b) = opts.budget {
        if started.elapsed() > b {
            scan.truncated = true;
            return;
        }
    }
    if opts.skip.iter().any(|s| path.starts_with(s)) {
        return;
    }

    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => {
            scan.skipped += 1;
            return;
        }
    };
    if meta.file_type().is_symlink() {
        return; // 不跟随链接
    }
    if meta.is_file() {
        // 硬链接去重
        let key = (meta.dev(), meta.ino());
        if seen_inode.insert(key) {
            by_size
                .entry(meta.len())
                .or_default()
                .push((path.to_path_buf(), meta));
        }
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            scan.skipped += 1;
            return;
        }
    };
    for entry in entries.flatten() {
        collect_files(&entry.path(), opts, seen_inode, by_size, scan, started);
    }
}

/// 读文件的头尾采样。小文件整体读。
fn sample_head_tail(path: &Path, size: u64) -> Option<Vec<u8>> {
    let mut f = fs::File::open(path).ok()?;
    let total = size as usize;
    if total <= SAMPLE * 2 {
        let mut buf = vec![0u8; total];
        use std::io::Read;
        f.read_exact(&mut buf).ok()?;
        return Some(buf);
    }
    let mut head = vec![0u8; SAMPLE];
    let mut tail = vec![0u8; SAMPLE];
    use std::io::{Read, Seek, SeekFrom};
    f.read_exact(&mut head).ok()?;
    f.seek(SeekFrom::End(-(SAMPLE as i64))).ok()?;
    f.read_exact(&mut tail).ok()?;
    let mut sample = head;
    sample.extend_from_slice(&tail);
    Some(sample)
}

/// 流式计算完整 SHA-256。扫描前后 mtime 不一致说明文件在变，跳过。
fn sha256_file(path: &Path, meta: &fs::Metadata) -> Option<[u8; 32]> {
    let before = meta.modified().ok();
    let mut f = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; BUF];
    use std::io::Read;
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let after = fs::metadata(path).ok()?.modified().ok();
    if before != after {
        return None;
    }
    Some(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-dup-{}-{}-{}",
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

    fn write(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = root.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn Identical_files_are_grouped() {
        let d = tmpdir("identical");
        write(&d, "a.bin", &vec![7u8; 200_000]);
        write(&d, "b.bin", &vec![7u8; 200_000]);
        write(&d, "c.bin", &vec![7u8; 200_000]);
        let scan = find_duplicates(&d, &WalkOptions::default());
        assert_eq!(scan.groups.len(), 1);
        assert_eq!(scan.groups[0].files.len(), 3);
        assert_eq!(scan.groups[0].bytes, 200_000);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn same_size_different_content_is_not_a_duplicate() {
        let d = tmpdir("diff");
        write(&d, "a.bin", &vec![1u8; 200_000]);
        write(&d, "b.bin", &vec![2u8; 200_000]);
        let scan = find_duplicates(&d, &WalkOptions::default());
        assert!(scan.groups.is_empty());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn same_head_tail_but_different_middle_is_not_a_duplicate() {
        let d = tmpdir("middle");
        let mut a = vec![0u8; 128 * 1024];
        let mut b = vec![0u8; 128 * 1024];
        a[60_000] = 1;
        b[60_000] = 2;
        write(&d, "a.bin", &a);
        write(&d, "b.bin", &b);
        let scan = find_duplicates(&d, &WalkOptions::default());
        assert!(scan.groups.is_empty(), "中间不同不能被采样放过");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn different_sizes_never_group() {
        let d = tmpdir("sizes");
        write(&d, "a.bin", &vec![7u8; 1000]);
        write(&d, "b.bin", &vec![7u8; 1001]);
        let scan = find_duplicates(&d, &WalkOptions::default());
        assert!(scan.groups.is_empty());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn hard_links_are_not_duplicates() {
        let d = tmpdir("hardlink");
        let a = write(&d, "a.bin", &vec![3u8; 5000]);
        let b = d.join("b.bin");
        fs::hard_link(&a, &b).unwrap();
        let scan = find_duplicates(&d, &WalkOptions::default());
        assert!(scan.groups.is_empty(), "硬链接同一数据块不算重复");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn symlinks_are_ignored() {
        let d = tmpdir("symlink");
        let target = tmpdir("symlink-target");
        write(&target, "real.bin", &vec![9u8; 60_000]);
        std::os::unix::fs::symlink(&target, d.join("link")).unwrap();
        let scan = find_duplicates(&d, &WalkOptions::default());
        assert!(scan.groups.is_empty(), "不跟随符号链接");
        fs::remove_dir_all(&d).ok();
        fs::remove_dir_all(&target).ok();
    }

    #[test]
    fn small_files_are_compared_whole() {
        let d = tmpdir("small");
        write(&d, "a.txt", b"same-content");
        write(&d, "b.txt", b"same-content");
        let scan = find_duplicates(&d, &WalkOptions::default());
        assert_eq!(scan.groups.len(), 1);
        assert_eq!(scan.groups[0].files.len(), 2);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unreadable_files_are_skipped_not_fatal() {
        let d = tmpdir("unreadable");
        write(&d, "a.bin", &vec![4u8; 10_000]);
        write(&d, "b.bin", &vec![4u8; 10_000]);
        let hidden = d.join("locked.bin");
        fs::write(&hidden, vec![4u8; 10_000]).unwrap();
        // 不可读：去掉所有权限位
        let mut perms = fs::metadata(&hidden).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o000);
        fs::set_permissions(&hidden, perms).unwrap();

        let scan = find_duplicates(&d, &WalkOptions::default());
        // a/b 仍应组成一组，locked 被跳过
        assert_eq!(scan.groups.len(), 1);
        assert_eq!(scan.groups[0].files.len(), 2);
        assert!(scan.skipped >= 1);
        fs::set_permissions(&hidden, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_dir_all(&d).ok();
    }
}
