//! 出图工具：把界面的每一种状态渲染成 PNG。
//!
//! 用法：`cargo run -p molt-gui --bin shots -- <输出目录>`
//!
//! 它跑的是 `molt_gui` 库里同一份界面代码，因此图上的东西就是用户会看到的。
//! 这条路径在没有屏幕录制权限时也能用（用 egui 自己的帧缓冲截图）。
//!
//! 用例按「用户会遇到的处境」组织，而不是按页面罗列：
//! 正常扫完、扫完什么都没有、扫描失败、全部受保护、AI 解释过、任务进行中（互斥）、
//! 确认框、隔离区、历史、自动清理、设置。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui;
use molt_core::{Candidate, CostKind, ItemKind, ProtectReason, RebuildCost, Verdict};
use molt_gui::state::{AppRow, AppState, HistoryRow, Page};
use molt_gui::theme::Mode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args
        .get(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/shots"));
    std::fs::create_dir_all(&out).expect("建输出目录");
    // 只出名字含某个片段的图：全量出图要遍历每个应用的真实体积，改一处等一分钟。
    let only = args.get(2).cloned();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("shots")
            .with_inner_size([1120.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "shots",
        options,
        Box::new(move |cc| Ok(Box::new(Shots::new(cc, out, only)))),
    )
    .expect("跑快照");
}

/// 一个要出图的状态。
struct Case {
    name: &'static str,
    page: Page,
    mode: Mode,
    setup: fn(&mut AppState),
}

/// 窄窗要看的不是「好不好看」，而是会不会把按钮挤出屏幕、
/// 或者把一行数字括成两层。尺寸按小尺寸笔记本的可视区域取。
const NARROW: &[&str] = &["18-narrow-workbench", "27-narrow-status-bar"];
const NARROW_SIZE: [f32; 2] = [760.0, 640.0];

/// 清单展开后要往下读很长一段，默认 800 高的窗只拍得到首屏。
/// 这一张是为了核对「每条到底多高、一屏能读几条」，所以给足高度。
const TALL: &[&str] = &[
    "23a-workbench-list",
    "23b-workbench-list-ai",
    "23c-workbench-list-dark",
    "23d-workbench-partial-failure",
    // 抽屉从窗口顶部拉到底部，矮窗里只能看到半页。
    "43-workbench-detail",
    "44-workbench-detail-dark",
];
const TALL_SIZE: [f32; 2] = [1120.0, 1600.0];

struct Shots {
    out: std::path::PathBuf,
    cases: Vec<Case>,
    idx: usize,
    frames: usize,
    /// 已经为本张图请求过截图
    requested: bool,
    /// 截图回调是否已写盘
    saved: Arc<AtomicBool>,
    app: Option<molt_gui::MoltApp>,
}

impl Shots {
    fn new(
        cc: &eframe::CreationContext<'_>,
        out: std::path::PathBuf,
        only: Option<String>,
    ) -> Self {
        let cases = vec![
            Case {
                name: "01-workbench",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_data,
            },
            // 读不到磁盘时不能画一条 0% 的环——那会被读成「盘是空的」。
            Case {
                name: "01a-workbench-no-disk",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: without_disk,
            },
            // 还没扫完：环照常画（磁盘数据不依赖扫描），但不说能清多少。
            Case {
                name: "01b-workbench-unsanned",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: disk_only,
            },
            Case {
                name: "02-workbench-ai",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_ai_notes,
            },
            Case {
                name: "03-workbench-dark",
                page: Page::Workbench,
                mode: Mode::Dark,
                setup: with_ai_notes,
            },
            Case {
                name: "04-workbench-loading",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_scanning,
            },
            Case {
                name: "05-workbench-empty",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_empty,
            },
            Case {
                name: "06-workbench-error",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_error,
            },
            Case {
                name: "07-workbench-protected",
                page: Page::Workbench,
                mode: Mode::Dark,
                setup: with_protected,
            },
            Case {
                name: "08-workbench-busy",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_busy,
            },
            Case {
                name: "09-confirm-quarantine",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_pending_quarantine,
            },
            Case {
                name: "10-confirm-delete",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_pending_delete,
            },
            Case {
                name: "12-quarantine",
                page: Page::Quarantine,
                mode: Mode::Light,
                setup: with_quarantine,
            },
            Case {
                name: "13-quarantine-empty",
                page: Page::Quarantine,
                mode: Mode::Light,
                setup: with_quarantine_empty,
            },
            Case {
                name: "14-history",
                page: Page::History,
                mode: Mode::Light,
                setup: with_history,
            },
            Case {
                name: "14b-history-filtered",
                page: Page::History,
                mode: Mode::Light,
                setup: with_history_filtered,
            },
            Case {
                name: "15-automation",
                page: Page::Automation,
                mode: Mode::Light,
                setup: with_automation,
            },
            Case {
                name: "15b-automation-dirty",
                page: Page::Automation,
                mode: Mode::Light,
                setup: with_automation_dirty,
            },
            Case {
                name: "16-automation-never",
                page: Page::Automation,
                mode: Mode::Dark,
                setup: with_automation_off,
            },
            Case {
                name: "17-settings",
                page: Page::Settings,
                mode: Mode::Light,
                setup: with_settings,
            },
            Case {
                name: "17b-settings-advanced",
                page: Page::Settings,
                mode: Mode::Light,
                setup: with_settings_advanced,
            },
            Case {
                name: "18-narrow-workbench",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_data,
            },
            Case {
                name: "19-workbench-freed",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_freed,
            },
            Case {
                name: "20-workbench-stale",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_stale,
            },
            Case {
                name: "21-workbench-ai-unset",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_ai_unset,
            },
            Case {
                name: "22-workbench-ai-failed",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_ai_failed,
            },
            Case {
                name: "23a-workbench-list",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_list_open,
            },
            Case {
                name: "23d-workbench-partial-failure",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_partial_failure,
            },
            Case {
                name: "23b-workbench-list-ai",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_list_open_ai,
            },
            Case {
                name: "23c-workbench-list-dark",
                page: Page::Workbench,
                mode: Mode::Dark,
                setup: with_list_open,
            },
            Case {
                name: "23-workbench-filtered",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_costly_filter,
            },
            // 清理进行中：环上要有进度弧、圆心要写百分比。
            Case {
                name: "24-workbench-cleaning",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_cleaning,
            },
            // 清完那一刻：满蓝环 + 「清理完成 / 已腾出」。
            Case {
                name: "25-workbench-cleaned",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_cleaned,
            },
            // 报错与提示改成底部悬浮条之后的样子。
            Case {
                name: "26-status-bar",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_status_modal,
            },
            // 窄窗 + 底部条：长句子不能把提示条撑出屏幕。
            Case {
                name: "27-narrow-status-bar",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_status_modal,
            },
            // 深色下也要看一遍：这条的底色取自页面背景，配色错了会变成一块
            // 突兀的浅色板压在深底上。
            Case {
                name: "28-status-bar-dark",
                page: Page::Workbench,
                mode: Mode::Dark,
                setup: with_status_modal,
            },
            // 成功态：蓝点 + 「完成」，没有「去设置」那枚次要按钮。
            Case {
                name: "29-status-bar-ok",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_status_ok,
            },
            // 卸载页：列应用、展开一个、本体与残留都可单独勾。
            Case {
                name: "30-uninstall",
                page: Page::Uninstall,
                mode: Mode::Light,
                setup: with_apps,
            },
            // 展开态：详情卡片 + 勾选行 + 「卸载」。
            Case {
                name: "38-uninstall-open",
                page: Page::Uninstall,
                mode: Mode::Light,
                setup: with_apps_open,
            },
            Case {
                name: "39-uninstall-open-dark",
                page: Page::Uninstall,
                mode: Mode::Dark,
                setup: with_apps_open,
            },
            Case {
                name: "31-uninstall-dark",
                page: Page::Uninstall,
                mode: Mode::Dark,
                setup: with_apps,
            },
            // 一个应用都没有：说的是「这两个目录里没有」，而不是「出错」。
            Case {
                name: "32-uninstall-empty",
                page: Page::Uninstall,
                mode: Mode::Light,
                setup: with_apps_empty,
            },
            Case {
                name: "33-uninstall-loading",
                page: Page::Uninstall,
                mode: Mode::Light,
                setup: with_apps_loading,
            },
            // 新建浮窗：叠在首页上，三页签 + 预检数字 + 创建按钮。
            Case {
                name: "34-create-window",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_create,
            },
            Case {
                name: "35-create-window-dark",
                page: Page::Workbench,
                mode: Mode::Dark,
                setup: with_create,
            },
            // 输入里有一行越界：预检要当场把它列出来，而不是按下创建才报错。
            Case {
                name: "36-create-rejected",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_create_rejected,
            },
            // 真机形态：数据目录名与应用名不一致，且数据挂在厂商/产品两层里。
            // 这两个坑只有真盘会暴露，夹具照真机抄一遍，好让以后改匹配规则时看得见后果。
            Case {
                name: "37-uninstall-vendor-layout",
                page: Page::Uninstall,
                mode: Mode::Light,
                setup: with_vendor_layout,
            },
            // 工具页：顶栏扳手切过去的那一页。
            // 三个分类、每个工具一张卡片，空分类要留占位。
            Case {
                name: "40-tools-page",
                page: Page::Tools,
                mode: Mode::Light,
                setup: with_tools_page,
            },
            Case {
                name: "41-tools-page-dark",
                page: Page::Tools,
                mode: Mode::Dark,
                setup: with_tools_page,
            },
            // 扫描中：能按的、不能按的要在同一张图里都看到，
            // 不能按的那条要写清理由，而不是点下去没反应。
            Case {
                name: "42-tools-page-busy",
                page: Page::Tools,
                mode: Mode::Light,
                setup: with_tools_page_busy,
            },
            // 详情抽屉：一行放不下的完整路径、判定理由与 AI 解释都在这里。
            Case {
                name: "43-workbench-detail",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_detail_open,
            },
            Case {
                name: "44-workbench-detail-dark",
                page: Page::Workbench,
                mode: Mode::Dark,
                setup: with_detail_open,
            },
        ];
        // 只保留名字含指定片段的用例：全量出图要遍历每个应用的真实体积，
        // 改一处布局等一分钟不值当。
        let mut cases = cases;
        if let Some(f) = only {
            cases.retain(|c| c.name.contains(f.as_str()));
        }

        let mut s = Self {
            out,
            cases,
            idx: 0,
            frames: 0,
            requested: false,
            saved: Arc::new(AtomicBool::new(false)),
            app: None,
        };
        s.prepare(cc);
        s
    }

    /// 按当前序号重建界面与状态。
    fn prepare(&mut self, cc: &eframe::CreationContext<'_>) {
        let case = &self.cases[self.idx];
        let mut state = AppState::new("/Users/me".into());
        (case.setup)(&mut state);
        state.page = case.page;
        self.apply_size(&cc.egui_ctx, case.name);

        let app = molt_gui::MoltApp::headless(&cc.egui_ctx, case.mode, state);
        // `Theme::install` 会把偏好设回 System，必须在它之后再锁一次
        cc.egui_ctx.set_theme(match case.mode {
            Mode::Light => egui::ThemePreference::Light,
            Mode::Dark => egui::ThemePreference::Dark,
        });
        // 截图时关掉动画：模态框有淡入，前几帧半透明
        for th in [egui::Theme::Light, egui::Theme::Dark] {
            cc.egui_ctx.style_mut_of(th, |s| s.animation_time = 0.0);
        }
        self.app = Some(app);
        self.frames = 0;
        self.requested = false;
        self.saved.store(false, Ordering::SeqCst);
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let Some(app) = &mut self.app else { return };
        app.draw(ui);
    }

    /// 换窗口尺寸。宽度真的变小时，按钮与数字的重排才会显出来。
    fn apply_size(&self, ctx: &egui::Context, name: &str) {
        let size = if NARROW.contains(&name) {
            egui::vec2(NARROW_SIZE[0], NARROW_SIZE[1])
        } else if TALL.contains(&name) {
            egui::vec2(TALL_SIZE[0], TALL_SIZE[1])
        } else {
            egui::vec2(1120.0, 800.0)
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }
}

impl eframe::App for Shots {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
        self.frames += 1;

        if self.frames < 40 {
            // 多空几帧：清理动画走 `stable_dt`，前几帧 dt 是 0，
            // 环上的进度弧根本还没长出来。
            ui.ctx().request_repaint();
            return;
        }

        if !self.requested {
            self.requested = true;
            let ctx = ui.ctx().clone();
            let path = self.out.join(format!("{}.png", self.cases[self.idx].name));
            let saved = self.saved.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::ScreenshotCallback::new(move |img| {
                    save_png(&path, &img);
                    saved.store(true, Ordering::SeqCst);
                }),
            ));
            ui.ctx().request_repaint();
            return;
        }

        if self.saved.load(Ordering::SeqCst) {
            self.idx += 1;
            if self.idx >= self.cases.len() {
                println!("全部完成");
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            let case = &self.cases[self.idx];
            let mut state = AppState::new("/Users/me".into());
            (case.setup)(&mut state);
            state.page = case.page;
            self.apply_size(&ui.ctx().clone(), case.name);
            let mode = case.mode;
            let ctx = ui.ctx().clone();
            for th in [egui::Theme::Light, egui::Theme::Dark] {
                ctx.style_mut_of(th, |s| s.animation_time = 0.0);
            }
            ctx.set_theme(match mode {
                Mode::Light => egui::ThemePreference::Light,
                Mode::Dark => egui::ThemePreference::Dark,
            });
            let mut theme = molt_gui::theme::Theme::install(&ctx);
            ctx.set_theme(match mode {
                Mode::Light => egui::ThemePreference::Light,
                Mode::Dark => egui::ThemePreference::Dark,
            });
            theme.mode = mode;
            theme.palette = molt_gui::theme::Palette::of(mode);
            theme.follow_system = false;
            molt_gui::theme::reinstall(&ctx, &theme);
            self.app = Some(molt_gui::MoltApp::with_parts(theme, state));
            self.frames = 0;
            self.requested = false;
            self.saved.store(false, Ordering::SeqCst);
        }
        ui.ctx().request_repaint();
    }
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) {
    let w = img.size[0] as u32;
    let h = img.size[1] as u32;
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    for px in &img.pixels {
        bytes.extend_from_slice(&[px.r(), px.g(), px.b(), px.a()]);
    }
    let file = std::fs::File::create(path).expect("建 png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&bytes))
        .expect("写 png");
    println!("{}", path.display());
}

// ---- 各种状态的数据 ----

fn disk(total: u64, avail: u64) -> molt_scan::disk::DiskUsage {
    molt_scan::disk::DiskUsage {
        total,
        used: total - avail,
        available: avail,
    }
}

fn protected() -> Verdict {
    Verdict::Protected {
        reason: ProtectReason::UserRecord,
    }
}

fn costly(kind: CostKind, note: &str) -> Verdict {
    Verdict::Costly {
        rebuild: RebuildCost::new(kind, note),
    }
}

fn cand(path: &str, bytes: u64, v: Verdict) -> Candidate {
    Candidate::new(path, ItemKind::Dir, v).with_size(bytes, bytes, 12)
}

/// 造一份接近真机的数据：大件少、小件多，三档都有。
///
/// 路径必须唯一：界面按路径字符串记「勾没勾」，重复路径会让几行共享同一个勾选状态。
fn items(n: usize) -> Vec<Candidate> {
    let projects = [
        "write-T",
        "mptext-cli",
        "mr.dayone",
        "web-app",
        "gallery",
        "molt",
        "blog",
        "console",
        "worker",
        "parser",
    ];
    let kinds = [
        (
            "target",
            26u64 << 30,
            costly(CostKind::Recompile, "重新编译，约 4 分钟"),
        ),
        (
            "target",
            8 << 30,
            costly(CostKind::Recompile, "重新编译，约 2 分钟"),
        ),
        (
            "node_modules",
            3 << 30,
            costly(CostKind::Reinstall, "重新安装依赖，需联网"),
        ),
        (
            ".venv",
            900 << 20,
            costly(CostKind::Reinstall, "重新安装依赖，需联网"),
        ),
    ];
    let caches = [
        (
            "~/Library/Caches/Cursor/CachedData",
            780u64 << 20,
            Verdict::Reclaimable,
        ),
        ("~/.npm/_cacache", 620 << 20, Verdict::Reclaimable),
        ("~/Library/Caches/Homebrew", 190 << 20, Verdict::Reclaimable),
        (
            "~/.cargo/registry/cache",
            410 << 20,
            costly(CostKind::Redownload, "重新下载，耗时取决于网络"),
        ),
        (
            "~/.ollama/models",
            4 << 30,
            costly(CostKind::Redownload, "重新下载，耗时取决于网络"),
        ),
        ("~/.claude/projects", 88 << 20, protected()),
        ("~/.ssh", 4 << 20, protected()),
    ];

    (0..n)
        .map(|i| {
            let mut c = if i % 2 == 0 {
                let (leaf, b, v) = &kinds[(i / 2) % kinds.len()];
                let project = projects[(i / 2) % projects.len()];
                let div = ((n - i) % 5 + 1) as u64;
                cand(&format!("~/project/{project}/{leaf}"), b / div, v.clone())
            } else {
                let (p, b, v) = &caches[(i / 2) % caches.len()];
                let div = ((n - i) % 4 + 1) as u64;
                cand(&format!("{p}/part-{}", i / 2), b / div, v.clone())
            };
            // 让一部分有代价项带上「建议清理」：界面要能画出那一档
            if i % 8 == 0 {
                c.suggest_clean = true;
            }
            c.files = 40 + (i as u64 % 7) * 13;
            c
        })
        .collect()
}

fn put(state: &mut AppState, n: usize) {
    state.apply_result(Arc::new(molt_gui::worker::ScanResult {
        items: items(n),
        rule_ids: vec![],
        denied_roots: vec![],
        elapsed_secs: 39.0,
        from_cache: false,
    }));
}

fn with_data(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 19 << 30)));
    put(s, 24);
}

/// 只有磁盘读数、没有扫描结果：第一帧的画面就是这个状态。
fn disk_only(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 19 << 30)));
}

/// 连磁盘也读不到（例如盘被卸载、或统计调用失败）。
fn without_disk(s: &mut AppState) {
    s.disk = Some(Err("statvfs 失败".to_string()));
    put(s, 24);
}

/// AI 解释过的状态：每一条都带一句说明。
fn with_ai_notes(s: &mut AppState) {
    with_data(s);
    for c in &s.result.clone().unwrap().items {
        let path = c.path.display().to_string();
        let note = if path.contains("target") {
            "cargo build 的输出目录，删掉后下次构建会重新生成，约 4 分钟"
        } else if path.contains("node_modules") {
            "项目的依赖目录，删掉后 npm install 会重新装好，需要联网"
        } else if path.contains("_cacache") {
            "npm 下载过的包缓存，删掉只会让下次安装慢一点"
        } else if path.contains("CachedData") {
            "Cursor 的编译缓存，编辑器会自己重建，不影响设置与历史"
        } else if path.contains("ollama") {
            "本地大模型权重，删掉要重新下载几十 GB"
        } else if path.contains(".ssh") {
            "你的 SSH 私钥，永远不该被清理工具碰到"
        } else if path.contains("claude") {
            "Claude Code 的对话记录，是你的工作记录而不是缓存"
        } else {
            "构建或下载留下的中间产物，可以重新生成"
        };
        s.ai.notes.insert(path, note.to_string());
    }
    s.ai.last_model = Some("TypeSafe Jev（第三方） · jev".into());
    s.ai.last_sent = 24;
    s.ai.last_at = Some("2026-10-23T11:40:00Z".into());
}

/// 详情抽屉打开：看第一条的完整记录。
fn with_detail_open(s: &mut AppState) {
    with_ai_notes(s);
    if let Some(first) = s.result.as_ref().and_then(|r| r.items.first()) {
        s.detail_path = Some(first.path.display().to_string());
    }
}

fn with_scanning(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 19 << 30)));
    // 界面要展示的是「正在扫描」：result 还没出来（None），用一个
    // 对真实主目录的只读扫描撑住前几帧。扫不存在的 /Users/me 会
    // 立刻完成，画面直接跳到「没有找到可清理的内容」。
    if let Ok(rules) = molt_rules::load("/Users/me", None) {
        s.rules = Some(rules.clone());
        let home = molt_act::paths::home_dir().to_string_lossy().to_string();
        s.job = Some(molt_gui::worker::ScanJob::start(rules, home, false));
    }
}

fn with_empty(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 120 << 30)));
    put(s, 0);
}

fn with_error(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 120 << 30)));
    s.error = Some("遍历 ~/Library/Caches 时被系统拒绝：没有读取权限".into());
}

fn with_protected(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    let list = vec![
        cand("~/.claude/projects", 88 << 20, protected()),
        cand("~/.ssh", 4 << 20, protected()),
        cand(
            "~/Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
            12 << 30,
            protected(),
        ),
        cand("~/Library/Developer/Xcode/Archives", 6 << 30, protected()),
    ];
    s.apply_result(Arc::new(molt_gui::worker::ScanResult {
        items: list,
        rule_ids: vec![],
        denied_roots: vec!["~/Library/Mail".into()],
        elapsed_secs: 22.0,
        from_cache: false,
    }));
}

/// 重扫失败、但旧结果还在：必须把「这是旧结果」画在最上面。
fn with_stale(s: &mut AppState) {
    with_data(s);
    s.last_scan_at = Some("10:42".into());
    s.error = Some("遍历 ~/Library/Caches 时被系统拒绝：没有读取权限".into());
}

/// AI 未配置：按钮该是灰的，旁边说清为什么。
fn with_ai_unset(s: &mut AppState) {
    with_data(s);
    s.ai.config.active_provider.clear();
    s.ai.config.active_model.clear();
}

/// AI 调了但失败了：结果条旁边要说「上次失败」并给一条去设置的路。
fn with_ai_failed(s: &mut AppState) {
    with_data(s);
    s.ai.config.active_provider = "deepseek".into();
    s.ai.config.active_model = "deepseek-chat".into();
    s.ai.failure = Some(molt_gui::ai_worker::AiFailure {
        message: "模型没有返回可解析的结果（HTTP 401：密钥无效）".into(),
        fixable_by_config: true,
        raw: None,
    });
}

/// 清单不筛：看整段分节列表的实际密度。
///
/// 这一张存在的理由是核对「一行到底多高」——行高与重复说明被压过一次，
/// 需要一张完全没有筛选干扰的清单来对照。
fn with_list_open(s: &mut AppState) {
    with_data(s);
}

/// 清单展开 + 每条都带 AI 解释：这是行高最高的一种情况，也是要看能不能读清的那种。
/// 清理完成但有几项失败：失败明细默认收起，只留一行摘要。
///
/// 这一张是用来核对「失败不该压过主任务」的——以前这里默认铺开十二条
/// 带完整路径与系统错误的橙色明细，清理结果反而被挤到屏幕外。
fn with_partial_failure(s: &mut AppState) {
    with_data(s);
    s.last_op = Some(
        molt_gui::state::OpReport {
            summary: "已移入隔离区 8 项，释放 25.4 GB".into(),
            done: 8,
            done_bytes: 25_400_000_000,
            failures: vec![
                (
                    "~/project/gallery/target".into(),
                    "权限不足：Operation not permitted (os error 1)".into(),
                ),
                (
                    "~/Library/Caches/Homebrew".into(),
                    "目标正在被占用：Resource busy (os error 16)".into(),
                ),
            ],
            kind: molt_gui::state::StatusKind::Warn,
            next: Some(molt_gui::state::NextStep::Quarantine),
        }
        .into(),
    );
}

fn with_list_open_ai(s: &mut AppState) {
    with_ai_notes(s);
}

/// 筛到「要确认」：清单只留那一档，且必须有一条回得去的路（显示全部）。
fn with_costly_filter(s: &mut AppState) {
    with_data(s);
    s.filter = molt_gui::state::Filter::Costly;
}

/// 清理刚做完：首屏要能说出「本次运行已腾出多少」。
///
/// 这个数字此前只累加在 state 里从不显示，于是用户点完「清理」，
/// 除了底部一条会消失的结果条之外没有任何持久证据。
fn with_freed(s: &mut AppState) {
    with_data(s);
    s.session_freed = 1_240_000_000;
}

/// 清理进行到一半：进度弧 + 圆心百分比。
fn with_cleaning(s: &mut AppState) {
    with_data(s);
    s.clean_anim = 0.46;
    s.clean_total_items = 24;
    s.clean_total_bytes = 3_600_000_000;
}

/// 刚清完：满蓝环停住，圆心写「清理完成 / 已腾出」。
fn with_cleaned(s: &mut AppState) {
    with_data(s);
    s.clean_anim = 1.0;
    s.clean_done_secs = 1.5;
    s.clean_was_delete = true;
    s.clean_total_items = 24;
    s.clean_total_bytes = 3_600_000_000;
    s.session_freed = 3_600_000_000;
}

/// 一条失败提示：必须看得见，但贴在窗口底部，不挡在屏幕正中。
fn with_status_modal(s: &mut AppState) {
    with_data(s);
    s.status = Some(molt_gui::state::StatusMessage::error(
        "清理没能完成：/Users/me/project/molt/target 被另一个进程占用（cargo 正在编译）。关掉那个进程后可以重试。",
    ));
}

/// 一条成功提示：蓝色点位 + 「完成」，没有「去设置」。
fn with_status_ok(s: &mut AppState) {
    with_data(s);
    s.status = Some(molt_gui::state::StatusMessage::info(
        "已把 24 项放进临时空间，7 天内可以随时还原。",
    ));
}

/// 互斥状态：有任务在跑，所有会冲突的按钮都该是灰的。
fn with_busy(s: &mut AppState) {
    with_ai_notes(s);
    if let Ok(rules) = molt_rules::load("/Users/me", None) {
        s.rules = Some(rules.clone());
        // 用一个对真实主目录的只读扫描撑住「忙碌」：扫描完成前
        // 不会覆盖 result（24 项 + AI 解释还在），顶栏显示
        // 「正在扫描 N/M」、所有运行入口置灰，互斥一目了然。
        // 之前用空 CleanJob 会在第 3 帧截图前就 Done，触发对
        // 不存在路径的重扫，顶栏冒出与互斥无关的假错误。
        let home = molt_act::paths::home_dir().to_string_lossy().to_string();
        s.job = Some(molt_gui::worker::ScanJob::start(rules, home, false));
    }
}

fn with_pending_quarantine(s: &mut AppState) {
    with_ai_notes(s);
    s.pending_clean = s.pending_from_checked(molt_act::CleanMode::Quarantine);
}

fn with_pending_delete(s: &mut AppState) {
    with_ai_notes(s);
    s.pending_clean = s.pending_from_checked(molt_act::CleanMode::Delete);
}

fn qrow(id: i64, name: &str, bytes: u64, restorable: bool) -> molt_gui::state::QuarantineRow {
    molt_gui::state::QuarantineRow {
        id,
        original: format!("~/{name}"),
        bytes,
        at: "2026-10-20T10:00:00Z".into(),
        purge_after: "2026-10-27T10:00:00Z".into(),
        restorable,
    }
}

fn with_quarantine(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    s.quarantine = vec![
        qrow(1, ".npm/_cacache", 620 << 20, true),
        qrow(2, "Library/Caches/Cursor/CachedData", 780 << 20, true),
        qrow(3, "project/web-app/node_modules", 3 << 30, false),
    ];
    s.quarantine_loading = false;
}

fn with_quarantine_empty(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 120 << 30)));
    s.quarantine_loading = false;
}

fn with_history(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    s.history = vec![
        HistoryRow {
            id: 3,
            path: "~/.npm/_cacache".into(),
            bytes: 620 << 20,
            at: "2026-10-23T03:00:12Z".into(),
            source: molt_act::CleanSource::Auto,
            action: molt_gui::state::HistoryAction::Quarantined,
        },
        HistoryRow {
            id: 2,
            path: "~/project/web-app/target".into(),
            bytes: 2 << 30,
            at: "2026-10-22T21:14:03Z".into(),
            source: molt_act::CleanSource::Manual,
            action: molt_gui::state::HistoryAction::Deleted,
        },
        HistoryRow {
            id: 1,
            path: "~/Library/Caches/Cursor/CachedData".into(),
            bytes: 780 << 20,
            at: "2026-10-22T21:10:44Z".into(),
            source: molt_act::CleanSource::Manual,
            action: molt_gui::state::HistoryAction::Quarantined,
        },
    ];
}

fn with_automation(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    if let Ok(rules) = molt_rules::load("/Users/me", None) {
        s.auto.config.rule_ids = molt_gui::schedule::all_rule_ids(&rules);
        s.rules = Some(rules);
    }
    s.auto.config.enabled = true;
    s.auto.config.hour = 3;
    s.auto.config.minute = 0;
    s.auto.config.frequency = molt_gui::schedule::Frequency::Daily;
    s.auto.loaded = true;
    s.auto.installed = Some(true);
    s.auto.last_run = Some(molt_gui::schedule::LastRun {
        at: "2026-10-23T03:00:07Z".into(),
        ok: true,
        types: 12,
        types_effective: 12,
        candidates: 6,
        done: 6,
        bytes: 1_240_000_000,
        failures: vec![],
        message: "已移入隔离区 6 项，共 1.2 GB".into(),
    });
}

fn with_automation_off(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    if let Ok(rules) = molt_rules::load("/Users/me", None) {
        s.auto.config.rule_ids = molt_gui::schedule::all_rule_ids(&rules);
        s.rules = Some(rules);
    }
    s.auto.loaded = true;
    s.auto.installed = Some(false);
}

/// 历史页：已经筛到「永久删除」这一档。
///
/// 这张图要能看出两件事：分段上带计数、列表只剩这一档的行。
fn with_history_filtered(s: &mut AppState) {
    with_history(s);
    s.history_filter = molt_gui::state::HistoryFilter::Gone;
}

/// 自动清理页：改过时间但还没保存。
///
/// 底部条必须在这一张图里说明「有未保存的更改」——这正是之前缺的那一步。
fn with_automation_dirty(s: &mut AppState) {
    with_automation(s);
    let mut draft = s.auto.config.clone();
    draft.hour = 9;
    draft.minute = 15;
    s.auto.draft = Some(draft);
}

/// 设置页：展开「高级」，把排障要用的原始字段露出来。
fn with_settings_advanced(s: &mut AppState) {
    with_settings(s);
    s.settings_advanced = true;
}

fn with_settings(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    if let Ok(rules) = molt_rules::load("/Users/me", None) {
        s.rules = Some(rules);
    }
    s.ai.config.active_provider = "deepseek".into();
    s.ai.config.active_model = "deepseek-chat".into();
    s.ai.config.providers[2].api_key = "sk-shots-demo".into();
}

// ---- 卸载页 ----

fn lrow(path: &str, category: &'static str, bytes: u64) -> molt_gui::state::LeftoverRow {
    molt_gui::state::LeftoverRow {
        path: path.to_string(),
        display: path.to_string(),
        category,
        bytes,
        needs_admin: path.starts_with("/Library/"),
        unmeasured: false,
    }
}

fn arow(
    name: &str,
    path: &str,
    bundle: Option<&str>,
    bytes: u64,
    leftovers: Vec<molt_gui::state::LeftoverRow>,
) -> AppRow {
    let leftover_bytes = leftovers.iter().map(|l| l.bytes).sum();
    AppRow {
        name: name.to_string(),
        path: path.to_string(),
        bundle_id: bundle.map(|s| s.to_string()),
        bytes,
        bytes_known: true,
        leftovers,
        leftover_bytes,
        // 出图时不读真实图标：夹具里的路径在本机多半不存在，
        // 而「首字母方块」这条退路本来也要能看。
        icon_file: None,
        // 夹具的残留是现成的，等同于「已经扫过」
        leftovers_loaded: true,
    }
}

/// 真机形态：三条性能与视觉上都有代表性的应用。
///
/// - Chrome：数据在 `Application Support/Google/Chrome` 与 `Caches/Google/Chrome`，
///   名字带了厂商却没有 `com.google.Chrome` 这个目录。
/// - VS Code：`.app` 叫 `Visual Studio Code`，数据目录却叫 `Code`。
/// - Obsidian：数据目录与 `.app` 同名，是「正常」的那一类。
///
/// 三种形态并排，任何一条匹配规则退化了，这张图都能看出来。
fn with_vendor_layout(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    s.uninstall.apps = vec![
        arow(
            "Google Chrome",
            "/Applications/Google Chrome.app",
            Some("com.google.Chrome"),
            2_100_000_000,
            vec![
                lrow(
                    "~/Library/Application Support/Google/Chrome",
                    "应用数据",
                    5_100_000_000,
                ),
                lrow("~/Library/Caches/Google/Chrome", "缓存", 1_100_000_000),
                lrow(
                    "~/Library/Preferences/com.google.Chrome.plist",
                    "偏好设置",
                    4_000,
                ),
            ],
        ),
        arow(
            "Visual Studio Code",
            "/Applications/Visual Studio Code.app",
            Some("com.microsoft.VSCode"),
            1_400_000_000,
            vec![
                lrow(
                    "~/Library/Application Support/Code",
                    "应用数据",
                    2_800_000_000,
                ),
                lrow("~/Library/Caches/com.microsoft.VSCode", "缓存", 2_800_000),
            ],
        ),
        arow(
            "Obsidian",
            "/Applications/Obsidian.app",
            Some("md.obsidian"),
            481_900_000,
            vec![lrow(
                "~/Library/Application Support/Obsidian",
                "应用数据",
                89_900_000,
            )],
        ),
    ];
    let path = s.uninstall.apps[0].path.clone();
    s.uninstall.open = Some(path);
}

/// 卸载页的正常状态：用**本机真实装着的几个应用**，图标才画得出来。
///
/// 假名字（Docker / Figma）在这台机器上解不出图标，出图时全是首字母方块，
/// 看不出网格到底对不对。这里挑几个各有代表性的真实 `.app`：
/// 图标在 icns 里的（Chrome）、二进制 Info.plist 的（VS Code）、
/// 大小写混排名字的（wpsoffice）、以及只有一个词的（Obsidian）。
///
/// 只读这几个的 Info.plist，不遍历整盘的体积——全量扫描要几分钟，
/// 出图工具不该为了一张图等那么久。
fn with_apps(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 40 << 30)));
    const PICKS: [&str; 8] = [
        "Google Chrome",
        "Visual Studio Code",
        "Obsidian",
        "WeChat",
        "Clash Verge",
        "Termius",
        "ModelTest",
        "Bunch",
    ];
    let mut rows = Vec::new();
    for name in PICKS {
        let app = std::path::Path::new("/Applications").join(format!("{name}.app"));
        if !app.is_dir() {
            continue;
        }
        // 体积用固定值：真实体积要遍历整个 .app，几十万文件会拖住出图。
        // 网格要验的是排布与图标，不是数字。
        let bytes = match name {
            "Google Chrome" => 2_100_000_000,
            "Visual Studio Code" => 1_400_000_000,
            _ => 180_000_000,
        };
        // 借 `scan_apps` 读 Info.plist（它只开这一个文件，不遍历目录），
        // 但把体积换成固定值——真实体积要遍历几十万文件，出图等不起。
        let meta = molt_act::uninstall::read_app_meta(&app);
        rows.push(AppRow {
            name: name.to_string(),
            path: app.display().to_string(),
            bundle_id: meta.bundle_id,
            bytes,
            bytes_known: true,
            leftover_bytes: 0,
            leftovers: Vec::new(),
            icon_file: meta.icon_file,
            // 真机路径量体积太慢，这里只借 Info.plist 造行；残留没扫过
            leftovers_loaded: false,
        });
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    s.uninstall.apps = rows;
    s.uninstall.loading = false;
    s.uninstall.scanned_dirs = vec!["/Applications".into(), "~/Applications".into()];
}

/// 同上，但展开体积最大的那个应用——详情卡片、勾选行、「卸载」按钮都在这一张里。
///
/// 残留清单照 Chrome 真机上的形态写死。这里**不**调 `leftovers_for`：
/// 那要遍历真实的 9 GB 目录来算体积，出一次图要等一分钟以上。
/// 匹配规则本身有单元测试守着，出图只需要一个像样的清单。
fn with_apps_open(s: &mut AppState) {
    with_apps(s);
    let Some(first) = s.uninstall.apps.first() else {
        return;
    };
    let path = first.path.clone();
    let rows = vec![
        lrow(
            "~/Library/Application Support/Google/Chrome",
            "应用数据",
            5_100_000_000,
        ),
        lrow("~/Library/Caches/Google/Chrome", "缓存", 1_100_000_000),
        lrow("~/Library/WebKit/com.google.Chrome", "网页数据", 96_000_000),
        lrow(
            "/Library/Application Support/com.google.Chrome",
            "系统级数据",
            2_600_000,
        ),
        lrow(
            "~/Library/Preferences/com.google.Chrome.plist",
            "偏好设置",
            4_000,
        ),
    ];
    let total: u64 = rows.iter().map(|r| r.bytes).sum();
    if let Some(app) = s.uninstall.apps.first_mut() {
        app.leftovers = rows;
        app.leftover_bytes = total;
    }
    s.uninstall.open = Some(path);
}

fn with_apps_empty(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 120 << 30)));
    s.uninstall.loading = false;
    s.uninstall.scanned_dirs = vec!["/Applications".into(), "~/Applications".into()];
}

fn with_apps_loading(s: &mut AppState) {
    s.disk = Some(Ok(disk(228 << 30, 120 << 30)));
    s.uninstall.loading = true;
}

// ---- 新建浮窗 ----

/// 浮窗打开、输入了一组路径：预检数字要跟着输入走。
fn with_create(s: &mut AppState) {
    with_data(s);
    s.create.open = true;
    s.create.root = "~/project/new-app".into();
    s.create.lines = "src/\ntests/\ndocs/\nREADME.md\nCargo.toml\n.gitignore\n".into();
}

/// 输入里混了一行越界路径：预检要当场列出来。
fn with_create_rejected(s: &mut AppState) {
    with_data(s);
    s.create.open = true;
    s.create.root = "~/project/new-app".into();
    s.create.lines = "src/\n../../etc/passwd\nREADME.md\n".into();
}

/// 工具页的样子。页面本身画三分类、三个工具。
fn with_tools_page(s: &mut AppState) {
    with_data(s);
}

/// 同上，但正处在一个会碰盘的动作里：工具页里「新建」与「重新扫描」
/// 都不可按，并写明为什么。
///
/// 这张图要能同时看到「能按的」和「不能按的」两种条目——
/// 只有前者的话，用户点了一个不响应的条目会以为界面坏了。
///
/// 用 `restoring` 这个既有的忙碌位来模拟，不真的起一个扫描线程：
/// 出图要的是那一屏的观感，不是重跑一次扫描。
fn with_tools_page_busy(s: &mut AppState) {
    with_tools_page(s);
    s.restoring = Some(1);
}
