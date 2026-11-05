//! `molt-gui`：给「净灵 · Cleanling」做的一个原生窗口。
//!
//! 它**不做任何判定**。扫描、判定、清理全部复用 `molt-core` / `molt-scan` /
//! `molt-act` 里已经存在并测过的那一套——GUI 只是同一份逻辑的另一个入口。
//! 这条约束是刻意的：如果两处各有一份判定，迟早会不一样，而差异出现在
//! 「能不能删」这种事上就是事故。
//!
//! 版式上刻意做成「顶部标签 + 一块宽内容区」，而不是常见的左侧栏：
//! 目标用户里有一半不懂电脑，左侧栏会让内容区凭空窄掉 180 点，
//! 体积数字和路径都会被挤断；而三个页面对他们来说是三个动作，横排更直白。
//!
//! 窗口主体放在 `app.rs` 并被编译成库，是为了让验证工具能复现同一份界面。
//!
//! 模块树只在 `lib.rs` 里声明一份：这里若再声明一遍，就会变成两个 crate
//! 各编译一次界面代码，`crate::ai_worker` 这类路径在 bin 里找不到——
//! 而且誰也说不清用户看到的到底是哪一份。

use molt_gui::MoltApp;

fn main() -> eframe::Result<()> {
    // 定时任务由 launchd 调用这个可执行文件本身。它不打开窗口，
    // 跑完就退出——用同一份二进制是为了「窗口里勾选的类型」与
    // 「夜里真正清理的东西」必然来自同一份实现。
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--run-schedule") {
        let code = molt_gui::schedule::run_headless();
        std::process::exit(code);
    }

    let options = eframe::NativeOptions {
        viewport: {
            let vp = eframe::egui::ViewportBuilder::default()
                .with_title("净灵 · Cleanling")
                .with_inner_size([1120.0, 800.0])
                .with_min_inner_size([720.0, 520.0]);
            // 窗口图标与顶栏用同一张图（crates/molt-gui/assets/icon.png）。
            // 裸二进制运行时 Dock 显示的就是它；打成 .app 后由 Info.plist 的
            // CFBundleIconFile 接管，两处保持同一个文件不会有一边忘了换。
            match molt_gui::brand::window_icon() {
                Some(icon) => vp.with_icon(icon),
                None => vp,
            }
        },
        ..Default::default()
    };

    eframe::run_native(
        "molt",
        options,
        Box::new(|cc| Ok(Box::new(MoltApp::new(cc)))),
    )
}
