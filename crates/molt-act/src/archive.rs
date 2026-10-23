//! 压缩归档：把「舍不得删、又很久没用」的内容打成 tar.gz 放到归档区。
//!
//! 与隔离区的区别只在于**存放形态**：隔离区是原样 `rename`，归档是打包成一个文件。
//! 对用户的意义是一样的——原位置腾出来了，内容还能拿回来。
//!
//! 两条必须守住的顺序：
//! 1. **先写包、再删原件**。反过来一旦压缩失败，原件已经没了，而包里什么都没有。
//! 2. **包写完立刻回读校验**，只有校验通过才删原件。tar 写入是流式的，
//!    磁盘满时 `finish()` 之前不会报错，回读是唯一能确认「包是完整的」的办法。
//!
//! 还有一个产品判断：压缩完如果没比原体积小多少（缓存类文本已经很紧凑、或本身是压缩格式），
//! 压缩就只是多花时间、还多一份解压步骤。这种情况**保留原件并如实报告**，
//! 而不是假装成功。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

/// 归档区。布局与隔离区一致：`<root>/<batch>/<id>-<name>.tar.gz`。
pub struct Archive {
    root: PathBuf,
    batch: String,
}

/// 打包结果。
#[derive(Debug, Clone)]
pub struct Packed {
    /// 包的位置
    pub path: PathBuf,
    /// 包的大小
    pub bytes: u64,
    /// 原内容的大小
    pub original_bytes: u64,
}

impl Packed {
    /// 压缩之后到底省了多少。负数代表包比原件还大。
    pub fn saved_bytes(&self) -> i64 {
        self.original_bytes as i64 - self.bytes as i64
    }
}

impl Archive {
    pub fn new(root: impl Into<PathBuf>, batch: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            batch: batch.into(),
        }
    }

    pub fn batch_dir(&self) -> PathBuf {
        self.root.join(&self.batch)
    }

    /// 打包一个路径。成功返回包的位置与体积；原路径**尚未**被删除——
    /// 删不删由调用方在拿到体积后决定（见 `purge_original`）。
    pub fn pack(&self, source: &Path, id: i64) -> std::io::Result<Packed> {
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "item".to_string());
        let dest = self.batch_dir().join(format!("{id}-{name}.tar.gz"));
        if dest.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("归档目标已存在：{}", dest.display()),
            ));
        }
        fs::create_dir_all(self.batch_dir())?;

        // 原内容的逻辑大小，用来判断压缩是否划算
        let original_bytes = measure(source);

        // 先把包写成临时文件：中途失败时不会在归档目录里留下半个包，
        // 而半个包在「还原」时看起来是存在的，只会解到一半才报错。
        let tmp = dest.with_extension("tar.gz.part");
        {
            let file = fs::File::create(&tmp)?;
            let enc = GzEncoder::new(file, Compression::default());
            let mut builder = tar::Builder::new(enc);

            // 以源自身为根写进包里（条目名带 basename），
            // 这样解压到「原位置的父目录」就能原样还原，不需要猜路径。
            let root_name = source.file_name().map(PathBuf::from).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("无法确定名称：{}", source.display()),
                )
            })?;
            builder.follow_symlinks(false);

            if source.is_dir() {
                builder.append_dir_all(&root_name, source)?;
            } else {
                let mut f = fs::File::open(source)?;
                let meta = f.metadata()?;
                let mut header = tar::Header::new_gnu();
                header.set_metadata(&meta);
                header.set_cksum();
                builder.append_data(&mut header, &root_name, &mut f)?;
            }

            let enc = builder
                .into_inner()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            enc.finish()?;
        }

        // 回读校验：能完整解开一遍才算包是好的
        if let Err(e) = verify(&tmp) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }

        fs::rename(&tmp, &dest)?;
        let bytes = fs::metadata(&dest)?.len();
        Ok(Packed {
            path: dest,
            bytes,
            original_bytes,
        })
    }

    /// 删掉已经被打包走的原件。只在包校验通过后由调用方调用。
    pub fn purge_original(source: &Path) -> std::io::Result<()> {
        let meta = fs::symlink_metadata(source)?;
        if meta.is_dir() {
            fs::remove_dir_all(source)
        } else {
            fs::remove_file(source)
        }
    }

    /// 清掉已经空的批次目录。
    pub fn tidy_batch(&self) {
        let dir = self.batch_dir();
        if let Ok(mut rd) = fs::read_dir(&dir) {
            if rd.next().is_none() {
                let _ = fs::remove_dir(&dir);
            }
        }
    }
}

/// 把包解回原位置。
///
/// 三条与隔离区一致的约束：原位置已存在就不覆盖、包不存在就报清楚、
/// 解压失败时不能留下半份内容——所以先解到临时目录，再整体搬过去。
pub fn unpack(archive: &Path, original: &Path) -> std::io::Result<()> {
    if original.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "原位置已存在同名项，未覆盖：{}。可先手动改名再还原。",
                original.display()
            ),
        ));
    }
    if !archive.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("归档文件已不在：{}", archive.display()),
        ));
    }

    let parent = original.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("原位置没有父目录：{}", original.display()),
        )
    })?;
    fs::create_dir_all(parent)?;

    // 先解到父目录下的临时目录：解到一半失败时，不会在原位置留下残缺内容。
    let staging = parent.join(format!(
        ".molt-restore-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&staging)?;

    let file = fs::File::open(archive)?;
    let mut dec = GzDecoder::new(file);
    let mut tar = tar::Archive::new(&mut dec);
    // `unpack` 会拒绝 `..` 与绝对路径的条目，不会写到 staging 之外。
    if let Err(e) = tar.unpack(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(std::io::Error::new(
            e.kind(),
            format!("解压失败，原位置未改动：{e}"),
        ));
    }

    // staging 里应当恰好有一个条目，名字是原 basename
    let restored = staging.join(
        original
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("item")),
    );
    if !restored.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("归档内容里找不到 {}，未还原", original.display()),
        ));
    }

    fs::rename(&restored, original)?;
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

/// 删掉归档文件（彻底清理）。
pub fn purge(archive: &Path) -> std::io::Result<()> {
    if archive.exists() {
        fs::remove_file(archive)
    } else {
        Ok(())
    }
}

/// 回读校验：完整走一遍解压，确认 CRC 与 tar 结构都对。
fn verify(path: &Path) -> std::io::Result<()> {
    let file = fs::File::open(path)?;
    let mut dec = GzDecoder::new(file);
    let mut tar = tar::Archive::new(&mut dec);
    let entries = tar
        .entries()
        .map_err(|e| std::io::Error::other(format!("归档无法读取：{e}")))?;

    let mut count = 0;
    let mut buf = [0u8; 8192];
    for entry in entries {
        let mut entry = entry.map_err(|e| std::io::Error::other(format!("归档条目损坏：{e}")))?;
        // 必须把每个条目的内容读完，否则 gzip 的 CRC 校验不会触发
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
        }
        count += 1;
    }
    if count == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "归档里没有任何内容",
        ));
    }
    Ok(())
}

/// 逻辑大小：目录下所有文件 `st_size` 之和。只用于判断压缩是否划算。
fn measure(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for entry in rd.flatten() {
            total += measure(&entry.path());
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-arch-{}-{}-{}",
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
    fn packs_a_directory_and_unpacks_it_back() {
        let base = tmpdir("roundtrip");
        let src = base.join("logs");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"hello").unwrap();
        fs::write(src.join("sub/b.txt"), b"world").unwrap();

        let a = Archive::new(base.join("archive"), "batch-1");
        let packed = a.pack(&src, 1).unwrap();
        assert!(packed.path.exists());
        assert_eq!(packed.original_bytes, 10);

        // 打完包，原件还在（删除由调用方决定）
        assert!(src.exists());
        Archive::purge_original(&src).unwrap();
        assert!(!src.exists());

        unpack(&packed.path, &src).unwrap();
        assert_eq!(fs::read(src.join("a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(src.join("sub/b.txt")).unwrap(), b"world");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn packs_a_single_file() {
        let base = tmpdir("file");
        let f = base.join("dump.sql");
        fs::write(&f, b"select 1;").unwrap();

        let a = Archive::new(base.join("archive"), "b");
        let packed = a.pack(&f, 7).unwrap();
        assert!(packed.path.to_string_lossy().ends_with("7-dump.sql.tar.gz"));

        Archive::purge_original(&f).unwrap();
        unpack(&packed.path, &f).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"select 1;");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn refuses_to_overwrite_on_restore() {
        let base = tmpdir("conflict");
        let src = base.join("dir");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("x"), b"1").unwrap();

        let a = Archive::new(base.join("archive"), "b");
        let packed = a.pack(&src, 1).unwrap();

        // 用户在这期间又生成了一份
        fs::write(src.join("new"), b"2").unwrap();
        let err = unpack(&packed.path, &src).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(src.join("new")).unwrap(), b"2", "新内容不能被覆盖");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn missing_archive_reports_the_path() {
        let base = tmpdir("missing");
        let target = base.join("x");
        let err = unpack(&base.join("nope.tar.gz"), &target).unwrap_err();
        assert!(err.to_string().contains("nope.tar.gz"));
    }

    #[test]
    fn corrupt_archive_leaves_the_original_untouched() {
        let base = tmpdir("corrupt");
        let bad = base.join("bad.tar.gz");
        fs::write(&bad, b"not a tarball").unwrap();
        let target = base.join("out");
        assert!(unpack(&bad, &target).is_err());
        assert!(!target.exists(), "失败时不能留下半份内容");

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn pack_rejects_an_existing_destination() {
        let base = tmpdir("dup");
        let src = base.join("a");
        fs::create_dir_all(&src).unwrap();
        let a = Archive::new(base.join("archive"), "b");
        a.pack(&src, 3).unwrap();
        let err = a.pack(&src, 3).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn verify_rejects_an_empty_archive() {
        let base = tmpdir("empty");
        let empty_dir = base.join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        let dest = base.join("e.tar.gz");
        let file = fs::File::create(&dest).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let builder = tar::Builder::new(enc);
        builder.into_inner().unwrap().finish().unwrap();

        assert!(verify(&dest).is_err(), "空包不能算打包成功");

        fs::remove_dir_all(&base).ok();
    }
}
