//! 路径与配置：把「东西放哪」从业务逻辑里拿出去。
//!
//! **不要在这里另写一份路径。** 这些路径是数据的归属，不是前端的偏好：
//! 命令行与窗口若各有一份副本，迟早会出现「命令行能还原、窗口还原不了」——
//! 它们看的根本不是同一个库。
//!
//! 这里原先确实各写了一份，代价在改名时暴露了出来：`app.shou` → `app.molt`
//! 的迁移只加在 `molt-act` 里，命令行走自己那份，于是旧隔离区一直没被搬过来。
//! 现在统一转发到 `molt-act::paths`，迁移逻辑也就自动对两边都生效。

pub use molt_act::paths::{
    audit_db, display_path, home_dir, quarantine_root, user_rules_dir, DEFAULT_RETAIN_DAYS,
};

/// 应用数据目录。转发到 `molt-act`，顺带触发一次旧目录迁移。
pub fn app_dir() -> std::path::PathBuf {
    molt_act::paths::app_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn display_path_collapses_home() {
        let home = home_dir();
        let p = home.join(".cargo/registry");
        let s = display_path(&p);
        assert!(s.starts_with("~/"), "实际 {s}");
        assert!(!s.contains(&home.to_string_lossy().to_string()));
    }

    #[test]
    fn display_path_keeps_outside_paths() {
        assert_eq!(display_path(Path::new("/etc/hosts")), "/etc/hosts");
    }

    #[test]
    fn app_paths_live_under_application_support() {
        assert!(audit_db().ends_with("app.molt/audit.sqlite"));
        assert!(quarantine_root().ends_with("app.molt/quarantine"));
    }
}
