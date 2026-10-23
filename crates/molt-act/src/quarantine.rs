//! 隔离区：清理的默认动作是「移走」，不是「删除」。
//!
//! 这个设计来自 cpa 里三次真实清理的共同点——没有一次是 `rm -rf`，
//! 全是「先取证 → 归档 → 留回滚路径」。工具要把这个习惯变成默认行为。
//!
//! 为什么不直接扔 `~/.Trash`：废纸篓对用户可见、会被其它清理工具二次处理、
//! `empty trash` 不可逆，而且不保留「从哪来的」这条信息。

use std::path::{Path, PathBuf};

/// 给 `rename` 一类系统调用的失败补上上下文：操作名、源、目标、原始 errno。
///
/// 光有一句 `Operation not permitted (os error 1)` 是没法排查的——
/// 真正被卡住的可能是源、目标、或目标父目录，而调用方当时看不到任何一条路径。
/// 一次 216 项的还原就因为这个卡住过：报错没说是哪一步，只能逐条手工试。
fn describe(op: &str, from: &Path, to: &Path, e: std::io::Error) -> std::io::Error {
    let raw = e
        .raw_os_error()
        .map(|n| format!("，errno {n}"))
        .unwrap_or_default();
    std::io::Error::new(
        e.kind(),
        format!(
            "{op}失败{raw}：{} -> {}（{e}）",
            from.display(),
            to.display()
        ),
    )
}

/// 隔离区布局：
/// ```text
/// <root>/2026-10-20T18-10-00/<id>-<basename>
/// ```
/// 时间戳目录让「一次清理」在文件系统层面也是可辨识的一组，
/// 即使审计库丢了，用户还能靠目录名把东西找回来。
pub struct Quarantine {
    root: PathBuf,
    batch: String,
}

impl Quarantine {
    /// `root` 通常为 `~/Library/Application Support/app.molt/quarantine`。
    pub fn new(root: impl Into<PathBuf>, batch: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            batch: batch.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn batch_dir(&self) -> PathBuf {
        self.root.join(&self.batch)
    }

    /// 把一个路径移进隔离区，返回它在隔离区里的新位置。
    ///
    /// 用 `rename`：同卷时是原子操作、瞬时完成，不会出现「复制到一半磁盘满」
    /// 导致原文件已毁、副本还没写成的情况。
    /// 跨卷时 `rename` 会失败——这时**不降级成复制+删除**，直接报错，
    /// 因为那会在磁盘将满时产生最坏结果。
    pub fn move_in(&self, source: &Path, id: i64) -> std::io::Result<PathBuf> {
        let file_name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "item".to_string());
        let dest = self.batch_dir().join(format!("{id}-{file_name}"));

        std::fs::create_dir_all(self.batch_dir())?;

        // 目标已存在说明 id 撞了，不能覆盖——覆盖等于静默丢一份数据
        if dest.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("隔离区目标已存在：{}", dest.display()),
            ));
        }

        std::fs::rename(source, &dest).map_err(|e| describe("移入隔离区", source, &dest, e))?;
        Ok(dest)
    }

    /// 把隔离区里的东西放回原位置。
    ///
    /// 原位置已被占用时**不覆盖**：那是用户后来又重新生成了一份，
    /// 覆盖它等于用旧数据盖掉新数据，比不还原更糟。
    pub fn move_out(&self, quarantine_path: &Path, original: &Path) -> std::io::Result<()> {
        if original.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "原位置已存在同名项，未覆盖：{}。可先手动改名再还原。",
                    original.display()
                ),
            ));
        }
        if let Some(parent) = original.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| describe("建还原目录", parent, parent, e))?;
        }
        std::fs::rename(quarantine_path, original)
            .map_err(|e| describe("还原", quarantine_path, original, e))
    }

    /// 隔离期满，真正删除。
    pub fn purge(&self, quarantine_path: &Path) -> std::io::Result<()> {
        let meta = std::fs::symlink_metadata(quarantine_path)?;
        if meta.is_dir() {
            std::fs::remove_dir_all(quarantine_path)
        } else {
            std::fs::remove_file(quarantine_path)
        }
    }

    /// 清掉已经空的批次目录。删完东西留下一个空壳目录会让用户困惑。
    pub fn tidy_batch(&self) {
        let dir = self.batch_dir();
        if let Ok(mut rd) = std::fs::read_dir(&dir) {
            if rd.next().is_none() {
                let _ = std::fs::remove_dir(&dir);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-quar-{}-{}-{}",
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
    fn moves_a_directory_into_the_batch() {
        let base = tmpdir("move");
        let src = base.join("cache");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x.bin"), b"data").unwrap();

        let q = Quarantine::new(base.join("quarantine"), "batch-1");
        let dest = q.move_in(&src, 7).unwrap();

        assert!(!src.exists(), "原位置应当已空");
        assert!(dest.exists());
        assert!(dest.to_string_lossy().contains("7-cache"));
        assert_eq!(fs::read(dest.join("x.bin")).unwrap(), b"data");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn refuses_to_overwrite_an_existing_quarantine_entry() {
        let base = tmpdir("dup");
        let src = base.join("a");
        fs::create_dir_all(&src).unwrap();
        let q = Quarantine::new(base.join("q"), "b");

        q.move_in(&src, 1).unwrap();
        // 再造一个同名同 id 的，应当被拒
        fs::create_dir_all(&src).unwrap();
        let err = q.move_in(&src, 1).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restores_back_to_the_original_path() {
        let base = tmpdir("restore");
        let src = base.join("deep/nested/cache");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("f.txt"), b"content").unwrap();

        let q = Quarantine::new(base.join("q"), "b");
        let qp = q.move_in(&src, 3).unwrap();
        assert!(!src.exists());

        q.move_out(&qp, &src).unwrap();
        assert_eq!(fs::read(src.join("f.txt")).unwrap(), b"content");
        assert!(!qp.exists());

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn refuses_to_restore_over_a_newer_file() {
        let base = tmpdir("conflict");
        let src = base.join("cfg");
        fs::create_dir_all(&src).unwrap();

        let q = Quarantine::new(base.join("q"), "b");
        let qp = q.move_in(&src, 5).unwrap();

        // 用户在这期间又生成了一份
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("new.txt"), b"newer").unwrap();

        let err = q.move_out(&qp, &src).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        // 新的那份必须原封不动
        assert_eq!(fs::read(src.join("new.txt")).unwrap(), b"newer");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn purge_removes_the_item() {
        let base = tmpdir("purge");
        let src = base.join("x");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a"), b"1").unwrap();

        let q = Quarantine::new(base.join("q"), "b");
        let qp = q.move_in(&src, 9).unwrap();
        q.purge(&qp).unwrap();
        assert!(!qp.exists());

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn restore_error_names_both_paths() {
        let base = tmpdir("errctx");
        let src = base.join("cache");
        fs::create_dir_all(&src).unwrap();

        let q = Quarantine::new(base.join("q"), "b");
        let qp = q.move_in(&src, 4).unwrap();

        // 用户在这期间又生成了一份，还原被拒
        fs::create_dir_all(&src).unwrap();
        let msg = q.move_out(&qp, &src).unwrap_err().to_string();

        // 报错必须能直接读出「卡在哪条路径」，否则用户只能逐条手工试
        assert!(msg.contains(&src.display().to_string()), "缺原位置：{msg}");
        assert!(msg.contains("未覆盖"), "缺处理建议：{msg}");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn failing_rename_reports_operation_and_paths() {
        let base = tmpdir("renameerr");
        let missing = base.join("not-there");
        let dest = base.join("dest");
        let q = Quarantine::new(base.join("q"), "b");

        let msg = q.move_out(&missing, &dest).unwrap_err().to_string();
        // 源不存在时必须说清是「还原」这一步、且两条路径都在
        assert!(msg.contains("还原失败"), "缺操作名：{msg}");
        assert!(msg.contains(&missing.display().to_string()), "缺源：{msg}");
        assert!(msg.contains(&dest.display().to_string()), "缺目标：{msg}");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn tidy_removes_empty_batch_dir_only() {
        let base = tmpdir("tidy");
        let q = Quarantine::new(base.join("q"), "b");
        fs::create_dir_all(q.batch_dir()).unwrap();
        q.tidy_batch();
        assert!(!q.batch_dir().exists());

        // 非空时不能删
        fs::create_dir_all(q.batch_dir()).unwrap();
        fs::write(q.batch_dir().join("x"), b"1").unwrap();
        q.tidy_batch();
        assert!(q.batch_dir().exists());

        fs::remove_dir_all(&base).ok();
    }
}
