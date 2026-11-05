//! 卸载页：像启动台一样铺开应用，双击打开、右键卸载。
//!
//! 版式照 Finder / 启动台来：一张张**图标 + 名字**的格子，而不是一行行路径。
//! 用户认得的是那个图标，不是 `/Applications/Figma.app` 这一串；只写名字的
//! 清单他得逐行读才能找到想卸的那个。图标的解码见 [`crate::appicon`]。
//!
//! 交互与启动台一致，三个动作各管一件事：
//!
//! 1. **单击** —— 选中它，并浮起一个小窗，把这个应用名下的文件和各自大小列出来
//! 2. **双击** —— 启动这个应用（用户点图标本来就想打开它）
//! 3. **右键** —— 菜单里只有「卸载」，确认后整包移入临时空间，7 天内可还原
//!
//! 详情为什么从「网格下方展开」改成浮窗：卸载真正的位置信息（它占了哪些文件、
//! 各有多大）是**看**的，不是**操作**的。压在网格下方会把剩下的应用推到视野外，
//! 而用户挑选时最需要的正是「整片应用都在眼前」。浮窗可以拖开、也可以不管它。
//!
//! 浮窗里刻意没有按钮：打开应用是双击，打开目录与卸载是右键，收起就是关掉浮窗。
//! 同一件事只留一个入口，浮窗就只剩「告诉你有哪些文件、具体多大」这一件事。
//!
//! 系统自带的应用不列：它们受 SIP 保护、删不掉，列出来只会让人以为「这个也能卸」。

use eframe::egui;

use crate::action::Action;
use crate::state::{AppRow, AppState};
use crate::theme::Palette;
use crate::widgets::{card, page_header, primary_button, secondary_button, state_block};

pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    // 说明行带上清单的时间：切回这一页不再重扫（那是几秒的等待），
    // 那就必须如实说出「这份清单是什么时候读的」，让「重新扫描」有依据。
    let subtitle = match state.uninstall.listed_at.and_then(listed_ago) {
        Some(ago) => format!(
            "单击看它有哪些文件，双击打开应用，右击卸载。清单{ago}读的，\
             装了或卸了应用后点「重新扫描」。"
        ),
        None => "单击看它有哪些文件，双击打开应用，右击卸载。".to_string(),
    };
    page_header(ui, p, "卸载", &subtitle, |ui| {
        if secondary_button(ui, p, "重新扫描", !state.is_listing()) {
            acts.push(Action::LoadApps);
        }
    });
    ui.add_space(10.0);

    // 读失败优先于「空」：扫不出来和「一个应用都没有」对用户是两件事。
    if let Some(err) = state.uninstall.error.clone() {
        card(ui, p, |ui| {
            ui.label(
                egui::RichText::new("列不出已安装的应用")
                    .size(p.sz(15.0))
                    .strong()
                    .color(p.danger),
            );
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(egui::RichText::new(&err).size(p.sz(12.0)).color(p.text_dim))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(10.0);
            if primary_button(ui, p, "重试", true) {
                acts.push(Action::LoadApps);
            }
        });
        return;
    }

    // 体积还在补的时候要持续重绘：后台每量好一条就发一条消息，
    // 不主动请求重绘的话界面会停在上一帧，用户看到的仍是「正在量…」。
    if state.app_scan_job.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }

    if state.uninstall.loading && state.uninstall.apps.is_empty() {
        state_block(
            ui,
            p,
            "◌",
            "正在列出应用…",
            "先只读名字与图标，体积随后逐个填上；残留等点中某个应用时再找。",
            p.text_faint,
        );
        return;
    }

    if state.uninstall.apps.is_empty() {
        state_block(
            ui,
            p,
            "○",
            "这两个目录里没有可卸载的应用",
            "看的是 /Applications 与 ~/Applications。系统自带的工具不列——它们删不掉。",
            p.text_faint,
        );
        return;
    }

    // 网格整页滚一次，按内容占高。浮窗不进滚动区：它必须能拖到网格外的空白上。
    egui::ScrollArea::vertical()
        .id_salt("uninstall_page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            grid(ui, state, p, acts);
        });

    info_window(ui.ctx(), state, p, acts);
}

/// 图标网格的格子尺寸。宽度是固定的：图标与名字的视觉重量不随窗口变，
/// 窗口变宽时该多排几列，而不是把每个格子拉长。
/// 宽度取 168：缩放到 1.4 后是 235 点，1160 宽的内容区正好排 5 列——
/// 8 个应用排成 5+3 而不是 7+1，不会有一个孤零零落在第二行。
const CELL_W: f32 = 168.0;
/// 格子高度按「图标 + 两行名字 + 体积」量出来：
/// 上边距 18 + 图标 64 + 间距 9 + 两行名字 30 + 间距 4 + 体积 14 + 下边距 6 = 145，
/// 取 148。
///
/// 少算就会让「Visual Studio Code」这种两行名字压到体积上——实测过，96 点正好重叠。
const CELL_H: f32 = 148.0;

/// 应用网格。撑满整页并随窗口宽度重排列数。
fn grid(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let apps = &state.uninstall.apps;
    let total = apps.len();
    let bytes: u64 = apps.iter().map(|a| a.bytes + a.leftover_bytes).sum();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{total} 个应用"))
                .size(p.sz(12.5))
                .strong()
                .color(p.text),
        );
        ui.label(
            egui::RichText::new(format!("· 合计 {}", molt_scan::human_bytes(bytes)))
                .size(p.sz(11.5))
                .color(p.text_dim),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new("按体积从大到小")
                    .size(p.sz(11.0))
                    .color(p.text_faint),
            );
        });
    });
    ui.add_space(6.0);

    // 网格卡片**按内容收缩**：应用少的时候不能撑满一屏、把下半截留成空白
    // （用户提过两次「下面空白」）。
    let cw = p.sz(CELL_W);
    let per_row = ((ui.available_width() / cw).floor() as usize).max(1);
    let rows = total.div_ceil(per_row);

    card(ui, p, |ui| {
        let w = ui.available_width();
        // 格子宽度固定、整行居中。把格子拉宽去填满会让列之间空出一大块，
        // 图标看起来像散在页面各处，而不是排成一片网格。
        let lead = ((w - per_row as f32 * cw) / 2.0).max(0.0);
        for r in 0..rows {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(lead);
                for c in 0..per_row {
                    let i = r * per_row + c;
                    let Some(app) = apps.get(i) else {
                        break;
                    };
                    cell(
                        ui,
                        p,
                        app,
                        cw,
                        state.uninstall.is_open(&app.path),
                        state.uninstall.removing.as_deref() == Some(app.path.as_str()),
                        acts,
                    );
                }
            });
        }
    });
}

/// 一个「图标 + 名字 + 体积」的格子，整块可点。
///
/// 一次点击在 egui 里是 click 与 double_clicked 两条事件，双击的第一下也会是
/// `clicked()`。所以这里的分工是：`clicked()` 只做「选中」（浮窗出现），
/// `double_clicked()` 才启动应用——先选中再启动不会有什么后果，
/// 反过来（双击只当单击）会让用户以为图标点不动。
fn cell(
    ui: &mut egui::Ui,
    p: &Palette,
    app: &AppRow,
    cell_w: f32,
    open: bool,
    removing: bool,
    acts: &mut Vec<Action>,
) {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(cell_w, p.sz(CELL_H)), egui::Sense::click());

    // 悬停 / 选中：整格一块浅底，和启动台一样「整块是一件事」
    if open {
        ui.painter().rect_filled(
            rect.shrink(p.sz(4.0)),
            egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
            p.accent_soft,
        );
    } else if resp.hovered() {
        ui.painter().rect_filled(
            rect.shrink(p.sz(4.0)),
            egui::CornerRadius::same(crate::theme::RADIUS_CONTROL),
            p.surface_hover,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // 图标：真图标解码不出来时退回「首字母圆角方块」——
    // 绝不能留一个空洞，那样网格会看起来像没加载完。
    let side = p.sz(crate::appicon::ICON_SIDE);
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + p.sz(18.0) + side / 2.0),
        egui::vec2(side, side),
    );
    let tex = crate::appicon::texture(
        ui.ctx(),
        std::path::Path::new(&app.path),
        app.icon_file.as_deref(),
    );
    match tex {
        Some(t) => {
            ui.painter().image(
                t.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            ui.painter().rect_filled(
                icon_rect,
                egui::CornerRadius::same((side * 0.24) as u8),
                p.surface_alt,
            );
            let initial: String = app.name.chars().take(1).collect();
            ui.painter().text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(side * 0.46),
                p.text_dim,
            );
        }
    }

    // 名字：最多两行，超出省略。启动台里也是这么折的。
    let name_top = icon_rect.bottom() + p.sz(9.0);
    let name_w = rect.width() - p.sz(14.0);
    let name_color = if open { p.reclaimable } else { p.text };
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = name_w;
    // 两行封顶：名字长的（如「Claude Code URL Handler」）不该把格子撑破
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = false;
    job.append(
        &app.name,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(p.sz(12.0)),
            color: name_color,
            ..Default::default()
        },
    );
    let shown = ui.painter().layout_job(job);
    let name_h = shown.size().y;
    ui.painter().galley(
        egui::pos2(rect.center().x - shown.size().x / 2.0, name_top),
        shown,
        name_color,
    );

    // 体积：带残留的一行灰字，让「卸它到底能清多少」一眼看到。
    // 位置固定在格子底部，与两行名字之间留足间距。
    //
    // `bytes_known == false` 表示「还没量完」（`list_apps` 先出清单、体积后补），
    // 这时写「正在量…」而不是 `0 B`——0 B 是一个错误的事实，用户会以为
    // 这个应用是空的。反过来，量过而结果是 0（真的很小、或那次遍历超时）
    // 就老老实实写出来，不能让格子永远停在加载态。
    let size_text = if !app.bytes_known {
        "正在量…".to_string()
    } else if app.leftover_bytes > 0 {
        format!(
            "{} + {}",
            molt_scan::human_bytes(app.bytes),
            molt_scan::human_bytes(app.leftover_bytes)
        )
    } else {
        molt_scan::human_bytes(app.bytes)
    };
    // 体积紧跟名字下方，而不是钉在格子底部：名字一行时贴底会空出一大段，
    // 两行时又贴到边上。跟着名字走，两种名字长度看起来都是成组的。
    ui.painter().text(
        egui::pos2(rect.center().x, name_top + name_h + p.sz(4.0)),
        egui::Align2::CENTER_TOP,
        size_text,
        egui::FontId::proportional(p.sz(10.8)),
        p.text_dim,
    );

    // 右击：菜单里只有卸载。菜单项点完要关闭菜单，否则它会留在窗口上
    // 等下一次点别处——用户会以为卸载没生效。
    resp.context_menu(|ui| {
        if ui.button("卸载").clicked() {
            acts.push(Action::PrepareUninstall(app.path.clone()));
            ui.close();
        }
    });

    // 正在卸载的那个格子不再响应点击：它马上就不在磁盘上了
    if resp.double_clicked() && !removing {
        acts.push(Action::LaunchApp(app.path.clone()));
    } else if resp.clicked() && !removing {
        acts.push(Action::OpenApp(app.path.clone()));
    }

    if removing {
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y),
            egui::Align2::CENTER_CENTER,
            "正在卸载…",
            egui::FontId::proportional(p.sz(11.5)),
            p.costly,
        );
    }
}

/// 文件信息浮窗：这个应用名下的每一个文件，以及它有多大。
///
/// 用 `egui::Window`（可拖动、无标题栏）而不是 `Modal`：它列的是「查看」用的
/// 信息，不该压暗整个界面挡住其它应用——用户经常要在两三个应用之间比大小。
fn info_window(ctx: &egui::Context, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(app) = state.uninstall.current() else {
        return;
    };
    let w = p.sz(420.0);
    let mut open = true;
    let mut close = false;

    // Esc 关掉：没有「收起」按钮了，键盘得有一条退路。
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }

    // 默认落在**右下角的空白**里。
    //
    // 之前的做法是按「这一列在左半边还是右半边」决定贴左还是贴右，可窗口不够宽时
    // 两半都压着图标——截图里 WeChat 就被切掉了一半。浮窗本来就该盖在空白上：
    // 网格通常只占屏幕上半截，下面的空白正好放它。
    //
    // 高度不预设，按清单行数反推：头部 62 + 每行 30 + 页脚 24。清单长时浮窗就往上长，
    // 短时贴着底边——这样无论 3 条还是 30 条，右下角都是它的落点。
    let screen = ctx.content_rect();
    let rows = state.uninstall.file_count().max(1);
    let est_h = p.sz(62.0) + p.sz(30.0) * rows as f32 + p.sz(28.0);
    let x = (screen.right() - w - p.sz(24.0)).max(screen.left() + p.sz(16.0));
    let y = (screen.bottom() - p.sz(20.0) - est_h).max(screen.top() + p.sz(88.0));
    let default_pos = egui::pos2(x, y);

    egui::Window::new("uninstall_info")
        .id(egui::Id::unique("uninstall_info_window"))
        .title_bar(false)
        .resizable(false)
        .movable(true)
        .default_pos(default_pos)
        .default_width(w)
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(egui::Stroke::new(1.0, p.border))
                .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
                .inner_margin(egui::Margin::same(16))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 22,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(38),
                }),
        )
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(w - p.sz(32.0));

            // 头部：图标 + 名字 + 总计。这里是用户确认「我看的是哪一个」的地方，
            // 所以名字用正文大小，不和下面的文件行走同一个字级。
            ui.horizontal(|ui| {
                let side = p.sz(30.0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                let tex = crate::appicon::texture(
                    ui.ctx(),
                    std::path::Path::new(&app.path),
                    app.icon_file.as_deref(),
                );
                if let Some(t) = tex {
                    ui.painter().image(
                        t.id(),
                        r,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                ui.add_space(p.sz(9.0));
                ui.vertical(|ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&app.name).size(p.sz(14.5)).strong())
                            .wrap_mode(egui::TextWrapMode::Truncate),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} 个文件 · 共 {}",
                            state.uninstall.file_count(),
                            molt_scan::human_bytes(state.uninstall.total_bytes())
                        ))
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                    );
                });
                // 右上角一个叉。它是窗口自己的关闭钮，不是原来那排操作按钮的一员：
                // 没有标题栏就只剩 Esc 一条路，而没人会去猜一个浮窗该怎么关。
                // 再点一次那个图标同样能关掉。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if crate::widgets::close_button(ui, p, "关闭（Esc）") {
                        close = true;
                    }
                });
            });

            ui.add_space(10.0);

            // 文件清单。本体排第一，后面按体积从大到小——用户想知道的是
            // 「到底是什么占了这么多」，所以他先看最大的那条。
            let mut rows: Vec<(&str, String, u64, bool)> =
                Vec::with_capacity(app.leftovers.len() + 1);
            rows.push((
                app.path.as_str(),
                "应用本体".to_string(),
                app.bytes,
                app.bytes_known,
            ));
            for l in &app.leftovers {
                let label = if l.needs_admin {
                    format!("{} · 需管理员权限", l.category)
                } else {
                    l.category.to_string()
                };
                rows.push((l.display.as_str(), label, l.bytes, !l.unmeasured));
            }
            // 本体不参与排序：它是这个应用本身，永远在最上面。
            rows[1..].sort_by_key(|r| std::cmp::Reverse(r.2));

            egui::ScrollArea::vertical()
                .id_salt("uninstall_info_files")
                .max_height(p.sz(300.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (path, label, bytes, measured) in &rows {
                        file_row(ui, p, path, label, *bytes, *measured);
                    }
                });

            // 残留还在找的时候把状态说清楚，而不是让清单看起来「就这么点」。
            if state.uninstall.leftovers_loading.as_deref() == Some(app.path.as_str()) {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("正在列出这个应用留下的其它文件…")
                        .size(p.sz(11.0))
                        .color(p.text_faint),
                );
            } else if app.leftovers_loaded && app.leftovers.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("除了本体，没有找到它留在别处的文件。")
                        .size(p.sz(11.0))
                        .color(p.text_faint),
                );
            }

            if app.bundle_id.is_none() {
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "这个应用没声明 bundle id，只能按名字找残留——外面也许还有没列出来的。",
                        )
                        .size(p.sz(10.5))
                        .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });

    if close || !open {
        acts.push(Action::CloseApp);
    }
}

/// 浮窗里的一行：左边路径与类别（长了截断），右边大小。
///
/// 左右两块都用**固定宽度**的子区域：直接把 `vertical` 和 `right_to_left` 并排，
/// 左边会吃掉全部可用宽度，右边于是贴着左边文字的末尾——同一列的 GB 与 KB
/// 会各自停在不同位置，看起来像没有对齐的表格。右栏定宽才有一条齐的竖线。
fn file_row(ui: &mut egui::Ui, p: &Palette, path: &str, label: &str, bytes: u64, measured: bool) {
    let size_w = p.sz(66.0);
    let left_w = (ui.available_width() - size_w - p.sz(8.0)).max(p.sz(60.0));
    let row_h = p.sz(30.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, row_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(path).size(p.sz(11.5)))
                        .wrap_mode(egui::TextWrapMode::Truncate),
                );
                ui.label(
                    egui::RichText::new(label)
                        .size(p.sz(10.0))
                        .color(p.text_faint),
                );
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(size_w, row_h),
            egui::Layout::right_to_left(egui::Align::Min),
            |ui| {
                // 量不完的目录写「≥」而不是给一个偏小的数字：那个数字是下界，
                // 当成完整大小会让人以为「就这么大」。
                let text = if measured {
                    molt_scan::human_bytes(bytes)
                } else {
                    format!("≥ {}", molt_scan::human_bytes(bytes))
                };
                ui.label(egui::RichText::new(text).size(p.sz(11.5)).color(p.text_dim));
            },
        );
    });
}

/// 收尾一次卸载：等本体真的从磁盘上消失，再把这个应用从网格里去掉。
///
/// 判断依据是**磁盘**，不是那批 `CleanJob` 的进度——卸载跑在后台线程里，
/// 拿进度去猜「成了没有」会在部分失败时给出错的结论（比如残留全移走了、
/// 本体因为被占用没动）。本体不见了才算卸掉。
///
/// 放在这一页而不是 `app.rs`：`pump_clean` / `pump_purge` 都在各自的页里，
/// 只有主循环能调到的东西就没法端到端验——而「卸完网格里还在」正是要验的那一步。
pub fn pump_done(state: &mut AppState) {
    let Some(target) = state.uninstall.removing.clone() else {
        return;
    };
    if std::path::Path::new(&target).exists() {
        // 还在盘上：可能正在移、也可能失败。清理任务收尾时会给出原因，
        // 那一份走 `pump_clean`，所以这里只负责「成功」这一侧。
        return;
    }

    state.uninstall.removing = None;
    state.uninstall.open = None;

    let name = state
        .uninstall
        .apps
        .iter()
        .find(|a| a.path == target)
        .map(|a| a.name.clone())
        .unwrap_or_else(|| crate::paths::display_path_str(&target));
    state.uninstall.apps.retain(|a| a.path != target);

    // 从网格里去掉之后，页头那个「N 个应用 · 合计」也跟着变——
    // 那条数字是现算的，所以这里不需要再改它。
    state.status = Some(crate::state::StatusMessage::info(format!(
        "已卸载 {name}，它和残留都放进了临时空间，{} 天内可还原",
        crate::paths::DEFAULT_RETAIN_DAYS
    )));
    crate::pages::quarantine::reload(state);
    crate::pages::history::reload(state);
    state.refresh_disk();
}

/// 右键菜单「卸载」之后的确认框。
///
/// 卸载不是不可恢复的（东西进临时空间，7 天内能放回去），所以用普通配色而不是
/// 红色危险框；但它会从一个位置拿走用户的整个应用，所以必须先把「拿走的到底是什么」
/// 一条条摆出来，而不是只报一个总数。
pub fn confirm_modal(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(pending) = state.pending_clean.as_ref() else {
        return;
    };
    // 这一页只认「卸载装出来的批次」。首页的清理批次也在这个字段里，
    // 但它在这一页画不出来（首页只在工作台页挂着自己的确认框）。
    let Some(app) = pending.items.first().and_then(|i| {
        state
            .uninstall
            .apps
            .iter()
            .find(|a| std::path::Path::new(&a.path) == i.path)
    }) else {
        return;
    };

    let admin = app.leftovers.iter().filter(|l| l.needs_admin).count();
    let mut note = format!(
        "本体与它留下的文件会一起移入临时空间，{} 天内随时可以还原。可用空间不会立刻变多。",
        crate::paths::DEFAULT_RETAIN_DAYS
    );
    if admin > 0 {
        // 先说清楚会失败的那几条：不说的结果是用户看到「3 项未处理」还得自己猜原因。
        note.push_str(&format!(
            "\n其中 {admin} 项在 /Library 下，需要管理员权限——这几项会失败，其余照常移走。"
        ));
    }

    let dialog = crate::components::ConfirmDialog::new(
        "molt-confirm-uninstall",
        crate::components::ConfirmKind::Normal,
        format!("卸载 {}？", app.name),
    )
    .summary(format!(
        "{} 项 · 共 {}",
        pending.count(),
        molt_scan::human_bytes(pending.bytes)
    ))
    .note(note)
    .confirm_label("确认卸载")
    .cancel_label("先不卸")
    .width(500.0);

    let outcome = dialog.show(ui, p, |ui| {
        let rows: Vec<(String, String)> = pending
            .items
            .iter()
            .map(|i| {
                let path = crate::paths::display_path_str(&i.path.display().to_string());
                (path, i.note.clone().unwrap_or_default())
            })
            .collect();
        crate::components::confirm_dialog::item_list(ui, p, false, &rows, None);
    });

    match outcome {
        crate::components::ConfirmOutcome::Confirmed => acts.push(Action::RunUninstall),
        crate::components::ConfirmOutcome::Cancelled => acts.push(Action::CancelUninstall),
        // 还开着的：什么都不发。用户可能只是在读清单。
        crate::components::ConfirmOutcome::Open => {}
    }
}

/// 清单是什么时候读的，说成人话。
///
/// 分钟以内不报数（「刚刚」），因为「0 分钟前」读起来像出错；
/// 超过一天就报到天——那时用户该重新扫描了，精确到分钟没有意义。
fn listed_ago(at: std::time::SystemTime) -> Option<String> {
    let secs = std::time::SystemTime::now()
        .duration_since(at)
        .ok()?
        .as_secs();
    Some(match secs {
        0..=59 => "刚刚".to_string(),
        60..=3599 => format!("{} 分钟前", secs / 60),
        3600..=86_399 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    })
}

#[cfg(test)]
mod tests {
    use super::listed_ago;

    /// 相对时间的分档必须连续：不能出现「60 分钟前」这种越界文案，
    /// 也不能在边界上返回 `None`（那会让页头整句消失）。
    #[test]
    fn relative_time_wording_covers_every_bucket() {
        let now = std::time::SystemTime::now();
        let back = |s: u64| now - std::time::Duration::from_secs(s);

        assert_eq!(listed_ago(back(0)).as_deref(), Some("刚刚"));
        assert_eq!(listed_ago(back(59)).as_deref(), Some("刚刚"));
        assert_eq!(listed_ago(back(60)).as_deref(), Some("1 分钟前"));
        assert_eq!(listed_ago(back(3_599)).as_deref(), Some("59 分钟前"));
        assert_eq!(listed_ago(back(3_600)).as_deref(), Some("1 小时前"));
        assert_eq!(listed_ago(back(86_400)).as_deref(), Some("1 天前"));
        // 时间戳落在未来（系统时钟被调回去）时不 panic，只是不给文案
        assert_eq!(listed_ago(now + std::time::Duration::from_secs(60)), None);
    }
}
