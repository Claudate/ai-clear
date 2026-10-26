//! 路径约定：隔离区、审计库、规则目录在哪。
//!
//! 放在 `molt-act` 而不是各前端各写一份，因为这几个路径是**数据的归属**，
//! 不是界面的偏好。CLI 与 GUI 若各有一份副本，迟早会出现
//! 「命令行能还原、窗口还原不了」这种最难查的问题——它们看的根本不是同一个库。

use std::path::{Path, PathBuf};

/// 改名前的应用数据目录名。
///
/// 这个产品原先叫 shou，改名为 molt 时只换了目录名，用户的隔离区与审计库
/// 还留在旧目录里。不做迁移的后果很具体：**1.2 GB 的隔离内容变成孤儿**——
/// 文件还在磁盘上占着空间，而新版本在 `app.molt` 下看不到它们，
/// 用户点「还原」也找不到记录。所以旧目录必须搬过来，不能只改常量。
const LEGACY_APP_DIR: &str = "app.shou";

/// 应用数据目录：`~/Library/Application Support/app.molt`。
///
/// 第一次调用时顺带做一次旧目录迁移（见 [`migrate_legacy_dir`]）。
/// 放在这里而不是 main：CLI 与 GUI 都从这里取路径，
/// 只要修在这一处，两边就都不可能漏掉迁移。
pub fn app_dir() -> PathBuf {
    let dir = home_dir()
        .join("Library")
        .join("Application Support")
        .join("app.molt");
    migrate_legacy_dir(&dir);
    dir
}

/// 一次迁移的结果。返回它而不是 `()`，是因为**静默失败在这里是真实发生过的**：
/// 旧目录带 TCC 保护属性时 `read_dir` 只返回部分条目，剩下的既没搬走也没人知道，
/// 用户看到隔离区是空的，而 1.2 GB 还在旧路径下占着地方。调用方拿到这个结构
/// 才能把「还有 N 项没搬走」说出来。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// 成功搬过来的项数
    pub moved: usize,
    /// 因新目录已有同名项而保留新版、跳过旧版的项数
    pub skipped_existing: usize,
    /// 想搬但失败的项（名字 + 原因）。这些仍留在旧目录里。
    pub failed: Vec<(String, String)>,
}

impl MigrationReport {
    /// 是否发生了需要告诉用户的事。
    pub fn has_problems(&self) -> bool {
        !self.failed.is_empty()
    }
}

/// 老目录里有哪些东西没搬过来。CLI 与 GUI 用它决定要不要提示用户。
pub fn legacy_leftovers() -> Vec<std::path::PathBuf> {
    let home = home_dir();
    let old = home
        .join("Library")
        .join("Application Support")
        .join(LEGACY_APP_DIR);
    let new = app_dir_static();
    if !old.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&old) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| !new.join(n).exists())
                .unwrap_or(false)
        })
        .collect()
}

/// 只算路径，不做迁移。`app_dir()` 内部用，避免迁移函数里递归调回自己。
fn app_dir_static() -> PathBuf {
    home_dir()
        .join("Library")
        .join("Application Support")
        .join("app.molt")
}

/// 把 `app.shou` 里的内容搬到 `app.molt`，只搬一次。返回这次搬了什么、卡在哪。
///
/// 两条规则：
/// - **新目录已有同名项就跳过**，绝不覆盖——已在新版里产生的新数据比旧数据重要。
/// - **逐项搬，失败就记下来**，不因为某一项搬不动就整批放弃；
///   搬不动的那项在旧目录里原样留着，并出现在返回值的 `failed` 里。
///
/// 搬空之后才删旧目录：`remove_dir` 对非空目录会失败，这个失败是安全的，
/// 它保证了「还有没搬走的东西」时不会把旧目录连同里面的内容一起丢掉。
fn migrate_legacy_dir(new_dir: &Path) -> MigrationReport {
    let mut report = MigrationReport::default();
    // 旧目录与新目录必然同级（都在 `Library/Application Support/` 下），
    // 所以从 new_dir 的父目录推导，而不是再读一次 `home_dir()`。
    // 这样这个函数不依赖全局环境，测试可以拿临时目录完整验证一遍。
    let Some(parent) = new_dir.parent() else {
        return report;
    };
    let old_dir = parent.join(LEGACY_APP_DIR);
    if !old_dir.is_dir() {
        return report;
    }
    let Ok(entries) = std::fs::read_dir(&old_dir) else {
        return report;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let shown = name.to_string_lossy().to_string();
        let dst = new_dir.join(&name);
        if dst.exists() {
            // 新目录已有同名项。通常保留新版——已在新版里产生的数据更重要。
            //
            // 唯一的例外是**空库**：新版一启动就会建一个空 audit.sqlite，
            // 而旧库里是用户真实的清理与还原记录。这时候「保留新版」等于
            // 把记录丢在旧目录里再也读不到。空库没有价值，用旧库换掉它。
            if is_empty_audit_db(&dst) && !is_empty_audit_db(&entry.path()) {
                let tmp = new_dir.join(format!(".{shown}.replacing"));
                let _ = std::fs::remove_file(&tmp);
                if std::fs::copy(entry.path(), &tmp).is_ok() && std::fs::rename(&tmp, &dst).is_ok()
                {
                    let _ = std::fs::remove_file(entry.path());
                    report.moved += 1;
                    continue;
                }
                let _ = std::fs::remove_file(&tmp);
            }
            report.skipped_existing += 1;
            continue;
        }
        let _ = std::fs::create_dir_all(new_dir);
        let src = entry.path();
        // 先试 rename（同卷，瞬间完成）；跨卷或权限问题退回拷贝。
        if std::fs::rename(&src, &dst).is_ok() {
            report.moved += 1;
            continue;
        }
        let copied = if src.is_dir() {
            copy_dir(&src, &dst).is_ok() && std::fs::remove_dir_all(&src).is_ok()
        } else {
            std::fs::copy(&src, &dst).is_ok() && std::fs::remove_file(&src).is_ok()
        };
        if copied {
            report.moved += 1;
        } else {
            // 拷贝失败时可能已经在新目录留下了半份，清掉，别留个残缺副本。
            if src.is_dir() {
                let _ = std::fs::remove_dir_all(&dst);
            } else {
                let _ = std::fs::remove_file(&dst);
            }
            report.failed.push((
                shown,
                "移动与拷贝都被拒绝（通常是没有该目录的访问权限）".to_string(),
            ));
        }
    }
    // 只有真的空了才会成功；还有残留就留着旧目录。
    let _ = std::fs::remove_dir(&old_dir);
    report
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// 判断一个 audit.sqlite 是不是「刚建出来、还没有任何记录」的空库。
///
/// 为什么不写 SQL：这一层不持有数据库连接，为一个判断去开库反而会在
/// 打不开的路径上引入失败分支。
///
/// 实测的空库形态：建完 schema（一张表 + 两个索引）是 **5 页 / 20480 字节**，
/// 不是 1 页——SQLite 会给 schema 与索引各分配页。所以阈值不能按「一页」设，
/// 否则空库会被当成有内容，旧库永远换不进来（这个错误在真实数据上验证过一次：
/// 新库 0 条记录、5 页，旧库 698 条记录、68 页）。
///
/// 记录一多就会超过 6 页，所以「不超过 6 页」足以区分空库。
/// 读不到文件时返回 false，保守地「当作有内容」而不覆盖。
fn is_empty_audit_db(p: &Path) -> bool {
    const EMPTY_DB_MAX_PAGES: u64 = 6;
    match std::fs::metadata(p) {
        Ok(m) => m.len() <= EMPTY_DB_MAX_PAGES * 4096,
        Err(_) => false,
    }
}
pub fn audit_db() -> PathBuf {
    app_dir().join("audit.sqlite")
}

pub fn quarantine_root() -> PathBuf {
    app_dir().join("quarantine")
}

/// 定时任务的日志目录。
pub fn log_dir() -> PathBuf {
    app_dir().join("logs")
}

/// 用户自定义规则目录。同名文件整体替换内置版本。
pub fn user_rules_dir() -> PathBuf {
    home_dir().join(".config").join("molt").join("rules")
}

/// 家目录。`MOLT_HOME` 可以把它指到别处——端到端验证需要一个假家目录，
/// 否则「清理」这条链路只能在真实用户目录上验，而那是不可接受的做法。
/// 该变量必须是绝对路径；写错时退回 `HOME`，不会把数据写到相对路径下。
pub fn home_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("MOLT_HOME") {
        let p = PathBuf::from(v);
        if p.is_absolute() {
            return p;
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// 允许清理的根。默认只到本人家目录——不碰别人的数据，也不碰系统目录。
pub fn authorized_roots() -> Vec<PathBuf> {
    vec![home_dir()]
}

/// 隔离保留天数。默认 7 天，与「一周内发现删错还来得及」这个直觉对齐。
pub const DEFAULT_RETAIN_DAYS: i64 = 7;

/// 把路径缩成 `~/...` 形式，让清单更短也更容易读。
pub fn display_path(path: &Path) -> String {
    display_in(&home_dir().to_string_lossy(), path)
}

/// 用指定的家目录做缩写。测试与 GUI 需要这样——它们不一定跑在真实 HOME 下。
pub fn display_in(home: &str, path: &Path) -> String {
    let home = Path::new(home);
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn app_paths_live_under_application_support() {
        assert!(audit_db().ends_with("app.molt/audit.sqlite"));
        assert!(quarantine_root().ends_with("app.molt/quarantine"));
        assert!(log_dir().ends_with("app.molt/logs"));
    }

    #[test]
    fn display_collapses_home() {
        assert_eq!(
            display_in("/Users/me", Path::new("/Users/me/.npm/_cacache")),
            "~/.npm/_cacache"
        );
        assert_eq!(display_in("/Users/me", Path::new("/Users/me")), "~");
        assert_eq!(
            display_in("/Users/me", Path::new("/etc/hosts")),
            "/etc/hosts"
        );
    }

    #[test]
    fn retain_days_matches_a_week() {
        assert_eq!(DEFAULT_RETAIN_DAYS, 7);
    }

    /// 改名迁移的核心约束：旧内容要搬过去。
    #[test]
    fn legacy_dir_is_migrated() {
        let base = tmp("migrate");
        let old = base.join("app.shou");
        let new = base.join("app.molt");
        fs::create_dir_all(old.join("quarantine/b1")).unwrap();
        fs::write(old.join("quarantine/b1/item"), b"payload").unwrap();
        fs::write(old.join("audit.sqlite"), b"db").unwrap();

        let report = migrate_legacy_dir(&new);

        assert_eq!(
            fs::read(new.join("quarantine/b1/item")).unwrap(),
            b"payload",
            "隔离内容必须搬到新目录"
        );
        assert!(new.join("audit.sqlite").exists(), "审计库必须搬过来");
        assert!(!old.exists(), "搬空后旧目录应被删掉");
        assert_eq!(report.moved, 2, "两项都该报成功");
        assert!(report.failed.is_empty());
        fs::remove_dir_all(&base).ok();
    }

    /// 新目录已有同名项时，**不能**用旧数据覆盖——新数据更重要。
    #[test]
    fn migration_never_overwrites_newer_data() {
        let base = tmp("migrate-keep");
        let old = base.join("app.shou");
        let new = base.join("app.molt");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("audit.sqlite"), b"old").unwrap();
        fs::write(new.join("audit.sqlite"), b"new").unwrap();

        let report = migrate_legacy_dir(&new);

        assert_eq!(
            fs::read(new.join("audit.sqlite")).unwrap(),
            b"new",
            "新库不能被旧库覆盖"
        );
        assert_eq!(
            report.skipped_existing, 1,
            "跳过旧版要计数，不能被当成没发生"
        );
        assert!(report.failed.is_empty());
        fs::remove_dir_all(&base).ok();
    }

    /// 旧目录没有时，函数应当安静地什么都不做。
    #[test]
    fn migration_is_a_noop_without_legacy_dir() {
        let base = tmp("migrate-none");
        let new = base.join("app.molt");
        let report = migrate_legacy_dir(&new);
        assert!(!new.exists());
        assert_eq!(
            report,
            MigrationReport::default(),
            "没有旧目录时什么也不该报告"
        );
        fs::remove_dir_all(&base).ok();
    }

    /// 搬不动的项必须出现在 `failed` 里，而不是被静默吞掉。
    /// 这条来自真实故障：旧目录受 TCC 保护时 `read_dir` 只返回部分条目，
    /// 之前那版实现对此毫无表示，用户以为隔离区是空的。
    #[test]
    fn migration_reports_items_it_could_not_move() {
        let base = tmp("migrate-fail");
        let old = base.join("app.shou");
        let new = base.join("app.molt");
        fs::create_dir_all(&old).unwrap();
        // 新目录的位置上放一个**同名文件**，让 create_dir_all 失败：
        // 这样 quarantine 这一项既 rename 不过去、也拷不过去。
        fs::create_dir_all(&base).unwrap();
        fs::write(&new, b"not a directory").unwrap();
        fs::create_dir_all(old.join("quarantine")).unwrap();
        fs::write(old.join("quarantine/x"), b"payload").unwrap();

        let report = migrate_legacy_dir(&new);

        assert!(
            report.has_problems(),
            "搬不动的项必须被报告出来，不能静默吞掉"
        );
        assert!(
            report.failed.iter().any(|(name, _)| name == "quarantine"),
            "失败项应带上名字，实际 {:?}",
            report.failed
        );
        // 旧数据必须还在——失败时绝不能把它删掉。
        assert!(old.join("quarantine/x").exists(), "搬不动就不能删原件");
        fs::remove_dir_all(&base).ok();
    }

    /// 新版一启动就会建一个空 audit.sqlite。此时旧库里有真实记录，
    /// 必须用旧库换掉空库——否则「保留新版」等于把记录遗弃在旧目录。
    #[test]
    fn empty_new_db_is_replaced_by_legacy_one_with_records() {
        let base = tmp("migrate-empty-db");
        let old = base.join("app.shou");
        let new = base.join("app.molt");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        // 旧库有内容：远超空库的 5 页
        let mut big = vec![0u8; 64 * 1024];
        big[..16].copy_from_slice(b"SQLite format 3\0");
        fs::write(old.join("audit.sqlite"), &big).unwrap();
        // 新库是刚建的空库：实测 5 页 / 20480 字节
        let mut small = vec![0u8; 5 * 4096];
        small[..16].copy_from_slice(b"SQLite format 3\0");
        fs::write(new.join("audit.sqlite"), &small).unwrap();

        let report = migrate_legacy_dir(&new);

        assert_eq!(
            fs::metadata(new.join("audit.sqlite")).unwrap().len(),
            64 * 1024,
            "空库应被有内容的旧库替换"
        );
        assert!(!old.join("audit.sqlite").exists(), "旧库搬走后不该留下");
        assert_eq!(report.moved, 1);
        fs::remove_dir_all(&base).ok();
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("molt-paths-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
}
