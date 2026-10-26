//! 路径转出：GUI 用的这几个路径都来自 `molt-act`。
//!
//! 这层转发不是多余的——它把「GUI 依赖哪几个路径」固定下来。
//! 如果直接让界面代码到处写 `molt_act::paths::xxx()`，将来挪一次目录
//! 就要改满界面；而这里只有一份清单。

pub use molt_act::paths::DEFAULT_RETAIN_DAYS;

pub fn audit_db() -> std::path::PathBuf {
    molt_act::paths::audit_db()
}

pub fn quarantine_root() -> std::path::PathBuf {
    molt_act::paths::quarantine_root()
}

pub fn home_dir() -> std::path::PathBuf {
    molt_act::paths::home_dir()
}

/// 把路径缩成 `~/...`。`home` 由调用方传入，这样界面显示的缩写
/// 与实际扫描用的根始终一致。
pub fn display_in(home: &str, path: &std::path::Path) -> String {
    molt_act::paths::display_in(home, path)
}

/// 现在这一刻，只取 `HH:MM`。
///
/// 界面上「这份结果是什么时候的」只需要时分：同一天内的先后关系足以判断
/// 新鲜度，写上年月日反而更长且没人读。
pub fn now_hhmm() -> String {
    let iso = molt_act::now_iso8601();
    if iso.len() >= 16 {
        iso[11..16].to_string()
    } else {
        iso
    }
}

/// 缩写一个字符串路径。用于后台任务回报的、已经没有 `home` 上下文的路径。
pub fn display_path_str(path: &str) -> String {
    molt_act::paths::display_path(std::path::Path::new(path))
}

/// 把一条路径翻成一句人话：「Cursor 的文件历史」。
///
/// 不懂电脑的用户读不懂 `~/Library/Application Support/Cursor/User/History`，
/// 但路径的末几段往往已经说明了它是什么。这里只做「认得出」的翻译，
/// 不猜语义——认不出来就退回末段目录名，绝不编一个听起来合理但可能错的名字。
pub fn friendly_name(home: &str, path: &std::path::Path) -> String {
    let shown = display_in(home, path);
    let lower = shown.to_lowercase();

    // 顺序有意义：越具体的先匹配。`Cursor/User/History` 必须落在第一条，
    // 否则会先命中泛的 `cursor`。
    const KNOWN: &[(&str, &str)] = &[
        ("cursor/user/history", "Cursor 的文件历史版本"),
        ("cursor/cacheddata", "Cursor 的代码缓存"),
        ("cursor/cache", "Cursor 的缓存"),
        ("cursor", "Cursor 的数据目录"),
        (".claude/projects", "Claude Code 的会话记录"),
        (".claude", "Claude Code 的本地数据"),
        (".codex", "Codex 的本地数据"),
        (".ollama/models", "本地 AI 模型文件"),
        (".ollama", "Ollama 的本地数据"),
        ("huggingface", "HuggingFace 下载的模型"),
        ("code/cacheddata", "VS Code 的代码缓存"),
        ("code/cache", "VS Code 的缓存"),
        ("code/user/history", "VS Code 的文件历史版本"),
        ("windsurf", "Windsurf 的本地数据"),
        (".cargo/registry", "Rust 依赖包缓存"),
        (".cargo/git", "Rust 的 Git 依赖缓存"),
        (".cargo", "Rust 工具链缓存"),
        (".rustup/toolchains", "Rust 工具链"),
        (".npm", "npm 的包缓存"),
        (".yarn", "Yarn 的包缓存"),
        (".pnpm", "pnpm 的包缓存"),
        (".cache/pip", "pip 的包缓存"),
        (".gradle", "Gradle 构建缓存"),
        (".m2", "Maven 依赖缓存"),
        (".go/pkg/mod", "Go 模块缓存"),
        ("deriveddata", "Xcode 的编译中间产物"),
        ("coresimulator", "iOS 模拟器运行时"),
        ("cocoapods", "CocoaPods 依赖缓存"),
        ("homebrew", "Homebrew 下载缓存"),
        ("com.docker.docker", "Docker 的磁盘镜像"),
        ("/target", "构建产物"),
        ("node_modules", "前端项目的依赖目录"),
        ("/.venv", "Python 虚拟环境"),
        ("__pycache__", "Python 的编译缓存"),
    ];

    // 路径末段一定要参与匹配：`/x/node_modules` 和 `/x/target` 就是靠它认出来的
    for (needle, label) in KNOWN {
        if lower.contains(needle) {
            // target / node_modules / .venv 会同时存在很多份，带上所属项目名
            // 才能让用户分清是哪一份（他就是靠这个决定哪个值得重编）。
            // 用「X 的构建产物」而不是「构建产物（X/target）」：后者把路径又抄一遍，
            // 而下一行本来就显示完整路径。
            if matches!(*needle, "/target" | "node_modules" | "/.venv") {
                let project = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if !project.is_empty() {
                    return match *needle {
                        "/target" => format!("{project} 的构建产物"),
                        "node_modules" => format!("{project} 的依赖目录"),
                        _ => format!("{project} 的虚拟环境"),
                    };
                }
            }
            return (*label).to_string();
        }
    }

    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| shown.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_name_prefers_the_specific_match() {
        // `Cursor/User/History` 必须比泛的 `cursor` 先命中，
        // 否则用户看到的是「Cursor 的数据目录」——那句话不解释它为什么占几十 GB。
        assert_eq!(
            friendly_name(
                "/Users/me",
                std::path::Path::new("/Users/me/Library/Application Support/Cursor/User/History")
            ),
            "Cursor 的文件历史版本"
        );
    }

    #[test]
    fn build_outputs_carry_their_project_name() {
        // target / node_modules 会同时存在很多份，只有带上项目名才能比较
        let n = friendly_name("/Users/me", std::path::Path::new("/Users/me/proj/a/target"));
        assert_eq!(n, "a 的构建产物", "名字里要能一眼分出是哪个项目");
    }

    #[test]
    fn unknown_paths_fall_back_to_their_last_segment() {
        // 认不出来时不能编：返回末段目录名，至少是真的
        assert_eq!(
            friendly_name("/Users/me", std::path::Path::new("/Users/me/weird-dir")),
            "weird-dir"
        );
    }
}
