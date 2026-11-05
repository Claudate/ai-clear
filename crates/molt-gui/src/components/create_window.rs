//! 「新建文件夹 / 文件」浮动窗口。
//!
//! 为什么是浮窗而不是第 7 个页面：它是一件**随手做的事**——建一个文件夹、
//! 起一个文件，做完就关。做成页面会让它跟「清理」「卸载」这些主线任务并列，
//! 用户每次都得先切页再回来；浮窗从工具页一点就开，关掉就回到原来那一页。
//!
//! 版式上只做一件事：**一屏里只有一个主要输入**（那几行路径）。
//! 建在哪、建什么这三块是同一个动作的三个部分，所以它们排成一条从上到下的
//! 单列，中间不再各自加小标题和说明；说明文字收进输入框的提示里。早期版本
//! 每个字段配一行灰字解释（「相对路径接在…」「一行一条…」），四条说明占掉
//! 窗口一半高度，读起来像一份表单填报表。
//!
//! 组件契约：只吃 props（[`CreateProps`]），只吐 [`CreateOutcome`]。
//! 它不认识 `AppState`，也不做任何写盘——预检结果由调用方算好传进来，
//! 真正落盘在 `app.rs::start_create`。

use eframe::egui;

use crate::icons;
use crate::state::CreateTab;
use crate::theme::Palette;

/// 这个窗口需要知道的一切。
pub struct CreateProps<'a> {
    pub root: &'a str,
    pub lines: &'a str,
    pub tab: CreateTab,
    /// 预检结果：会建几个、跳过几个、拒了哪几个
    pub plan: &'a molt_act::CreatePlan,
    /// 上一次创建的结果（用来显示「撤销」）
    pub last: Option<&'a molt_act::CreateReport>,
    pub error: Option<&'a str>,
    /// 现在能不能执行（有别的任务在跑时要挡住）
    pub can_run: bool,
    /// 挡住执行的理由
    pub blocked_hint: &'a str,
}

/// 用户在窗口里做了什么。
#[derive(Debug, Clone, PartialEq)]
pub enum CreateOutcome {
    None,
    Close,
    SetRoot(String),
    SetLines(String),
    SetTab(CreateTab),
    Run,
    Undo,
    /// 请求开系统目录选择框。真正弹框在 `app.rs` 里做：
    /// `rfd` 是阻塞调用，不能在画帧的途中调，否则整个窗口会冻在那里。
    PickRoot,
}

pub struct CreateWindow;

impl CreateWindow {
    /// 画一帧。返回用户这帧里做的事（每帧至多一件，够用且不会互相覆盖）。
    pub fn show(ctx: &egui::Context, p: &Palette, props: CreateProps<'_>) -> CreateOutcome {
        let w = p.sz(520.0);

        // 用 `Modal` 而不是 `Window`：`Window` 没有遮罩，浮在首页那张大白卡上时
        // 两者底色一样，看上去像卡片被切开了一块；而且它挡住的东西事后分不清
        // 「哪些是弹窗、哪些是页面」。`Modal` 自带压暗遮罩，点遮罩或按 Esc 就关，
        // 与确认框是同一套观感——这件随手做的事不该有第二种关闭方式。
        let resp = egui::Modal::new(egui::Id::unique("create_window"))
            .backdrop_color(egui::Color32::from_black_alpha(80))
            .frame(
                egui::Frame::NONE
                    .fill(p.surface)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| {
                ui.set_width(w);
                if title_row(ui, p) {
                    CreateOutcome::Close
                } else {
                    body(ui, p, &props)
                }
            });

        if resp.should_close() {
            CreateOutcome::Close
        } else {
            resp.inner
        }
    }
}

/// 自绘标题行。返回 `true` 表示用户点了叉号。
fn title_row(ui: &mut egui::Ui, p: &Palette) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        // 标题左边给一枚图标：这一行是窗口里唯一不是输入的地方，
        // 有图标才不像「一个没有头的表单」。
        ui.add(
            egui::Label::new(icons::rich_for(p, icons::FOLDER_PLUS, p.sz(19.0)))
                .wrap_mode(egui::TextWrapMode::Extend),
        );
        ui.add_space(p.sz(4.0));
        ui.add(
            egui::Label::new(egui::RichText::new("新建").size(p.sz(17.0)).strong())
                .wrap_mode(egui::TextWrapMode::Extend),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::close_button(ui, p, "关掉") {
                close = true;
            }
        });
    });
    ui.add_space(14.0);
    close
}

fn body(ui: &mut egui::Ui, p: &Palette, props: &CreateProps<'_>) -> CreateOutcome {
    let mut out = CreateOutcome::None;

    // ── 建在哪：一枚前置图标 + 输入框 + 选择按钮，一条线 ──
    //
    // 不再单列一个「建在哪」小标题：这个输入框本身已经写着完整路径，
    // 加上标题只是多一行字。图标把这一行认成「位置」，比标题更快。
    let mut root = props.root.to_string();
    ui.horizontal(|ui| {
        // 图标 → 输入框 → 按钮，按正常顺序排（左到右），按钮用
        // `available_width` 收尾贴到右边。不能整行用 `right_to_left`：
        // 那个布局里输入框若写 `desired_width(available_width())` 会吃掉
        // 后面的全部空间，图标被挤成 0 宽——图标就在布局里占一块真实位置，
        // 不是画完之后拿 painter 叠上去（叠上去会压在提示文字上）。
        let side = p.sz(16.0);
        let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
        icons::draw(ui.painter(), icons::FOLDER, icon_rect, p.text_faint);
        ui.add_space(p.sz(6.0));

        // 按钮宽度先量出来，剩下的才是输入框的
        let btn_w = ui
            .painter()
            .layout_no_wrap(
                "选择…".to_owned(),
                egui::FontId::proportional(p.sz(12.5)),
                p.text,
            )
            .size()
            .x
            + p.sz(24.0);
        ui.add(
            egui::TextEdit::singleline(&mut root)
                .desired_width((ui.available_width() - btn_w - p.sz(6.0)).max(p.sz(80.0)))
                .font(egui::TextStyle::Body)
                .hint_text("~/Documents/新项目"),
        );
        ui.add_space(p.sz(6.0));
        if crate::widgets::secondary_button(ui, p, "选择…", true) {
            out = CreateOutcome::PickRoot;
        }
    });
    if root != props.root {
        out = CreateOutcome::SetRoot(root);
    }

    ui.add_space(10.0);

    // ── 建什么：三个带图标的页签 ──
    //
    // 页签只决定「起点模板」，所以它们的名字要能在不看说明的情况下读懂；
    // 图标是给扫视用的（项目 → 文件夹、单个 → 文件）。
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = p.sz(6.0);
        for tab in CreateTab::ALL {
            if tab_pill(ui, p, tab, props.tab == tab) {
                out = CreateOutcome::SetTab(tab);
            }
        }
    });

    ui.add_space(10.0);

    // ── 那几行路径：窗口的主体 ──
    let mut lines = props.lines.to_string();
    let edit = ui.add_sized(
        egui::vec2(ui.available_width(), p.sz(168.0)),
        egui::TextEdit::multiline(&mut lines)
            .font(egui::TextStyle::Monospace)
            .hint_text("src/\nREADME.md\n# 以 # 开头的一行会跳过"),
    );
    if edit.changed() {
        out = CreateOutcome::SetLines(lines);
    }

    if let Some(err) = props.error {
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(egui::RichText::new(err).size(p.sz(11.5)).color(p.danger))
                .wrap_mode(egui::TextWrapMode::Wrap),
        );
    }

    ui.add_space(10.0);
    summary(ui, p, props);

    ui.add_space(14.0);

    // ── 底栏：取消在左、创建在右，主次分明 ──
    ui.horizontal(|ui| {
        if crate::widgets::secondary_button(ui, p, "取消", true) {
            out = CreateOutcome::Close;
        }
        // 撤销只在上一次真的建了东西时出现——按钮在那儿却不能按更让人困惑
        if props.last.is_some_and(|r| !r.created.is_empty()) {
            if crate::widgets::secondary_button(ui, p, "撤销上一次", props.can_run) {
                out = CreateOutcome::Undo;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let can = props.can_run && props.plan.will_create() > 0;
            if crate::widgets::primary_button(ui, p, "创建", can) {
                out = CreateOutcome::Run;
            }
            if !props.can_run && !props.blocked_hint.is_empty() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(props.blocked_hint)
                            .size(p.sz(10.5))
                            .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
    });

    out
}

/// 一个页签：图标 + 名字，选中时用主色实底。
///
/// 不用 `widgets::segmented`：那个是按「可回收 / 有代价」这种**数值筛选**做的，
/// 选项之间是同一维度的开关；这里是三种起点，配图标更好扫。
fn tab_pill(ui: &mut egui::Ui, p: &Palette, tab: CreateTab, selected: bool) -> bool {
    let (glyph, label) = match tab {
        CreateTab::Skeleton => (icons::CUBE, "项目骨架"),
        CreateTab::Dirs => (icons::FOLDER, "文件夹"),
        CreateTab::Files => (icons::FILE, "文件"),
    };
    let font = egui::FontId::proportional(p.sz(12.5));
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        font,
        if selected { p.on_accent() } else { p.text_dim },
    );
    let icon_side = p.sz(13.0);
    let pad = egui::vec2(p.sz(11.0), p.sz(6.0));
    let gap = p.sz(5.0);
    let size = egui::vec2(
        pad.x * 2.0 + icon_side + gap + galley.size().x,
        (galley.size().y + pad.y * 2.0).max(p.sz(30.0)),
    );
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = resp.hovered();
    let (bg, fg) = if selected {
        (p.reclaimable, p.on_accent())
    } else if hovered {
        (p.surface_hover, p.text)
    } else {
        (p.surface_alt, p.text_dim)
    };
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(crate::theme::RADIUS_PILL),
        bg,
        egui::Stroke::new(1.0, if selected { p.reclaimable } else { p.border }),
        egui::StrokeKind::Inside,
    );
    icons::draw(
        ui.painter(),
        glyph,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + pad.x + icon_side / 2.0, rect.center().y),
            egui::vec2(icon_side, icon_side),
        ),
        fg,
    );
    ui.painter().galley(
        egui::pos2(
            rect.left() + pad.x + icon_side + gap,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        fg,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// 预检结论：一行摘要 + 被拒的项。
///
/// 「会建几个 / 跳过几个 / 拒了几个」必须在按下去之前就写出来：
/// 用户在按下「创建」前要能看出自己写错了哪一行。
fn summary(ui: &mut egui::Ui, p: &Palette, props: &CreateProps<'_>) {
    let plan = props.plan;
    let will = plan.will_create();
    let skipped = plan.skipped();
    let rejected = plan.rejected();

    let color = if !rejected.is_empty() {
        p.costly
    } else if will == 0 {
        p.text_faint
    } else {
        p.reclaimable
    };

    // 摘要做成一条软底条：它是这一屏唯一的「结论」，和输入区分开。
    ui.scope(|ui| {
        let w = ui.available_width();
        let h = p.sz(30.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
            p.surface_alt,
        );
        icons::draw(
            ui.painter(),
            if will > 0 {
                icons::SPARKLE
            } else {
                icons::MAGNIFYING_GLASS
            },
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + p.sz(16.0), rect.center().y),
                egui::vec2(p.sz(14.0), p.sz(14.0)),
            ),
            color,
        );

        let mut parts = vec![format!("将创建 {will} 项")];
        if skipped > 0 {
            parts.push(format!("{skipped} 项已存在"));
        }
        if !rejected.is_empty() {
            parts.push(format!("{} 项被拒绝", rejected.len()));
        }
        ui.painter().text(
            egui::pos2(rect.left() + p.sz(30.0), rect.center().y),
            egui::Align2::LEFT_CENTER,
            parts.join(" · "),
            egui::FontId::proportional(p.sz(12.0)),
            color,
        );
    });

    if !rejected.is_empty() {
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("create_rejected")
            .max_height(p.sz(72.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (path, why) in rejected.iter().take(20) {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{} — {why}", path.display()))
                                .size(p.sz(10.5))
                                .color(p.costly),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
            });
    }

    if let Some(last) = props.last {
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("上一次：{}", last.summary()))
                    .size(p.sz(10.5))
                    .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
    }
}
