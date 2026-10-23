//! 内容哈希：用于还原前校验「放回去的还是原来那些字节」。
//!
//! 只对大目录做目录级哈希是不够的（无法发现部分丢失），
//! 但对几百 MB 的构建缓存逐文件哈希又太慢。折中：**只对文件做完整哈希，
//! 目录用「文件清单 + 各自大小」的组合哈希**。这样能发现绝大多数损坏
//! （文件丢失、被截断、被替换），代价是可控的。

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// 单文件哈希的分块大小。8 MB 足够快，也不会一次吃掉太多内存。
const CHUNK: usize = 8 * 1024 * 1024;

/// 算一个文件的 sha256。读失败时返回 None——调用方据此把这一项判为不可校验。
pub fn file_sha256(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex(&hasher.finalize()))
}

/// 算一个路径的哈希：文件算内容，目录算「相对路径 + 大小」的清单。
///
/// 这是「成本可接受」与「能发现损坏」之间有意选的中间点：
/// 它发现不了「同大小的内容被替换」，但那种情况在清理工具的场景里几乎不存在。
pub fn path_sha256(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_file() {
        return file_sha256(path);
    }
    if !meta.is_dir() {
        return Some(format!(
            "nondir:{}:{}",
            meta.len(),
            meta.file_type().is_symlink()
        ));
    }

    let mut entries: Vec<(String, u64)> = Vec::new();
    collect_entries(path, path, &mut entries);
    entries.sort();

    let mut hasher = Sha256::new();
    for (rel, size) in entries {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        hasher.update(size.to_le_bytes());
        hasher.update(b"\n");
    }
    Some(hex(&hasher.finalize()))
}

fn collect_entries(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            collect_entries(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.to_string_lossy().to_string());
            out.push((rel, meta.len()));
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-hash-{}-{}-{}",
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
    fn same_content_same_hash() {
        let d = tmpdir("same");
        let a = d.join("a");
        let b = d.join("b");
        fs::write(&a, b"hello").unwrap();
        fs::write(&b, b"hello").unwrap();
        assert_eq!(file_sha256(&a), file_sha256(&b));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn different_content_different_hash() {
        let d = tmpdir("diff");
        let a = d.join("a");
        let b = d.join("b");
        fs::write(&a, b"hello").unwrap();
        fs::write(&b, b"hellp").unwrap();
        assert_ne!(file_sha256(&a), file_sha256(&b));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn directory_hash_changes_when_a_file_disappears() {
        let d = tmpdir("dirloss");
        fs::write(d.join("a.txt"), b"aaaa").unwrap();
        fs::write(d.join("b.txt"), b"bbbb").unwrap();
        let before = path_sha256(&d).unwrap();

        fs::remove_file(d.join("b.txt")).unwrap();
        let after = path_sha256(&d).unwrap();

        assert_ne!(before, after, "文件丢失必须导致哈希变化");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn directory_hash_is_stable_for_unchanged_content() {
        let d = tmpdir("stable");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("sub/a.txt"), b"x").unwrap();
        assert_eq!(path_sha256(&d), path_sha256(&d));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn directory_hash_ignores_traversal_order() {
        let d1 = tmpdir("order1");
        let d2 = tmpdir("order2");
        fs::create_dir_all(d1.join("x")).unwrap();
        fs::create_dir_all(d2.join("x")).unwrap();
        fs::write(d1.join("a"), b"1").unwrap();
        fs::write(d1.join("x/b"), b"2").unwrap();
        // 换一个创建顺序
        fs::write(d2.join("x/b"), b"2").unwrap();
        fs::write(d2.join("a"), b"1").unwrap();
        assert_eq!(path_sha256(&d1), path_sha256(&d2));
        fs::remove_dir_all(&d1).ok();
        fs::remove_dir_all(&d2).ok();
    }

    #[test]
    fn missing_path_returns_none() {
        assert!(path_sha256(Path::new("/no/such/thing/xyz")).is_none());
    }
}
