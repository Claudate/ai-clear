//! 永久删除：**唯一会真正让磁盘空间变少的动作**。
//!
//! 隔离区用的是 `rename`，文件还在同一块盘上，所以「移入隔离区」后可用空间
//! 一个字节都不会变。用户要的「清理完空间真的多出来」只能靠这里。
//!
//! 因此这一层比隔离区更需要边界。隔离的边界是「可还原」，删除没有这一层兜底，
//! 只能在动手前把路径本身验死：
//!
//! - 必须是绝对路径，且不含 `..`（不靠字符串拼接去猜真实位置）
//! - 不能是 `/`、家目录本身、或应用自己的数据目录（审计库/隔离区）
//! - 必须落在授权根（家目录）之内
//!
//! 这些检查在 `remove` 内做，而不是指望调用方先验过——写盘层是最后一道闸，
//! 它不该假设上游一定算对了。

use std::path::{Component, Path, PathBuf};

/// 判断一个路径是否允许被永久删除（授权根取默认家目录）。
///
/// 返回 `Err` 时附带一句能直接显示给用户的原因。
pub fn validate(path: &Path) -> Result<(), String> {
    validate_within(path, &crate::paths::home_dir())
}

/// 同上，但显式指定授权根。
///
/// 拆出这一版是为了让删除边界可以被测试、也可以被将来的「只清某个项目目录」
/// 这类需求复用——把 `~` 写死在函数里，测试就只能去动真实的 `$HOME`，
/// 那既不可靠也不安全。
pub fn validate_within(path: &Path, home: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("不是绝对路径，拒绝删除：{}", path.display()));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("路径里含 `..`，拒绝删除：{}", path.display()));
    }

    // 组件数为 1 就是根目录（`/`），2 是 `/Users` 这类挂载点级别的目录
    let depth = path.components().count();
    if depth < 3 {
        return Err(format!("拒绝删除层级过浅的路径：{}", path.display()));
    }

    if !path.starts_with(home) {
        return Err(format!(
            "不在授权范围（家目录）内，拒绝删除：{}",
            path.display()
        ));
    }
    if path == home {
        return Err("拒绝删除家目录本身".to_string());
    }

    // 应用自己的数据目录里是审计库与隔离区，删掉它们等于把回滚能力一起清了
    let app = home.join("Library/Application Support/app.molt");
    if path.starts_with(&app) {
        return Err(format!(
            "这是「蜕」自己的数据目录，拒绝删除：{}",
            path.display()
        ));
    }

    Ok(())
}

/// 永久删除一个文件、目录或符号链接。
///
/// 符号链接只删链接本身，**不跟随**到目标——跟随会把删除范围扩到链接指向的地方，
/// 而那个位置可能根本不在授权根内。
pub fn remove(path: &Path) -> Result<(), String> {
    remove_within(path, &crate::paths::home_dir())
}

/// 同上，但显式指定授权根。
pub fn remove_within(path: &Path, home: &Path) -> Result<(), String> {
    validate_within(path, home)?;
    delete_tree(path)
}

/// 真正的删除动作，不做范围校验。
///
/// 与 `remove` 分开是为了让「校验」与「动手」各自能被单独测：
/// 校验的测试不该真去删东西，删除的测试也不该被 HOME 影响。
/// 外部调用方一律用 `remove`。
fn delete_tree(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;

    let result = if meta.file_type().is_symlink() || meta.is_file() {
        std::fs::remove_file(path)
    } else if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        return Err(format!("未知的文件类型，未删除：{}", path.display()));
    };

    result.map_err(|e| {
        let hint = match e.kind() {
            std::io::ErrorKind::PermissionDenied => "（没有权限。若是系统隐私保护目录，需在「系统设置 → 隐私与安全性 → 完全磁盘访问权限」里授权）",
            std::io::ErrorKind::DirectoryNotEmpty => "（目录里有文件正被其它进程写入）",
            _ => "",
        };
        format!("删除 {} 失败：{e}{hint}", path.display())
    })
}

/// 路径是否可能是「删了会把应用自己弄坏」的位置。给界面做提示用。
pub fn is_app_data(path: &Path) -> bool {
    path.starts_with(crate::paths::app_dir())
}

/// 把一组候选的路径规整成删除顺序：**先子后父**。
///
/// 如果用户同时勾了 `~/x` 和 `~/x/y`，先删父目录会让子项那一步必然报
/// 「不存在」——看起来像清理失败，其实只是顺序。按路径深度倒序排一次，
/// 子项先走，父项后走，两条都成功，统计也才对得上。
pub fn deepest_first(paths: &mut [PathBuf]) {
    paths.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "molt-del-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn rejects_relative_and_parent_paths() {
        assert!(validate(Path::new("foo/bar")).is_err());
        assert!(validate(Path::new("/Users/me/../etc")).is_err());
    }

    #[test]
    fn rejects_root_and_shallow_paths() {
        assert!(validate(Path::new("/")).is_err());
        assert!(validate(Path::new("/Users")).is_err());
    }

    #[test]
    fn rejects_paths_outside_home() {
        assert!(validate(Path::new("/etc/hosts")).is_err());
    }

    #[test]
    fn rejects_its_own_data_dir() {
        let app = crate::paths::app_dir();
        let err = validate(&app.join("quarantine")).unwrap_err();
        assert!(err.contains("自己的数据目录"), "实际：{err}");
    }

    #[test]
    fn removes_a_directory_tree_and_reports_real_deletion() {
        let base = tmpdir("tree");
        let cache = base.join("cache");
        std::fs::create_dir_all(cache.join("nested")).unwrap();
        std::fs::write(cache.join("nested/a.bin"), vec![0u8; 4096]).unwrap();

        // 直接测底层删除动作，不靠改 HOME —— 并行测试里改全局环境变量会互相干扰。
        delete_tree(&cache).unwrap();
        assert!(!cache.exists(), "目录应已被真正删除");
        assert!(base.exists(), "只应删掉目标，不能波及父目录");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn symlink_is_removed_without_touching_its_target() {
        let base = tmpdir("symlink");
        let target = tmpdir("symlink-target");
        std::fs::write(target.join("keep.txt"), b"keep").unwrap();
        std::os::unix::fs::symlink(&target, base.join("link")).unwrap();

        delete_tree(&base.join("link")).unwrap();
        assert!(!base.join("link").exists(), "链接应已删除");
        assert!(target.join("keep.txt").exists(), "目标绝不能被删");

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&target).ok();
    }

    #[test]
    fn deepest_first_puts_children_before_parents() {
        let mut v = vec![
            PathBuf::from("/Users/me/x"),
            PathBuf::from("/Users/me/x/y/z"),
            PathBuf::from("/Users/me/x/y"),
        ];
        deepest_first(&mut v);
        assert_eq!(v[0], PathBuf::from("/Users/me/x/y/z"));
        assert_eq!(v[2], PathBuf::from("/Users/me/x"));
    }
}
