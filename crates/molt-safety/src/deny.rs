//! 硬禁区：**代码里写死、规则库无法覆盖**的危险路径。
//!
//! 为什么不能只靠规则库：规则库是可配置的，可配置就意味着可写错、可被覆盖、
//! 可被用户在不知情的情况下改坏。硬禁区的作用是保证「即使所有规则都写错了，
//! 这几类东西也动不了」。它排在规则匹配之后运行，所以只会让判定更保守。
//!
//! 这里的每一条都对应一次真实风险，注释里写了为什么。

pub struct DenyRule {
    /// 规则名，会出现在 `ProtectReason::HardDeny` 里，供用户和审计追溯
    pub name: &'static str,
    /// 相对家目录的路径模式；`~` 由 `matchers()` 展开
    pub patterns: &'static [&'static str],
    /// 为什么不能碰——这句话会显示给用户
    pub why: &'static str,
}

/// 全部硬禁区。顺序无所谓，任一命中即受保护。
pub const DENY_RULES: &[DenyRule] = &[
    DenyRule {
        name: "keychains",
        patterns: &["~/Library/Keychains/**"],
        why: "系统钥匙串",
    },
    DenyRule {
        name: "ssh-keys",
        patterns: &["~/.ssh/**"],
        why: "SSH 私钥",
    },
    DenyRule {
        name: "gpg-keys",
        patterns: &["~/.gnupg/**"],
        why: "GPG 密钥",
    },
    DenyRule {
        name: "env-files",
        patterns: &["**/.env", "**/.env.*"],
        why: "环境变量文件，通常含密钥",
    },
    DenyRule {
        name: "secret-dirs",
        patterns: &["**/.secrets/**"],
        why: "密钥目录",
    },
    DenyRule {
        name: "private-keys",
        patterns: &["**/*.pem", "**/*.key", "**/*.p12", "**/*.pfx"],
        why: "证书或私钥文件",
    },
    DenyRule {
        name: "app-config",
        // 清理工具最容易误删的就是配置文件——它小、看起来没用、删了应用就起不来
        patterns: &["**/config.yaml", "**/config.yml"],
        why: "应用配置",
    },
    DenyRule {
        name: "sqlite-sidecar",
        // 单独删 -wal/-shm 会让 SQLite 库打不开，而且现场看起来「只删了个小文件」
        patterns: &[
            "**/*.sqlite-wal",
            "**/*.sqlite-shm",
            "**/*.db-wal",
            "**/*.db-shm",
        ],
        why: "SQLite 边车文件，单删会损坏数据库",
    },
    DenyRule {
        name: "system",
        patterns: &["/System/**", "/usr/**", "/bin/**", "/sbin/**"],
        why: "系统文件",
    },
    DenyRule {
        name: "trash",
        patterns: &["~/.Trash/**"],
        why: "废纸篓本身不清",
    },
    DenyRule {
        name: "time-machine",
        patterns: &["/Volumes/**/Backups.backupdb/**"],
        why: "Time Machine 备份",
    },
    DenyRule {
        name: "icloud",
        patterns: &["~/Library/Mobile Documents/**"],
        why: "iCloud Drive 内容",
    },
];

/// 硬禁区判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyHit {
    pub rule: &'static str,
    pub matched_pattern: String,
    pub why: &'static str,
}

/// 检查一个已经规范化过的绝对路径是否命中硬禁区。
///
/// `home` 由调用方传入而不是在这里读环境变量，是为了让这条判定可以被单测
/// 精确覆盖——测试里不该依赖跑测试那台机器的真实家目录。
pub fn check(path: &str, home: &str) -> Option<DenyHit> {
    let expanded: Vec<(&'static str, String, &'static str)> = DENY_RULES
        .iter()
        .flat_map(|r| {
            r.patterns
                .iter()
                .map(move |p| (r.name, expand(p, home), r.why))
        })
        .collect();

    for (name, pattern, why) in expanded {
        if molt_core::glob::glob_match(&pattern, path) {
            return Some(DenyHit {
                rule: name,
                matched_pattern: pattern,
                why,
            });
        }
    }
    None
}

/// 硬禁区的全部模式（已展开），给「扫描时直接跳过这些子树」用。
pub fn all_patterns(home: &str) -> Vec<(String, &'static str)> {
    DENY_RULES
        .iter()
        .flat_map(|r| r.patterns.iter().map(move |p| (expand(p, home), r.name)))
        .collect()
}

fn expand(pattern: &str, home: &str) -> String {
    if pattern == "~" {
        home.to_string()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        pattern.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn hit(path: &str) -> Option<DenyHit> {
        check(path, HOME)
    }

    #[test]
    fn ssh_private_key_is_denied() {
        let h = hit("/Users/me/.ssh/id_ed25519").expect("SSH 目录必须被拦");
        assert_eq!(h.rule, "ssh-keys");
    }

    #[test]
    fn app_config_is_denied_even_in_a_cache_looking_dir() {
        // cpa 里有 11 个 config.yaml.bak.*，名字像垃圾但含真 key
        let h = hit("/Users/me/project/cpa/CLIProxyAPI/config.yaml").expect("配置必须被拦");
        assert_eq!(h.rule, "app-config");
    }

    #[test]
    fn config_backup_is_not_denied_by_pattern_but_secrets_are() {
        // .bak 不是 .yaml 结尾，硬禁区不认它——它要靠指纹哨兵拦（见 fingerprint 模块）。
        // 这条测试把边界写死，避免以后有人误以为硬禁区覆盖了备份文件。
        assert!(hit("/Users/me/project/cpa/CLIProxyAPI/config.yaml.bak.20260923").is_none());
    }

    #[test]
    fn sqlite_sidecar_is_denied() {
        assert_eq!(
            hit("/Users/me/.codex/logs_2.sqlite-wal").unwrap().rule,
            "sqlite-sidecar"
        );
        assert_eq!(
            hit("/Users/me/.codex/logs_2.sqlite-shm").unwrap().rule,
            "sqlite-sidecar"
        );
    }

    #[test]
    fn sqlite_main_file_is_not_denied_by_this_rule() {
        // 主库文件本身可以被判定为「可清日志」，只是不能单独删边车文件
        assert!(hit("/Users/me/.codex/logs_2.sqlite").is_none());
    }

    #[test]
    fn env_files_are_denied_at_any_depth() {
        assert!(hit("/Users/me/project/x/.env").is_some());
        assert!(hit("/Users/me/project/x/.env.local").is_some());
    }

    #[test]
    fn normal_cache_is_not_denied() {
        assert!(hit("/Users/me/.cargo/registry/cache").is_none());
        assert!(hit("/Users/me/project/cpa/CLIProxyAPI/.gocache/0a").is_none());
    }

    #[test]
    fn system_paths_are_denied() {
        assert!(hit("/System/Library/CoreServices").is_some());
        assert!(hit("/usr/local/bin/x").is_some());
    }

    #[test]
    fn home_expansion_uses_the_passed_home_not_the_real_one() {
        assert!(hit("/Users/other/.ssh/id_rsa").is_none());
        assert!(check("/Users/other/.ssh/id_rsa", "/Users/other").is_some());
    }
}
