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
use molt_gui::state::{AppState, HistoryRow, Page};
use molt_gui::theme::Mode;

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/shots"));
    std::fs::create_dir_all(&out).expect("建输出目录");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("shots")
            .with_inner_size([1120.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "shots",
        options,
        Box::new(move |cc| Ok(Box::new(Shots::new(cc, out)))),
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
const NARROW: &[&str] = &["18-narrow-workbench", "27-narrow-status-modal"];
const NARROW_SIZE: [f32; 2] = [760.0, 640.0];

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
    fn new(cc: &eframe::CreationContext<'_>, out: std::path::PathBuf) -> Self {
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
            // 清完那一刻：满绿环 + 「清理完成 / 已腾出」。
            Case {
                name: "25-workbench-cleaned",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_cleaned,
            },
            // 报错与提示改成弹窗之后的样子。
            Case {
                name: "26-status-modal",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_status_modal,
            },
            // 窄窗 + 弹窗：长句子不能把弹窗撑出屏幕。
            Case {
                name: "27-narrow-status-modal",
                page: Page::Workbench,
                mode: Mode::Light,
                setup: with_status_modal,
            },
        ];

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
            self.app = Some(molt_gui::MoltApp { theme, state });
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

/// 筛到「要确认」：清单只留那一档，且必须有一条回得去的路（显示全部）。
fn with_costly_filter(s: &mut AppState) {
    with_data(s);
    s.filter = molt_gui::state::Filter::Costly;
    s.list_expanded = true;
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

/// 刚清完：满绿环停住，圆心写「清理完成 / 已腾出」。
fn with_cleaned(s: &mut AppState) {
    with_data(s);
    s.clean_anim = 1.0;
    s.clean_done_secs = 1.5;
    s.clean_was_delete = true;
    s.clean_total_items = 24;
    s.clean_total_bytes = 3_600_000_000;
    s.session_freed = 3_600_000_000;
}

/// 一条失败提示：必须弹出来，不能只在角落写一行小字。
fn with_status_modal(s: &mut AppState) {
    with_data(s);
    s.status = Some(molt_gui::state::StatusMessage::error(
        "清理没能完成：/Users/me/project/molt/target 被另一个进程占用（cargo 正在编译）。关掉那个进程后可以重试。",
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
