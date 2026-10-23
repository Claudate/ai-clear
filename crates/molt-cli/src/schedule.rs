//! 定时任务：把 `molt auto` 挂到 launchd 上。
//!
//! 为什么用 launchd 而不是 cron：macOS 上 cron 需要「完全磁盘访问权限」授权，
//! 而那个授权按可执行文件路径发放——`cargo build` 换一次路径就得重新授权，
//! 用户会以为是工具坏了。launchd 的 agent 走用户会话，权限与终端里跑一致。
//!
//! 这里**只生成配置文件，不自己执行任何清理**。装不装、什么时候跑，是用户的决定。

use std::path::{Path, PathBuf};

/// launchd 的 label，同时也是 plist 文件名（去掉 `.plist`）。
pub const LABEL: &str = "app.molt.autoclean";

/// 定时执行的时间点：每周日 03:00。
///
/// 选周日凌晨的理由：这是一周里最不可能有人在敲代码的时间，
/// 而且周末结束后第一次构建本来就要重编——被清掉的缓存不会额外打断谁。
pub const WEEKDAY: u32 = 0; // 0 = 周日
pub const HOUR: u32 = 3;
pub const MINUTE: u32 = 0;

/// plist 的完整内容。
///
/// 几个必须写对的地方，写错了 launchd 会静默不加载：
/// - `ProgramArguments` 里每一项都必须是绝对路径，不能有 `~`
/// - 标准输出必须落到文件，否则子进程写满管道会卡住
/// - `StartCalendarInterval` 用整数，不是字符串
pub fn plist(binary: &Path, log_dir: &Path) -> String {
    let bin = binary.display();
    let out = log_dir.join("auto.log").display().to_string();
    let err = log_dir.join("auto.err.log").display().to_string();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>

    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>auto</string>
        <string>--apply</string>
    </array>

    <!-- 每周日 03:00。launchd 在机器休眠错过后会在唤醒时补跑一次。 -->
    <key>StartCalendarInterval</key>
    <dict>
        <key>Weekday</key>
        <integer>{WEEKDAY}</integer>
        <key>Hour</key>
        <integer>{HOUR}</integer>
        <key>Minute</key>
        <integer>{MINUTE}</integer>
    </dict>

    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>

    <!-- 低优先级：清理不该和用户正在跑的构建抢 IO -->
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>

    <key>RunAtLoad</key>
    <false/>
</dict>
</plist>
"#
    )
}

/// 定时任务的 plist 路径：`~/Library/LaunchAgents/app.molt.autoclean.plist`。
pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// 日志目录。与隔离区、审计库同一个 app 目录，用户找的时候只记一个地方。
pub fn log_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_has_no_tilde_and_uses_absolute_binary() {
        let text = plist(
            Path::new("/Users/me/.local/bin/molt"),
            Path::new("/Users/me/Library/Application Support/app.molt/logs"),
        );
        assert!(!text.contains('~'), "plist 里不能有 ~，launchd 不展开它");
        assert!(text.contains("/Users/me/.local/bin/molt"));
        assert!(text.contains("app.molt.autoclean"));
    }

    #[test]
    fn plist_schedules_sunday_three_am() {
        let text = plist(Path::new("/bin/molt"), Path::new("/tmp/logs"));
        assert!(text.contains("<key>Weekday</key>"));
        assert!(text.contains("<integer>0</integer>"));
        assert!(text.contains("<key>Hour</key>"));
        assert!(text.contains("<integer>3</integer>"));
        assert!(text.contains("<string>auto</string>"));
    }

    #[test]
    fn scheduled_run_actually_applies() {
        // 少了 --apply，定时任务会永远只演练、一次都不清。
        // 这个错误不会报错，只会表现为「装了一年什么都没发生」。
        let text = plist(Path::new("/bin/molt"), Path::new("/tmp/logs"));
        assert!(
            text.contains("<string>--apply</string>"),
            "定时任务必须带 --apply，否则只会演练"
        );
    }

    #[test]
    fn plist_does_not_run_at_load() {
        // 登录时跑一次清理是最招人烦的设计：用户刚开机，磁盘就在转
        let text = plist(Path::new("/bin/molt"), Path::new("/tmp/logs"));
        assert!(text.contains("<key>RunAtLoad</key>\n    <false/>"));
    }

    #[test]
    fn stdout_goes_to_a_file() {
        let text = plist(Path::new("/bin/molt"), Path::new("/tmp/logs"));
        assert!(text.contains("<key>StandardOutPath</key>"));
        assert!(text.contains("/tmp/logs/auto.log"));
    }

    #[test]
    fn plist_path_lives_under_launch_agents() {
        let p = plist_path(Path::new("/Users/me"));
        assert_eq!(
            p,
            PathBuf::from("/Users/me/Library/LaunchAgents/app.molt.autoclean.plist")
        );
    }

    #[test]
    fn plist_is_valid_xml_plist() {
        // 写成非法 plist 时 launchd 不会报错，只会永远不执行——
        // 用户会以为是「定时任务没生效」而不是「配置写错了」。
        // 所以这里用系统自带的 plutil 真校验一次。
        let dir = std::env::temp_dir().join(format!("molt-plist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.plist");
        std::fs::write(&path, plist(Path::new("/bin/molt"), Path::new("/tmp/logs"))).unwrap();

        let out = std::process::Command::new("plutil")
            .args(["-lint", &path.display().to_string()])
            .output();

        let cleanup = || {
            std::fs::remove_dir_all(&dir).ok();
        };
        match out {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                let ok = o.status.success() && !stdout.contains("error");
                cleanup();
                assert!(ok, "plist 不合法：{stdout}{stderr}");
            }
            // 没有 plutil 的平台跳过——这不是失败
            Err(_) => cleanup(),
        }
    }
}
