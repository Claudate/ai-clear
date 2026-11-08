//! 工作台：整个产品只有一条主线。
//!
//! 打开这一页要能立刻回答三件事，顺序就是版面的顺序：
//! 1. **现在该做什么**——一句结论 + 一个主按钮；
//! 2. **哪些建议清理、哪些留着**——按优先级分节，不是一长串按体积排的清单；
//! 3. **每一条到底是什么**——本地判定 + AI 解释同时挂在那一行上。
//!
//! AI 不再是独立的一页。它在扫描之后由用户点一次「让 AI 解释这次扫描」触发，
//! 结果直接落到每一条数据上；它失败也不会挡住本地扫描与手动清理。
//!
//! 互斥在这里体现成两件事：执行期间「勾选」与「动作」全部置灰并给出理由，
//! 而不是点了没反应。

use std::sync::Arc;

use eframe::egui;

use crate::action::Action;
use crate::components::Block;
use crate::state::{AppState, Filter, PendingItem, Priority, StatusKind, StatusMessage};
use crate::theme::Palette;
use crate::widgets::{
    badge, busy_bar, card, danger_text_button, page_header, primary_button, progress_bar,
    secondary_button, state_block,
};
use molt_act::CleanMode;
use molt_core::Verdict;

/// 工作台页面。只读 `state`，改动一律作为 `Action` 追加到 `acts`。
///
/// 这条签名就是这次重构的契约：页面拿不到 `&mut AppState`，所以它**不可能**
/// 绕过 `action::apply` 去改状态。于是「点了这一下应该变成什么样」只在
/// `apply` 里有一份定义，也就能被单测直接断言。
pub fn show(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    // 底栏先占位：主操作必须始终看得见，绝不能随清单滚出屏幕。
    //
    // 清单现在默认展开，底栏就跟常显——「选中 → 执行」是这一页最常走的一步，
    // 用户不该先把它找出来。
    let has_items = state
        .result
        .as_ref()
        .map(|r| !r.items.is_empty())
        .unwrap_or(false);
    if has_items {
        footer_bar(ui, state, p, acts);
    }

    // 底部操作栏还没占位时量一次：滚动区里的 `available_height` 拿不到真实窗口高度。
    let page_height = ui.available_height();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(ui, p);
            ui.add_space(12.0);

            scan_control_bar(ui, state, p, acts);
            ui.add_space(8.0);

            // 摘要取真实磁盘容量、可清理量和上次扫描时间；扫描没完成也能看见可用空间。
            summary(ui, state, p, acts);
            // 清完之后「这次一共腾出多少」要有持久证据：底部结果条会消失，
            // 而这是用户唯一能回头确认的数字。
            if state.session_freed > 0 {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "本次运行已腾出 {}",
                            molt_scan::human_bytes(state.session_freed)
                        ))
                        .size(p.sz(12.0))
                        .strong()
                        .color(p.reclaimable),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
            }
            ui.add_space(12.0);

            // 重复文件是独立功能，入口必须第一眼看得见。
            //
            // 它原先挂在清单末尾，而清单要「扫描过、且条目非空」才画出来——
            // 于是打开软件（还没有结果）、或这台机器没有可清缓存时，入口整个
            // 不存在，用户会以为功能被删了。放在摘要之下就与扫描状态无关了。
            duplicate_section(ui, state, p, acts);

            // 出错优先于「空」：读失败与「盘上很干净」在版面上长得像，
            // 但含义完全相反，绝不能画成一个。
            if let Some(err) = state.error.clone() {
                if state.result.is_none() {
                    failed_card(ui, state, p, &err, acts);
                    return;
                }
                // 旧结果还在：不丢掉它，但必须先说明它是旧的。
                stale_banner(ui, state, p, &err, acts);
            }

            let Some(result) = state.result.clone() else {
                if state.is_scanning() {
                    scanning_card(ui, state, p);
                } else {
                    hint_line(
                        ui,
                        p,
                        "还没有扫描结果。点上面的「重新扫描」开始——只读取文件信息，不会改动任何东西。",
                    );
                }
                return;
            };

            if result.items.is_empty() {
                // 重新扫描时旧结果可能正好是空的（新机器第一次扫完就是这样）。
                // 那时若继续画「没有找到可清理的内容」，用户按住「重新扫描」也
                // 看不到任何变化，会以为按钮没接上。
                if state.is_scanning() {
                    scanning_card(ui, state, p);
                    return;
                }
                hint_line(
                    ui,
                    p,
                    "没有找到可清理的内容：规则覆盖的目录目前都是空的，\
                     或者这些工具还没在这台 Mac 上产生缓存。",
                );
                return;
            }

            cached_note(ui, state, p);
            // 整页可用高度决定图能占多少：这一页先要能看见清单，图是第二眼。
            occupancy_ranking(ui, state, p, page_height, acts);
            ui.add_space(10.0);
            report_banner(ui, state, p, acts);
            // 清单不再折叠：这一页的主体就是它。
            list(ui, state, p, acts);

            // 底栏是固定面板，不属于滚动内容；末尾必须留出它的高度，
            // 否则最后一组重复文件或清单行会滚到底栏下面，用户看不到也点不到。
            ui.add_space(p.sz(96.0));
        });

    // 详情抽屉画在所有内容之上（Area，不占布局），Esc 或点暗处关闭。
    detail_panel(ui, state, p, acts);
}

fn header(ui: &mut egui::Ui, p: &Palette) {
    // 重扫操作与容量摘要同框，页头保留清晰的页面身份。
    page_header(
        ui,
        p,
        "清理",
        "安全释放 AI 工具缓存与构建产物占用的空间",
        |_ui| {},
    );
}

/// 顶部的摘要条：一条容量条 + 「可用 / 可清理」两个数字 + 重新扫描 + 状态。
///
/// 扫描进行中的控制条：暂停 / 继续 / 取消。
///
/// 只出现在扫描真正在跑且未被取消时。暂停后进度与已收集的候选保留，
/// 继续时从同一进度接着扫，不会从头再来。
fn scan_control_bar(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(job) = state.job.as_ref() else {
        return;
    };
    if job.is_finished() || job.control.is_cancelled() {
        return;
    }
    let paused = job.control.is_paused();
    card(ui, p, |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(if paused {
                        "扫描已暂停"
                    } else {
                        "正在扫描…"
                    })
                    .size(p.sz(12.5))
                    .strong()
                    .color(if paused { p.costly } else { p.reclaimable }),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 取消在**两种状态下都要有**。以前它只在暂停后才出现，
                // 于是「扫到一半不想扫了」的唯一出路是先点暂停、再点取消——
                // 一个纯粹多余而且不显眼的两步。用户按下的那一刻就是想停，
                // 不该再让他先做个他不想做的动作。
                if secondary_button(ui, p, "取消", true) {
                    acts.push(Action::CancelScan);
                }
                ui.add_space(6.0);
                if paused {
                    if primary_button(ui, p, "继续", true) {
                        acts.push(Action::ResumeScan);
                    }
                } else if secondary_button(ui, p, "暂停", true) {
                    acts.push(Action::PauseScan);
                }
            });
        });
    });
}

/// 空间占用图：面积等于实际占用，颜色等于判定，一眼看出「盘被谁占了」。
///
/// 只画得出可辨识标签的那些块；再往下的碎块并成一块「其他 N 项」——
/// 一像素宽的色块既写不下字，也点不准。精确到每一项的清单在页面下方，
/// 所以这里不必强行把所有条目塞进图里。
///
/// 块里带着原始下标：排序只决定面积，点击仍然打开那一条自己的详情。
fn occupancy_ranking(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    page_height: f32,
    acts: &mut Vec<Action>,
) {
    let Some(result) = state.result.as_ref() else {
        return;
    };
    let mut sorted: Vec<usize> = (0..result.items.len()).collect();
    sorted.sort_unstable_by_key(|&i| std::cmp::Reverse(size_of(&result.items[i])));
    let total: u64 = sorted.iter().map(|&i| size_of(&result.items[i])).sum();
    if total == 0 {
        return;
    }

    // 矮窗口整块不画：清单必须留在首屏，而图只是「盘被谁占了」的第一眼。
    // 阈值是窗口高度，**不乘字号缩放**——把 1.4 倍字号乘进去会变成 896，
    // 于是一个 800 高的窗口里这块永远不出现。
    if page_height < 640.0 {
        return;
    }

    let wide = ui.available_width() >= p.sz(520.0);
    // 块数上限与「值不值得单画一块」的下限都按窗口宽度分档：窄窗里同样一个
    // 百分比只剩几十像素宽，写不下字也点不准。
    // 块数压到 7/5 档：块越多，后面那些小块只剩一两条细缝，既写不下字
    // 也点不准；它们本来就该属于「其他」。
    let max_blocks = if wide { 7 } else { 5 };
    let min_share = if wide { 0.04 } else { 0.07 };
    let mut blocks: Vec<Block> = Vec::new();
    // blocks 与 sources 一一对应；「其他」块没有对应项，所以只在 sources 里留空。
    let mut sources: Vec<Option<usize>> = Vec::new();
    let mut merged_bytes = 0u64;
    let mut merged_count = 0usize;
    for &i in &sorted {
        let c = &result.items[i];
        let bytes = size_of(c);
        let share = bytes as f64 / total as f64;
        if blocks.len() < max_blocks && share >= min_share {
            blocks.push(Block {
                label: crate::paths::friendly_name(&state.home, &c.path),
                detail: format!(
                    "{}\n{}\n{}",
                    molt_scan::human_bytes(bytes),
                    c.path.display(),
                    c.verdict.explanation()
                ),
                bytes,
                color: p.verdict(&c.verdict),
            });
            sources.push(Some(i));
        } else {
            merged_bytes += bytes;
            merged_count += 1;
        }
    }
    if merged_count > 0 {
        blocks.push(Block {
            label: format!("其他 {merged_count} 项"),
            detail: format!(
                "合计 {}\n这些块太小，写不下标签；完整清单在下方",
                molt_scan::human_bytes(merged_bytes)
            ),
            bytes: merged_bytes,
            color: p.used_bar,
        });
        sources.push(None);
    }

    card(ui, p, |ui| {
        let heading = egui::RichText::new("空间占用排行")
            .size(p.sz(14.0))
            .strong()
            .color(p.text);
        if wide {
            ui.horizontal(|ui| {
                ui.label(heading);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 外面这层是从右往左排的，`ui.horizontal` 会继承这个方向。
                    // 必须显式指定左到右，否则四项被倒着画、圆点落到文字右边。
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ranking_legend(ui, p)
                    });
                });
            });
        } else {
            ui.label(heading);
            ui.add_space(p.sz(2.0));
            ranking_legend(ui, p);
        }
        ui.add_space(p.sz(6.0));
        // 高度随窗口走：默认 800 高的窗口里给 150，块内写得下名字和体积；
        // 刚过阈值的矮窗口里只给 92，别把清单挤下去。
        let base = if page_height >= 660.0 { 150.0 } else { 92.0 };
        let height = p.sz(if wide { base } else { base - 8.0 });
        if let Some(block) = crate::components::occupancy_map(ui, p, &blocks, height) {
            if let Some(Some(index)) = sources.get(block) {
                let path = result.items[*index].path.display().to_string();
                acts.push(Action::OpenDetail(path));
            }
        }
    });
}

/// 一条候选的实际占用。报「预计释放」一律用这个口径（见 `Candidate::alloc_bytes`）。
fn size_of(c: &molt_core::Candidate) -> u64 {
    c.alloc_bytes.max(c.bytes)
}

/// 图例顺序与色块判定色一致，避免把有代价的空间误读成「能清」。
fn ranking_legend(ui: &mut egui::Ui, p: &Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = p.sz(8.0);
        for (name, color) in [
            ("能清", p.reclaimable),
            ("清理有代价", p.costly),
            ("受保护", p.protected),
            ("其他", p.used_bar),
        ] {
            let (dot, _) =
                ui.allocate_exact_size(egui::vec2(p.sz(7.0), p.sz(7.0)), egui::Sense::hover());
            ui.painter()
                .circle_filled(dot.center(), dot.width() / 2.0, color);
            ui.label(egui::RichText::new(name).size(p.sz(10.5)).color(p.text_dim));
        }
    });
}

/// 重复文件检测区块：只展示与定位，绝不自动删除。
///
/// 这一块的第一职责是**说清它在干什么**。发现重复不意味着该删——哪些路径算
/// 「重复」、删哪个由用户判断，这里只把内容相同的一组文件列出来，并说明每组
/// 保留一份后能省多少，动作只有「访达定位」与「复制路径」两个。
///
/// 三种状态都要给足信息，不能只放一个按钮：
/// - 没跑过：说明它比对的是什么、会花多久、会不会动文件；
/// - 正在跑：当前阶段 + 已比对 / 待比对的数量，让人知道它不是卡住了；
/// - 有结果：每组算清「保留一份、其余可省」，而不是只丢一串路径出来。
fn duplicate_section(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let scanning = state
        .duplicate_job
        .as_ref()
        .map(|j| !j.is_finished())
        .unwrap_or(false);
    let scan = state.duplicate_scan.clone();

    card(ui, p, |ui| {
        // ---- 标题行：名字 +（右侧）当前状态与主操作 ----
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("重复文件")
                        .size(p.sz(14.0))
                        .strong()
                        .color(p.text),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if scanning {
                    // 扫描期间不画第二个启动按钮：点它什么也不会发生，
                    // 一个按不动的按钮比没有按钮更糟。
                    ui.add(egui::Spinner::new().size(p.sz(14.0)));
                } else if scan.is_some() {
                    if secondary_button(ui, p, "重新查找", true) {
                        acts.push(Action::StartDuplicateScan);
                    }
                } else if primary_button(ui, p, "查找重复文件", true) {
                    acts.push(Action::StartDuplicateScan);
                }
            });
        });

        ui.add_space(6.0);

        // ---- 常驻说明：这一段是入口能被看懂的前提，任何状态都留着 ----
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "找出内容完全相同的文件。只读取文件做比对，不删除、不移动，找到后由你自己决定留哪一份。",
                )
                .size(p.sz(12.0))
                .color(p.text_dim),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );

        // ---- 正在扫描：阶段 + 进度 + 已看了多少文件 ----
        if scanning {
            if let Some(job) = state.duplicate_job.as_ref() {
                ui.add_space(8.0);
                let (phase, done, total, seen) = match job.peek() {
                    crate::worker::DuplicateProgress::Working {
                        phase,
                        done,
                        total,
                        files_seen,
                    } => (*phase, *done, *total, *files_seen),
                    _ => (molt_scan::DuplicatePhase::Walking, 0, 0, 0),
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(phase.label())
                            .size(p.sz(12.0))
                            .strong()
                            .color(p.text),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.add_space(6.0);
                // 有分母才画进度条；没有分母时用脉冲条表示「在动」，不假装百分比。
                if total > 0 {
                    ui.add(
                        egui::ProgressBar::new((done as f32 / total as f32).clamp(0.0, 1.0))
                            .desired_height(p.sz(6.0))
                            .show_percentage(),
                    );
                } else {
                    ui.add(
                        egui::ProgressBar::new(0.0)
                            .desired_height(p.sz(6.0))
                            .animate(true),
                    );
                }
                ui.add_space(6.0);
                let detail = if total > 0 {
                    format!("已比对 {done} / {total} 个候选文件 · 共看过 {seen} 个文件")
                } else {
                    format!("已看过 {seen} 个文件")
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(detail)
                            .size(p.sz(11.5))
                            .color(p.text_dim),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.add_space(2.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "文件多时要跑一会儿。这一页关掉也不影响，扫描在后台继续。",
                        )
                        .size(p.sz(11.0))
                        .color(p.text_faint),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            return;
        }

        let Some(scan) = scan else {
            return; // 没跑过：标题 + 说明 + 按钮已经够了
        };

        ui.add_space(8.0);

        // ---- 被时间截断：先说清「没比完」，绝不报「没有重复」 ----
        if scan.truncated {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("没比完，下面的结果不是全部")
                        .size(p.sz(12.0))
                        .strong()
                        .color(p.costly),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "时间到了就收手了，还有一部分文件没比对过。这时说「没有重复」是错的；点「重新查找」可以再来一次。",
                    )
                    .size(p.sz(11.5))
                    .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(8.0);
        }

        if scan.groups.is_empty() {
            if !scan.truncated {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "没有发现内容完全相同的文件（共看过 {} 个文件）。",
                            scan.files_seen
                        ))
                        .size(p.sz(12.0))
                        .strong()
                        .color(p.text),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
            return;
        }

        // ---- 有结果：先给总量，再逐组展开 ----
        // 每组保留一份，其余都是可以省下的副本。
        let waste: u64 = scan
            .groups
            .iter()
            .map(|g| g.bytes.saturating_mul((g.files.len() - 1) as u64))
            .sum();
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} 组重复 · 每组保留一份的话，可省下 {} · 共看过 {} 个文件",
                    scan.groups.len(),
                    molt_scan::human_bytes(waste),
                    scan.files_seen
                ))
                .size(p.sz(12.0))
                .strong()
                .color(p.text),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
        if scan.skipped > 0 {
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "另有 {} 个文件读不了或比对时正在变动，已跳过。",
                        scan.skipped
                    ))
                    .size(p.sz(11.0))
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }

        // 最多列出 40 组，避免家目录里几千组时页面无限拉长。
        for (i, group) in scan.groups.iter().take(40).enumerate() {
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

            let each = group.bytes / group.files.len().max(1) as u64;
            let save = group.bytes.saturating_mul((group.files.len() - 1) as u64);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("第 {} 组", i + 1))
                            .size(p.sz(12.5))
                            .strong()
                            .color(p.text),
                    )
                    .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "{} 个文件 · 每个 {} · 保留一份可省 {}",
                                group.files.len(),
                                molt_scan::human_bytes(each),
                                molt_scan::human_bytes(save)
                            ))
                            .size(p.sz(11.5))
                            .color(p.text_dim),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
            });

            for (idx, f) in group.files.iter().enumerate() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    // 第一份标成「建议保留」：总得留一份，但这里只是建议，
                    // 没有替用户决定删除任何东西。
                    if idx == 0 {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("建议保留")
                                    .size(p.sz(10.5))
                                    .color(p.reclaimable),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("副本")
                                    .size(p.sz(10.5))
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(f.path.display().to_string())
                                .size(p.sz(11.5))
                                .color(p.text),
                        )
                        .truncate()
                        .wrap_mode(egui::TextWrapMode::Truncate),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if secondary_button(ui, p, "复制路径", true) {
                            acts.push(Action::CopyPath(f.path.display().to_string()));
                        }
                        if secondary_button(ui, p, "访达定位", true) {
                            acts.push(Action::RevealInFinder(f.path.display().to_string()));
                        }
                    });
                });
            }
        }
        if scan.groups.len() > 40 {
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "…还有 {} 组未列出（按占用从大到小排，前 40 组最值得看）。",
                        scan.groups.len() - 40
                    ))
                    .size(p.sz(11.0))
                    .color(p.text_faint),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
        }
    });
    ui.add_space(10.0);
}

/// 首页三项摘要；磁盘读数、可清理体积和上次扫描均来自当前状态，不使用设计图示例值。
fn summary(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let available = match &state.disk {
        Some(Ok(d)) => molt_scan::human_bytes(d.available),
        _ => "—".to_string(),
    };
    let (_, clean_bytes) = state.cleanable_summary();
    let cleanable = if state.result.is_some() {
        molt_scan::human_bytes(clean_bytes)
    } else {
        "—".to_string()
    };
    let last_scan = state
        .last_scan_at
        .as_ref()
        .map(|at| {
            if state.scanned_now {
                format!("今天 {at}")
            } else {
                format!("上次 {at}")
            }
        })
        .unwrap_or_else(|| {
            // 有结果却说「尚未扫描」会自相矛盾：下面明明列着一屏条目、
            // 底栏还写着「已选 N 项」。这一刻只知道它是早先扫出来的，
            // 那就如实说「早先一次扫描」，别让时间戳缺失变成「没扫过」。
            if state.result.is_some() {
                "早先一次扫描".to_string()
            } else {
                "尚未扫描".to_string()
            }
        });

    // 活动状态与上次扫描二选一：正在跑的时候，上一次的结果不再是重点。
    let act = state.activity();
    let activity = if state.clean_done_secs > 0.0 {
        // 清完会立刻触发重扫；先保留完成反馈，避免被「正在扫描」盖住。
        let amount = molt_scan::human_bytes(state.clean_total_bytes);
        if state.clean_was_delete {
            format!("清理完成 · 永久删除，释放 {amount}")
        } else {
            format!("清理完成 · 已移入临时空间 {amount}，可还原")
        }
    } else if let Some(label) = act.label() {
        let detail = match act {
            crate::state::Activity::Scanning => state.job.as_ref().and_then(|j| match j.peek() {
                crate::worker::Progress::Judging { done, total } if *total > 0 => {
                    Some(format!(" {done}/{total}"))
                }
                crate::worker::Progress::Walking { found } if *found > 0 => {
                    Some(format!(" 已找到 {found} 项"))
                }
                _ => None,
            }),
            crate::state::Activity::Cleaning => {
                state.clean_job.as_ref().and_then(|j| match j.peek() {
                    crate::worker::CleanProgress::Working { done, total } => Some(format!(
                        " {done}/{total} 项 · {:.0}%",
                        if *total == 0 {
                            0.0
                        } else {
                            *done as f32 / *total as f32 * 100.0
                        }
                    )),
                    _ => None,
                })
            }
            _ => None,
        };
        format!("{label}{}", detail.unwrap_or_default())
    } else {
        String::new()
    };

    let release = state.pending_release();
    let release_status = match release {
        Some((0, _)) => "待释放 0 项（临时空间为空）".to_string(),
        Some((count, bytes)) => format!(
            "待释放 {} · {count} 项（彻底删除后才释放）",
            molt_scan::human_bytes(bytes),
        ),
        None if state.quarantine_loading => "正在读取临时空间待释放量…".to_string(),
        None => format!(
            "待释放读取失败：{}；请到临时空间点刷新",
            state.quarantine_error.as_deref().unwrap_or("原因未知"),
        ),
    };
    let release_color = if release.is_some_and(|(n, _)| n > 0) {
        p.costly
    } else {
        p.text_dim
    };

    let summary_width = ui.available_width();
    egui::Frame::NONE
        .fill(p.surface_alt)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CARD))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_min_width(summary_width - 24.0);
            let wide = ui.available_width() >= p.sz(540.0);
            let very_narrow = ui.available_width() < p.sz(380.0);
            let button_width = p.sz(94.0);
            let slot_width = if wide {
                ((ui.available_width() - button_width - p.sz(48.0)) / 3.0).max(1.0)
            } else {
                ((ui.available_width() - p.sz(30.0)) / 3.0).max(1.0)
            };
            if very_narrow {
                for (label, value, tint) in [
                    ("可用", available.as_str(), p.text),
                    ("可清理", cleanable.as_str(), p.reclaimable),
                    ("上次扫描", last_scan.as_str(), p.text),
                ] {
                    summary_metric(ui, p, label, value, tint, ui.available_width());
                }
            } else {
                ui.horizontal(|ui| {
                    summary_metric(ui, p, "可用", &available, p.text, slot_width);
                    summary_divider(ui, p);
                    summary_metric(ui, p, "可清理", &cleanable, p.reclaimable, slot_width);
                    summary_divider(ui, p);
                    summary_metric(ui, p, "上次扫描", &last_scan, p.text, slot_width);
                    if wide {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if rescan_button(
                                ui,
                                p,
                                button_width,
                                state.can_start(),
                                state.is_scanning(),
                            ) {
                                acts.push(Action::FollowNextStep(crate::state::NextStep::Retry));
                            }
                        });
                    }
                });
            }
            if !wide {
                ui.add_space(p.sz(5.0));
                if rescan_button(ui, p, button_width, state.can_start(), state.is_scanning()) {
                    acts.push(Action::FollowNextStep(crate::state::NextStep::Retry));
                }
            }
        });
    // 从临时空间彻底删除才实际释放磁盘；这条状态不能被「可清理」数字遮掉。
    ui.add_space(p.sz(3.0));
    ui.horizontal_wrapped(|ui| {
        if !activity.is_empty() {
            ui.label(egui::RichText::new(&activity).size(p.sz(10.5)).color(
                if state.clean_done_secs > 0.0 {
                    p.reclaimable
                } else {
                    p.costly
                },
            ));
            ui.add_space(p.sz(10.0));
        }
        ui.label(
            egui::RichText::new(release_status)
                .size(p.sz(10.5))
                .color(release_color),
        );
    });
}

/// 统一三项指标的字阶，窄宽时由外层决定并排或逐行显示。
fn summary_metric(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    value: &str,
    tint: egui::Color32,
    width: f32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, p.sz(44.0)), egui::Sense::hover());
    ui.painter().text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(p.sz(10.5)),
        p.text_dim,
    );
    ui.painter().text(
        rect.left_top() + egui::vec2(0.0, p.sz(15.0)),
        egui::Align2::LEFT_TOP,
        value,
        egui::FontId::proportional(p.sz(18.0)),
        tint,
    );
}

/// 指标之间只用细线分区，不再以三张小卡浪费宽度。
fn summary_divider(ui: &mut egui::Ui, p: &Palette) {
    let (line, _) = ui.allocate_exact_size(egui::vec2(1.0, p.sz(40.0)), egui::Sense::hover());
    ui.painter().vline(
        line.center().x,
        line.y_range(),
        egui::Stroke::new(1.0, p.border_strong),
    );
}

/// 重扫保持旧 Action 链，以便扫描中禁用、失败后重试的反馈完全不变。
fn rescan_button(
    ui: &mut egui::Ui,
    p: &Palette,
    width: f32,
    can_start: bool,
    scanning: bool,
) -> bool {
    let text = if scanning {
        "扫描中…"
    } else {
        "重新扫描"
    };
    let btn = egui::Button::new(egui::RichText::new(text).size(p.sz(11.5)).strong().color(
        if can_start {
            p.reclaimable
        } else {
            p.text_faint
        },
    ))
    .min_size(egui::vec2(width, p.sz(30.0)))
    .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_CONTROL))
    .fill(p.surface)
    .stroke(egui::Stroke::new(
        1.0,
        if can_start { p.reclaimable } else { p.border },
    ));
    ui.add_enabled(can_start, btn).clicked()
}

/// 一行可点的文字链：给筛选留一条「显示全部」的出口。
///
/// 筛选切到某一档后，用户得有一条回到全局的路，而不是再去分段控件里找。
fn link(ui: &mut egui::Ui, p: &Palette, text: &str, color: egui::Color32) -> bool {
    ui.add(egui::Link::new(
        egui::RichText::new(text).size(p.sz(11.5)).color(color),
    ))
    .clicked()
}

fn report_banner(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let action = crate::widgets::op_report(ui, p, state.last_op.as_deref());
    match action {
        crate::widgets::ReportAction::None => {}
        crate::widgets::ReportAction::Dismiss => acts.push(Action::DismissReport),
        crate::widgets::ReportAction::Next(next) => acts.push(Action::FollowNextStep(next)),
    }
    if state.last_op.is_some() {
        ui.add_space(10.0);
    }
}

/// 显示的是上次留下的结果时，说明一句。
///
/// 不声不响地摆一堆数字，用户会默认它们是最新的——而这些数字直接引出
/// 删除动作。一行灰字就够了，不用横幅：它不阻断任何操作。
fn cached_note(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    // 只有「已经有结果、但这份结果不是这次扫出来的」才需要说明。
    if state.result.is_none() || state.scanned_now {
        return;
    }
    let at = state.last_scan_at.as_deref().unwrap_or("上次");
    hint_line(
        ui,
        p,
        &format!("上面是{at}那次扫描的结果，要更新就点「重新扫描」。"),
    );
    ui.add_space(6.0);
}

/// 重扫失败但旧结果还在时，先说清「下面这些是什么时候的」。
///
/// 不这么做的话，用户看到的是上一次成功的数字，却以为刚扫过——而清理
/// 是按这些数字做的决定，代价可能是一次不必要的重建。
fn stale_banner(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    err: &str,
    acts: &mut Vec<Action>,
) {
    // 框体交给 `components::banner`，这里只决定说什么。
    let detail = match &state.last_scan_at {
        Some(at) => format!("下面仍是 {at} 那次的结果"),
        None => "下面是一份较早的结果".to_string(),
    };
    if crate::components::banner::banner(
        ui,
        p,
        &format!("这次扫描失败：{err}"),
        &detail,
        Some("重试"),
        state.can_start(),
    ) {
        acts.push(Action::FollowNextStep(crate::state::NextStep::Retry));
    }
    ui.add_space(10.0);
}

/// 仪表盘下方的一行说明。
///
/// 空态不该再用一整块居中提示占掉半屏：仪表盘已经把「还剩多少」讲清楚了，
/// 这里只需要补一句「为什么清单是空的」。
fn hint_line(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.add(
        egui::Label::new(egui::RichText::new(text).size(p.sz(12.0)).color(p.text_dim))
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
}

/// 分档清单：首页的主体，默认展开。
///
/// 首页要回答的是「哪些能清、哪些要确认」——答案就是这张清单本身，
/// 不该再点一次才出现。顶部一行分段筛选，想只看某一档时点一下就换；
/// AI 解释收进这一行右侧的一个按钮，因为它是可选动作。
fn list(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(result) = state.result.clone() else {
        return;
    };
    let busy = state.activity();

    toolbar(ui, state, p, &busy, acts);
    ui.add_space(10.0);

    // hero 里的「要重建 / 是你的记录」两个入口只设了筛选，用户点了如果没滚动，
    // 会觉得「什么都没发生」——清单在屏幕外。这里把视图移到清单开头。
    if state.scroll_to_list {
        acts.push(Action::ConsumeScrollTo);
        ui.scroll_to_rect(ui.max_rect(), Some(egui::Align::TOP));
    }

    let visible = state.visible_items();
    if visible.is_empty() {
        empty_list(ui, state, p, &result, acts);
        return;
    }

    let mut toggle: Option<String> = None;
    // 沿用整页滚动；大量条目只为可见行组装字体与交互，视口外仍保留等高占位。
    let paint_rect = ui.clip_rect().expand(p.sz(120.0));

    let groups = [
        (Priority::Suggested, p.costly),
        (Priority::Safe, p.reclaimable),
        (Priority::Costly, p.costly),
        (Priority::Keep, p.protected),
    ];

    for (pr, tint) in groups {
        let rows: Vec<usize> = visible
            .iter()
            .copied()
            .filter(|&i| state.priority_of(&result.items[i]) == pr)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let bytes: u64 = rows
            .iter()
            .map(|&i| result.items[i].alloc_bytes.max(result.items[i].bytes))
            .sum();

        let group_state = state.group_check(pr);
        let name = format!(
            "{}，{} 项，{}",
            pr.label(),
            rows.len(),
            match group_state {
                crate::components::checkbox::TriState::Checked => "已全选",
                crate::components::checkbox::TriState::Partial => "部分选中",
                crate::components::checkbox::TriState::Locked => "不可选",
                crate::components::checkbox::TriState::Unchecked => "未选中",
            }
        );
        if crate::widgets::section_header_checkable(
            ui,
            p,
            pr.label(),
            pr.hint(),
            rows.len(),
            bytes,
            tint,
            Some((
                group_state,
                state.can_select() && group_state != crate::components::checkbox::TriState::Locked,
            )),
            &name,
        ) {
            acts.push(Action::ToggleGroup(pr));
        }

        // 同一档再按项目或工具归并。路径只用于命名，不把父目录当成待清理对象；
        // 真正执行的仍是扫描器逐项判定过的叶子候选。
        let mut buckets = std::collections::BTreeMap::<String, (String, Vec<usize>, u64)>::new();
        for i in rows {
            let c = &result.items[i];
            let (key, title) = source_group(&state.home, c);
            let bucket = buckets.entry(key).or_insert_with(|| (title, Vec::new(), 0));
            bucket.1.push(i);
            bucket.2 = bucket.2.saturating_add(c.alloc_bytes.max(c.bytes));
        }
        let mut buckets: Vec<_> = buckets.into_iter().collect();
        buckets.sort_by(|a, b| b.1 .2.cmp(&a.1 .2).then_with(|| a.1 .0.cmp(&b.1 .0)));
        for (key, (title, items, size)) in buckets {
            let selectable: Vec<_> = items
                .iter()
                .filter(|&&i| AppState::item_actionable(&result.items[i]))
                .collect();
            let selected = selectable
                .iter()
                .filter(|&&&i| state.is_checked(&result.items[i].path.display().to_string()))
                .count();
            use crate::components::checkbox::TriState;
            let selection = if selectable.is_empty() {
                TriState::Locked
            } else if selected == 0 {
                TriState::Unchecked
            } else if selected == selectable.len() {
                TriState::Checked
            } else {
                TriState::Partial
            };
            let expanded = !state.collapsed_groups.contains(&(pr, key.clone()));
            let (open_clicked, select_clicked) = crate::widgets::subgroup_header(
                ui,
                p,
                &title,
                items.len(),
                size,
                expanded,
                selection,
                state.can_select() && selection != TriState::Locked,
            );
            if open_clicked {
                acts.push(Action::ToggleSubgroupOpen(pr, key));
            }
            if select_clicked {
                acts.push(Action::ToggleSubgroupSelection(
                    pr,
                    items
                        .iter()
                        .map(|&i| result.items[i].path.display().to_string())
                        .collect(),
                ));
            }
            if !expanded {
                continue;
            }
            // 保留整页的一个滚动条，视口外的行只占原来的空间。
            for i in items {
                let height = row_height(state, p, &result.items[i]);
                let top = ui.cursor().top();
                if visible.len() > 250
                    && (top + height < paint_rect.top() || top > paint_rect.bottom())
                {
                    ui.allocate_space(egui::vec2(ui.available_width(), height));
                    continue;
                }
                match row(ui, state, p, &result, i) {
                    Some(RowOutcome::Toggle) => {
                        toggle = Some(result.items[i].path.display().to_string())
                    }
                    Some(RowOutcome::Detail) => acts.push(Action::OpenDetail(
                        result.items[i].path.display().to_string(),
                    )),
                    None => {}
                }
            }
            ui.add_space(p.sz(6.0));
        }
        ui.add_space(p.sz(12.0));
    }

    if let Some(path) = toggle {
        acts.push(Action::ToggleCheck(path));
    }
}

/// 把叶子候选映射到可读的项目/工具名；分组本身不参与清理判定。
fn source_group(home: &str, c: &molt_core::Candidate) -> (String, String) {
    let path = c.path.to_string_lossy();
    let relative = path
        .strip_prefix(home)
        .map(|p| p.trim_start_matches('/'))
        .or_else(|| path.strip_prefix("~/"))
        .unwrap_or(&path);
    let parts: Vec<&str> = relative.split('/').filter(|p| !p.is_empty()).collect();
    let leaf = parts.last().copied().unwrap_or("");
    if parts.len() >= 2 && matches!(leaf, "target" | "node_modules" | ".venv" | "build" | "dist") {
        let parent = parts[parts.len() - 2];
        return (
            format!("project:{}", parts[..parts.len() - 1].join("/")),
            format!("项目 · {parent}"),
        );
    }
    if parts.len() >= 2 && matches!(parts[0], "project" | "projects" | "Projects") {
        return (
            format!("project:{}/{}", parts[0], parts[1]),
            format!("项目 · {}", parts[1]),
        );
    }
    if parts.len() >= 2 && parts[0] == "Library" && parts[1] == "Caches" {
        if parts.len() > 3 {
            return (
                format!("cache:{}", parts[2]),
                format!("应用缓存 · {}", parts[2]),
            );
        }
        return ("cache:general".into(), "其他应用缓存".into());
    }
    if let Some(tool) = parts.first().filter(|name| name.starts_with('.')) {
        let tool = tool.trim_start_matches('.');
        return (format!("tool:{tool}"), format!("工具 · {tool}"));
    }
    let root = parts.first().copied().unwrap_or("其他位置");
    (format!("location:{root}"), format!("位置 · {root}"))
}

/// 清单上方的工具条：筛选分段 + 搜索 + AI 解释入口。
fn toolbar(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    busy: &crate::state::Activity,
    acts: &mut Vec<Action>,
) {
    let (clean_n, clean_bytes) = state.cleanable_summary();
    let (costly_n, _) = state.summary_of(Priority::Costly);
    let (keep_n, _) = state.summary_of(Priority::Keep);
    // 「久未使用」的数字必须和点进去看到的行数同源：筛选按 `suggest_clean` 判
    // （见 `visible_items`），这里就直接数这个标记，不能再拿某一档 `Priority`
    // 的项数冒充——那样标签上的数字和列表长度会对不上。
    let recent_n = state.suggest_clean_count();
    // 「全部」这一档列出的是「会被移动的东西」——保留项不在其中（见
    // `visible_items` 的 `Filter::All`）。标签必须用同一个口径，
    // 否则会写成「全部 47」而点进去只有 43 行，用户会以为清单漏了东西。
    let total_n = clean_n + costly_n;

    ui.horizontal_wrapped(|ui| {
        let opts = [
            (
                format!(
                    "能清 {clean_n} 项 · {}",
                    molt_scan::human_bytes(clean_bytes)
                ),
                state.filter == Filter::Cleanable,
            ),
            (format!("要确认 {costly_n}"), state.filter == Filter::Costly),
            (format!("保留 {keep_n}"), state.filter == Filter::Protected),
            (
                format!("久未使用 {recent_n}"),
                state.filter == Filter::Recent,
            ),
            (format!("全部 {total_n}"), state.filter == Filter::All),
        ];
        let refs: Vec<(&str, bool)> = opts.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        if let Some(i) = crate::widgets::segmented(ui, p, &refs) {
            acts.push(Action::SetFilter(match i {
                0 => Filter::Cleanable,
                1 => Filter::Costly,
                2 => Filter::Protected,
                3 => Filter::Recent,
                _ => Filter::All,
            }));
        }

        ui.add_space(8.0);
        // 搜索框用**草稿**而不是 `state.query`：视图只有 `&AppState`，
        // 拿不到 `&mut String`。输入先进草稿，回车或失焦时再作为动作提交。
        let mut draft = state.query_draft.clone();
        let resp = ui
            .add(
                egui::TextEdit::singleline(&mut draft)
                    .hint_text(
                        egui::RichText::new("搜索名字或路径")
                            .size(p.sz(12.0))
                            .color(p.text_faint),
                    )
                    .desired_width(200.0 * p.scale)
                    .margin(egui::Margin::symmetric(8, 6)),
            )
            .accessible_name("搜索名字或路径");
        if resp.changed() {
            // 边打边过滤：以前草稿只落在 `query_draft`，要等回车或失焦才提交，
            // 于是「框里已经有字、列表却没过滤」可以一直挂着——切页再回来
            // 更明显，用户会以为搜索坏了。过滤本身是每帧都在做的 O(n) 扫，
            // 边打边算不多花什么。
            acts.push(Action::SetQueryDraft(draft.clone()));
            acts.push(Action::SetQuery(draft.clone()));
        }
        if resp.lost_focus() {
            acts.push(Action::CommitQuery);
        }
        // 「清空」跟着框里的内容走，而不是跟着已提交的查询走：
        // 打了一半还没提交时框里有字，也该能一键清掉。
        if (!state.query_draft.is_empty() || !state.query.is_empty())
            && secondary_button(ui, p, "清空", true)
        {
            acts.push(Action::ClearQuery);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ai_action(ui, state, p, busy, acts);
        });
    });

    // 「显示全部」必须**单独占一行**。
    //
    // 它以前和分段控件挤在同一个 `horizontal_wrapped` 里。`segmented` 内部是一层
    // `Frame`，它在布局里报的高度远大于文字可见的那一行；于是「显示全部」这颗链
    // 的文字被画出来、热区却被分段控件整片盖住——实测整段文字范围内点下去，
    // 命中的都是「能清」那一档。用户切了筛选之后，唯一一条回「全部」的路是死的。
    if state.filter != Filter::All {
        ui.add_space(2.0);
        if link(ui, p, "显示全部", p.reclaimable) {
            acts.push(Action::SetFilter(Filter::All));
        }
    }

    if busy.is_busy() {
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(busy.blocking_hint())
                    .size(p.sz(10.5))
                    .color(p.costly),
            )
            .wrap_mode(egui::TextWrapMode::Wrap),
        );
    }
}

/// AI 解释：一个按钮 + 一行状态，不再是一张独立的卡片。
fn ai_action(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    busy: &crate::state::Activity,
    acts: &mut Vec<Action>,
) {
    if state.ai.is_running() {
        badge(ui, p, "AI 正在解释…", p.costly, p.costly_soft);
        busy_bar(ui, p, p.sz(90.0));
        return;
    }
    if state.ai.has_notes() {
        badge(
            ui,
            p,
            &format!("AI 已解释 {} 条", state.ai.notes.len()),
            p.reclaimable,
            p.accent_soft,
        );
        ui.add_space(4.0);
    }
    let resolved = state.ai.resolve();
    let label = if state.ai.has_notes() {
        "重新解释"
    } else {
        "让 AI 解释"
    };
    let ready = state.can_start() && resolved.is_ok();
    if secondary_button(ui, p, label, ready) {
        acts.push(Action::Analyze);
    }
    // 置灰必须说明为什么：一个点不动又不解释的按钮，用户只会以为软件坏了。
    // 这一行只在真的点不了时出现，正常状态下不占版面。
    //
    // 文案分两种：已经解释过、但配置现在读不出来（改过设置、密钥删了）时，
    // 不能写「未配置」——那和左边「已解释 24 条」直接打架，用户会以为
    // 那些解释是假的。说清是「现在重新解释不了」才准确。
    if !resolved.is_ok() && !state.ai.is_running() {
        let (hint, fix) = if state.ai.has_notes() {
            ("当前配置不可用，无法重新解释", "去设置")
        } else {
            ("AI 未配置，需要先填模型与密钥", "去设置")
        };
        if secondary_button(ui, p, fix, true) {
            acts.push(Action::GoSettingsForError);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(hint)
                    .size(p.sz(10.5))
                    .color(p.text_faint),
            )
            .wrap_mode(egui::TextWrapMode::Truncate),
        );
    }
    if let Some(f) = &state.ai.failure {
        if !state.ai.is_running() {
            // 顺序：原因在左、动作在右。这段画在 right_to_left 布局里，
            // 所以先加的那个反而靠右——先加按钮，用户才会先读到原因。
            if f.fixable_by_config && secondary_button(ui, p, "去设置", true) {
                acts.push(Action::GoSettingsForError);
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("上次失败：{}", crate::app::short(&f.message, 34)))
                        .size(p.sz(10.5))
                        .color(p.danger),
                )
                .wrap_mode(egui::TextWrapMode::Truncate),
            );
        }
    }
    // 未配置时什么都不画：首页要回答的是「还剩多少、能清多少、点哪个」，
    // 「AI 未配置」是设置页的事。这里挂一句尾巴，只会让人以为清理被挡住了。
    let _ = busy;
}

/// 清单为空时的三种不同情况，各自说清是哪一种。
fn empty_list(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    result: &crate::worker::ScanResult,
    acts: &mut Vec<Action>,
) {
    if !state.query.is_empty() {
        state_block(
            ui,
            p,
            "○",
            "没有匹配的内容",
            "换个关键词，或者清掉搜索框看全部。",
            p.text_faint,
        );
        if secondary_button(ui, p, "清空搜索", true) {
            acts.push(Action::ClearQuery);
        }
        return;
    }

    // 「全部受保护」是一种真实且常见的结果，不能画成「什么都没找到」
    let kept = state.count_of(Priority::Keep);
    if kept > 0 {
        let bytes: u64 = result
            .items
            .iter()
            .filter(|c| state.priority_of(c) == Priority::Keep)
            .map(|c| c.alloc_bytes.max(c.bytes))
            .sum();
        card(ui, p, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("这一档里没有内容，扫到的都在「保留」里")
                        .size(p.sz(14.0))
                        .strong(),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{} 项、共 {}：都是你的记录、密钥或正在使用的内容，不会被移动。",
                        kept,
                        molt_scan::human_bytes(bytes)
                    ))
                    .size(p.sz(11.5))
                    .color(p.text_dim),
                )
                .wrap_mode(egui::TextWrapMode::Wrap),
            );
            ui.add_space(8.0);
            if secondary_button(ui, p, "看看这些是什么", true) {
                acts.push(Action::SetFilter(Filter::Protected));
            }
        });
        return;
    }

    state_block(
        ui,
        p,
        "○",
        "这一档里没有内容",
        "换一个分段看，或者重新扫描一次。",
        p.text_faint,
    );
}

/// AI 解释最多两行。
///
/// 超过两行就裁掉：第三行开始通常是重复的修饰，而一屏多滚半屏的代价
/// 是用户看不到下面那些条目。行高按「两行字 + 内边距」固定，不做单行截断——
/// 两行正好够说清「这是什么」。
fn ai_note_clamped(ui: &mut egui::Ui, p: &Palette, text: &str) {
    let w = ui.available_width();
    let h = p.sz(36.0);
    // 先占位，再在**这块矩形**里画。
    //
    // 不能用 `allocate_ui_with_layout`：它给子 ui 的 `max_rect` 会被布局扩展，
    // 裁剪矩形跟着变大，第三行就漏到行的下面去了（清单滚到底时能看见它
    // 越过底栏）。这里把 max_rect 钉死成占位矩形，多出来的文字直接被裁掉。
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    badge(&mut child, p, "AI", p.costly, p.costly_soft);
    child.add_space(p.sz(4.0));
    child.add(
        egui::Label::new(egui::RichText::new(text).size(p.sz(10.5)).color(p.text_dim))
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
}

/// 一行上可能发生的两种点击。
///
/// 勾选与「详情」是两个不同的动作，行本身只能返回一个值，所以用一个枚举
/// 把「点了哪里」带出去，由调用方统一翻成 `Action`。
enum RowOutcome {
    Toggle,
    Detail,
}

/// 清单行的固定高度，同时用于真实绘制和视口外占位。
fn row_height(state: &AppState, p: &Palette, c: &molt_core::Candidate) -> f32 {
    if state.ai_note(&c.path.display().to_string()).is_some() {
        p.sz(101.0)
    } else {
        p.sz(82.0)
    }
}

/// 一行数据。返回这一下点在了哪里（由调用方统一改状态，避免同帧多次借用）。
///
/// 行内三层信息，从上到下：
/// - 名字 + 判定徽章
/// - 路径（缩写成 `~/...`）
/// - **本地判定为什么这样** + **AI 说它是什么**——这两句是这次重做的重点，
///   用户不该再为了「这是什么」去另一个页面
fn row(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    result: &crate::worker::ScanResult,
    i: usize,
) -> Option<RowOutcome> {
    let c = &result.items[i];
    let path = c.path.display().to_string();
    let pr = state.priority_of(c);
    let color = p.verdict(&c.verdict);
    let soft = p.verdict_soft(&c.verdict);
    let actionable = AppState::item_actionable(c);
    let checked = state.is_checked(&path);
    let note = state.ai_note(&path);
    let selectable = state.can_select() && actionable;

    let full_w = ui.available_width();
    // 逐项说明只在「这一项自己的理由和分节标题不同」时才画。
    // 可回收档那句固定文案已经由分节标题统一讲过，十一行重复同一句是噪声；
    // 「有代价」那些要重下什么、重编多久是逐项不同的，省掉等于让用户盲选。
    let why = c.verdict.explanation();
    let has_why = why != Verdict::Reclaimable.explanation();
    // 行高只有两档：一行字与「一行字 + 两行解释」。清单默认展开后，
    // 一屏能多读几行比每行都长得像卡片更重要。
    let min_h = row_height(state, p, c);

    let frame = egui::Frame::NONE
        // 选中只改底色：再叠一圈蓝描边，十一项连起来就是一整片蓝，
        // 蓝色也就不再表示「这里可交互」了。
        //
        // 不再给每行画圆角边框：一行一张卡片会把清单读成一叠卡片，行与行
        // 之间全是缝；列表靠一条 1 点分隔线连成一张表就够了。
        .fill(if checked {
            p.accent_soft
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(14, 4));

    // 「详情」按钮是否被点中。它要跨出 Frame 的闭包，所以先声明在外。
    let mut detail_clicked = false;
    let resp = frame
        .show(ui, |ui| {
            ui.set_width(full_w - 30.0);
            ui.set_min_height(min_h - 16.0);
            ui.horizontal(|ui| {
                crate::components::checkbox::checkbox(
                    ui,
                    p,
                    checked,
                    actionable,
                    state.can_select(),
                );
                ui.add_space(4.0);

                let size_w = p.sz(92.0);
                let left_w = (ui.available_width() - size_w - 12.0).max(p.sz(160.0));
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, min_h - 16.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        // 名字、路径、逐项说明挤在同一行：这是「一屏能读几条」
                        // 的关键。分两行写时，大字号（1.4 倍）下光是两行文字
                        // 加内边距就有 104 px，十一项要滚三屏。
                        // 说明只在「这一项自己的理由和分节标题不同」时才画：
                        // 可回收档那句固定文案已经由分节标题统一讲过了。
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 2.0);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(crate::paths::friendly_name(
                                        &state.home,
                                        &c.path,
                                    ))
                                    .size(p.sz(12.5))
                                    .strong()
                                    .color(p.text),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                            badge(ui, p, pr.label(), color, soft);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(crate::paths::display_in(
                                        &state.home,
                                        &c.path,
                                    ))
                                    .size(p.sz(10.0))
                                    .monospace()
                                    .color(p.text_faint),
                                )
                                .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                            if has_why {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&why).size(p.sz(10.5)).color(color),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Truncate),
                                );
                            }
                        });
                        // AI 解释：它是什么。直接挂在这一行上，最多两行。
                        if let Some(n) = note {
                            ai_note_clamped(ui, p, n);
                        }
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(size_w, min_h - 16.0),
                        egui::Layout::top_down(egui::Align::Max),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(molt_scan::human_bytes(
                                        c.alloc_bytes.max(c.bytes),
                                    ))
                                    .size(p.sz(13.5))
                                    .strong()
                                    .color(color),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                            if c.suggest_clean {
                                if let Some(d) = idle_days(c) {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("{d} 天未用"))
                                                .size(p.sz(10.0))
                                                .color(p.text_faint),
                                        )
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                    );
                                }
                            }
                            // 详情入口：一行放不下的完整路径、判定理由与 AI 解释
                            // 都在抽屉里。小字放在体积下面，不跟勾选抢注意力。
                            //
                            // 用无框按钮而不是带 Sense 的 Label：Label 的命中矩形
                            // 跟着文字走，在大字号下窄得几乎点不到。
                            let detail = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("详情")
                                        .size(p.sz(10.5))
                                        .color(p.text_faint),
                                )
                                .frame(false)
                                .min_size(egui::vec2(p.sz(48.0), p.sz(20.0))),
                            );
                            if detail.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            detail_clicked = detail.clicked();
                        },
                    );
                });
            });
        })
        .response;
    // 行与行之间的 1 点分隔线：列表靠它连成一张表，而不是靠每行自己的边框。
    ui.painter().hline(
        resp.rect.left()..=resp.rect.right(),
        resp.rect.bottom(),
        egui::Stroke::new(1.0, p.row_separator),
    );
    if actionable {
        // 行末那一列留给「详情」：整行点击若把它也盖住，后注册的交互会
        // 吃掉按钮的 click，详情就永远打不开。
        let mut hit_rect = resp.rect;
        hit_rect.max.x -= p.sz(52.0);
        let hit = ui.interact(
            hit_rect,
            ui.scope_id().with(("wb-row", i)),
            egui::Sense::click(),
        );
        hit.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::CheckBox,
                selectable,
                checked,
                format!(
                    "{}，{}，{}",
                    crate::paths::friendly_name(&state.home, &c.path),
                    pr.label(),
                    molt_scan::human_bytes(c.alloc_bytes.max(c.bytes))
                ),
            )
        });
        if selectable && hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if detail_clicked {
            return Some(RowOutcome::Detail);
        }
        if selectable && hit.clicked() {
            return Some(RowOutcome::Toggle);
        }
        return None;
    }
    // 保留项不能勾选，但它的详情仍然值得看。
    detail_clicked.then_some(RowOutcome::Detail)
}

/// 首页的详情抽屉：一行放不下的信息（完整路径、大小、判定理由、AI 解释）。
///
/// 只读。抽屉打开时清单仍然可见、可勾——用户常常一边看某一项的简介，
/// 一边对照着勾别的行。动作入口留在清单和底栏，这里不放执行按钮。
fn detail_panel(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(path) = state.detail_path.clone() else {
        return;
    };
    let Some(c) = state.result.as_ref().and_then(|r| {
        r.items
            .iter()
            .find(|c| c.path.display().to_string() == path)
    }) else {
        // 重扫描之后这一项可能不在了：抽屉自己关上，别停在一份过期数据上。
        acts.push(Action::CloseDetail);
        return;
    };

    let pr = state.priority_of(c);
    let mut rows: Vec<(String, String)> = vec![
        ("路径".to_string(), path.clone()),
        (
            "大小".to_string(),
            molt_scan::human_bytes(c.alloc_bytes.max(c.bytes)),
        ),
        ("判定理由".to_string(), c.verdict.explanation().to_string()),
    ];
    if let Some(d) = idle_days(c) {
        rows.push(("最近使用".to_string(), format!("{d} 天前")));
    }
    let mut matched_rule = false;
    for evidence in &c.evidence {
        if let molt_core::Evidence::RuleHit { rule_id, pattern } = evidence {
            rows.push(("命中规则".to_string(), format!("{rule_id} · {pattern}")));
            matched_rule = true;
        }
    }
    if !matched_rule {
        if let Some(rule_id) = &c.rule_id {
            rows.push(("命中规则".to_string(), rule_id.clone()));
        }
    }
    rows.push((
        "本次勾选状态".to_string(),
        if state.is_checked(&path) {
            "已勾选".to_string()
        } else {
            "未勾选".to_string()
        },
    ));

    let title = crate::paths::friendly_name(&state.home, &c.path);
    let note = state.ai_note(&path);
    let outcome = crate::widgets::detail_drawer(
        ui.ctx(),
        p,
        &title,
        (
            pr.label(),
            p.verdict(&c.verdict),
            p.verdict_soft(&c.verdict),
        ),
        &rows,
        note,
        true,
    );
    match outcome {
        crate::widgets::DetailDrawerOutcome::None => {}
        crate::widgets::DetailDrawerOutcome::Close => acts.push(Action::CloseDetail),
        crate::widgets::DetailDrawerOutcome::Reveal => acts.push(Action::RevealInFinder(path)),
    }
}

/// 窗口底部固定操作条：**主操作不随清单滚动**。
///
/// 旧版把「清理这些」埋在首页那张大卡片中间，下面还接着 AI 条与几百行清单；
/// 用户往下翻一眼清单，那个按钮就再也看不到了，只能往回滚。
/// 清理工具里「选中 → 执行」是最常走的一步，它必须一直在手边。
fn footer_bar(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    egui::Panel::bottom("workbench-actions")
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(p.surface)
                .inner_margin(egui::Margin::symmetric(28, 10)),
        )
        .show(ui, |ui| {
            let n = state.checked_count();
            let can = state.can_start();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = p.sz(1.0);
                    if n == 0 {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("还没有选中任何项")
                                    .size(p.sz(12.5))
                                    .color(p.text_dim),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("在下面清单里点一行即可勾选")
                                    .size(p.sz(10.5))
                                    .color(p.text_faint),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "已选 {n} 项 · {}",
                                    molt_scan::human_bytes(state.checked_bytes())
                                ))
                                .size(p.sz(14.0))
                                .strong(),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                        // 筛选只影响「看到哪些行」，不影响勾选。当前筛选下看不见
                        // 但已被勾中的那些必须说清，否则用户会以为自己只选了 3 个。
                        let hidden = state.checked_outside_filter();
                        if hidden > 0 {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "其中 {hidden} 项在当前筛选之外（筛选不会取消勾选）"
                                    ))
                                    .size(p.sz(10.5))
                                    .color(p.costly),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new("放入临时空间 · 保留 7 天")
                                        .size(p.sz(10.5))
                                        .color(p.text_faint),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, "清理这些（可还原）", n > 0 && can) {
                        acts.push(Action::PrepareClean(CleanMode::Quarantine));
                    }
                    // 不可恢复的路留在主按钮左边，降成一行红字：它需要用户
                    // 主动去找，而不是和主按钮并排抢眼。10 月 23 日那次误删
                    // 两个真实目录，就是因为它当时和主按钮长得一样大。
                    if danger_text_button(ui, p, "永久删除…", n > 0 && can) {
                        acts.push(Action::PrepareClean(CleanMode::Delete));
                    }
                    if secondary_button(ui, p, "全不选", n > 0 && state.can_select()) {
                        acts.push(Action::ClearSelection);
                    }
                    // 「全选」三个动作在 `action.rs` 里早就实现了，界面却一直没有
                    // 入口——用户只能一行一行点勾。这里补上最保守的一档：只选当前
                    // 筛选下看得见的行（不含「建议保留」），受保护条目本身也选不上。
                    if secondary_button(
                        ui,
                        p,
                        "全选本页",
                        state.can_select() && !state.visible_items().is_empty(),
                    ) {
                        acts.push(Action::SelectAllVisible);
                    }
                });
            });

            if state.activity().is_busy() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(state.activity().blocking_hint())
                            .size(p.sz(10.5))
                            .color(p.costly),
                    )
                    .wrap_mode(egui::TextWrapMode::Wrap),
                );
            }
        });
}

fn scanning_card(ui: &mut egui::Ui, state: &AppState, p: &Palette) {
    card(ui, p, |ui| {
        ui.label(egui::RichText::new("正在检查…").size(p.sz(15.0)).strong());
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new("第一次扫描要遍历各工具的缓存目录，通常需要几十秒。")
                .size(p.sz(11.5))
                .color(p.text_dim),
        );
        ui.add_space(12.0);

        match state.job.as_ref().map(|j| j.peek().clone()) {
            Some(crate::worker::Progress::Judging { done, total }) if total > 0 => {
                progress_bar(
                    ui,
                    p,
                    done as f32 / total as f32,
                    p.sz(320.0).min(ui.available_width()),
                    &format!("已检查 {done} / {total} 项"),
                    p.sz(6.0),
                    p.reclaimable,
                );
            }
            Some(crate::worker::Progress::Walking { found }) if found > 0 => {
                busy_bar(ui, p, p.sz(320.0).min(ui.available_width()));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("已找到 {found} 项，正在测量大小"))
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                );
            }
            _ => {
                busy_bar(ui, p, p.sz(320.0).min(ui.available_width()));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("正在遍历目录…")
                        .size(p.sz(11.0))
                        .color(p.text_dim),
                );
            }
        }
    });
}

fn failed_card(
    ui: &mut egui::Ui,
    state: &AppState,
    p: &Palette,
    err: &str,
    acts: &mut Vec<Action>,
) {
    card(ui, p, |ui| {
        ui.label(
            egui::RichText::new("扫描没有完成")
                .size(p.sz(15.0))
                .strong()
                .color(p.danger),
        );
        ui.add_space(3.0);
        ui.add(
            egui::Label::new(egui::RichText::new(err).size(p.sz(12.0)).color(p.text_dim))
                .wrap_mode(egui::TextWrapMode::Wrap),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("扫描是只读的，你的文件没有被改动过。")
                .size(p.sz(11.5))
                .color(p.reclaimable),
        );
        ui.add_space(10.0);
        if primary_button(ui, p, "重试扫描", state.can_start()) {
            acts.push(Action::FollowNextStep(crate::state::NextStep::Retry));
        }
    });
}

/// 确认框。清理与隔离区彻底删除共用同一套视觉语言，文案只讲这一次的后果。
pub fn confirm_modal(ui: &mut egui::Ui, state: &AppState, p: &Palette, acts: &mut Vec<Action>) {
    let Some(pending) = state.pending_clean.clone() else {
        return;
    };
    let is_delete = pending.mode == CleanMode::Delete;
    let title = if is_delete {
        "永久删除这些内容？"
    } else {
        "把这些内容放入临时空间？"
    };

    // 骨架来自 `components::confirm_dialog`，与临时空间页的「彻底删除」共用：
    // 标题层级、量化数字、红底警示、按钮顺序都只有一份实现。
    // 这一页只负责填词，以及中间那份**带 AI 解释**的对象清单。
    let mut dialog = crate::components::ConfirmDialog::new(
        "molt-confirm-clean",
        if is_delete {
            crate::components::ConfirmKind::Danger
        } else {
            crate::components::ConfirmKind::Normal
        },
        title,
    )
    .summary(format!(
        "{} 项 · 共 {}",
        pending.count(),
        molt_scan::human_bytes(pending.bytes)
    ))
    .confirm_label(if is_delete {
        "永久删除"
    } else {
        "确认放入临时空间"
    })
    .width(480.0);

    dialog = if is_delete {
        dialog.warning(
            "不可恢复，也不会经过临时空间。",
            "这一步会直接从原位置删除所选文件，请先核对下方明细。",
        )
    } else {
        let expiry =
            molt_act::add_days(&molt_act::now_iso8601(), crate::paths::DEFAULT_RETAIN_DAYS);
        dialog.note(format!(
            "可还原至 {}。移到临时空间不会立刻腾出磁盘空间。当前直供版移除应用不会自动清空这里；手动删除应用数据目录会让未还原内容和记录一并丢失。",
            &expiry[..10]
        ))
    };

    let outcome = dialog.show(ui, p, |ui| {
        object_list(ui, p, &pending.items, state, is_delete);
    });

    acts.extend(confirm_actions(outcome));
}

/// 确认框给出结果后要发的动作。
///
/// 确认只发 `RunPendingClean` 这一条：它经 `clean_from_pending` 取走批次的同时
/// 就把 `pending_clean` 清空了。曾经这里先推 `CancelPendingClean` 再推
/// `RunPendingClean`，两条按序执行时前一条已经把批次清空，后一条拿不到东西
/// 就静默返回——弹窗关了、盘上没动、也没有任何提示，用户看到的正是「点了没用」。
///
/// 抽成函数是为了让测试能直接拿这份动作去 `apply`，验证它真的换出了
/// `Effect::Clean`，而不是只断言「这里写了两个 push」。
fn confirm_actions(outcome: crate::components::ConfirmOutcome) -> Vec<Action> {
    match outcome {
        crate::components::ConfirmOutcome::Confirmed => vec![Action::RunPendingClean],
        crate::components::ConfirmOutcome::Cancelled => vec![Action::CancelPendingClean],
        crate::components::ConfirmOutcome::Open => Vec::new(),
    }
}

/// 确认框里的对象清单。只给数字是不够的——用户有权在不可撤销之前看见选了什么，
/// 以及 AI 对每一条的解释。
///
/// 框体与行布局交给 `components::confirm_dialog::item_list`，这里只负责
/// 把每条转成「显示用的路径 + AI 说明」两行。
fn object_list(
    ui: &mut egui::Ui,
    p: &Palette,
    items: &[PendingItem],
    state: &AppState,
    danger: bool,
) {
    const SHOWN: usize = 5;
    let rows: Vec<(String, String)> = items
        .iter()
        .take(SHOWN)
        .map(|item| {
            (
                crate::paths::display_in(&state.home, &item.path),
                item.note.clone().unwrap_or_default(),
            )
        })
        .collect();
    let footer = (items.len() > SHOWN).then(|| {
        format!(
            "还有 {} 项，共 {}",
            items.len() - SHOWN,
            molt_scan::human_bytes(items.iter().skip(SHOWN).map(|i| i.bytes).sum())
        )
    });
    crate::components::confirm_dialog::item_list(ui, p, danger, &rows, footer);
}

fn idle_days(c: &molt_core::Candidate) -> Option<u64> {
    if let Some(d) = c.evidence.iter().find_map(|e| match e {
        molt_core::Evidence::LastUsed { days_ago } => Some(*days_ago),
        _ => None,
    }) {
        return Some(d);
    }
    let then = c.last_used?;
    std::time::SystemTime::now()
        .duration_since(then)
        .ok()
        .map(|d| d.as_secs() / 86_400)
}

/// 起一次扫描。互斥由 `can_start` 在按钮层挡，这里再确认一次。
pub fn start_scan(state: &mut AppState) {
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    let Some(rules) = state.rules.clone() else {
        state.error = Some("规则库没有加载成功".into());
        return;
    };
    state.refresh_disk();
    state.error = None;
    state.status = Some(StatusMessage::info("正在扫描…"));
    state.job = Some(crate::worker::ScanJob::start(
        rules,
        state.home.clone(),
        true,
    ));
}

/// 起一次 AI 解释。所有前置条件在这里集中判断，
/// 这样按钮的可用状态与点击后的行为不会分叉。
pub fn start_analysis(state: &mut AppState) {
    if !state.can_start() {
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }
    let resolved = match state.ai.resolve() {
        Ok(r) => r,
        Err(e) => {
            state.ai.failure = Some(crate::ai_worker::AiFailure {
                message: e,
                fixable_by_config: true,
                raw: None,
            });
            return;
        }
    };
    let Some(result) = state.result.clone() else {
        state.status = Some(StatusMessage::warn("还没有扫描结果，先扫描一次"));
        return;
    };

    let disk = match &state.disk {
        Some(Ok(d)) => Some(*d),
        _ => None,
    };
    state.ai.failure = None;
    state.ai.job = Some(crate::ai_worker::AiJob::start(
        resolved,
        state.ai.config.clone(),
        result.items.clone(),
        state.home.clone(),
        disk,
    ));
    state.status = Some(StatusMessage::info("正在让 AI 解释这次扫描…"));
}

/// 执行清理。与 CLI 走同一个 `molt_act::Executor`。
///
/// 批次由调用方传入（来自 `Effect::Clean`），不再从 `state.pending_clean` 里取：
/// 两者在确认框按下确定的那一刻已经是同一个东西，再读一次状态等于给
/// 「实际执行的」与「用户确认的」留下分叉的可能。
pub fn start_clean(state: &mut AppState, pending: &crate::state::PendingClean) {
    let Some(result) = state.result.clone() else {
        state.pending_clean = None;
        return;
    };
    if !state.can_start() {
        state.pending_clean = None;
        state.status = Some(StatusMessage::warn(
            state.activity().blocking_hint().to_string(),
        ));
        return;
    }

    let selected: Vec<molt_core::Candidate> = result
        .items
        .iter()
        .filter(|c| state.is_checked(&c.path.display().to_string()))
        .filter(|c| pending.items.iter().any(|i| i.path == c.path))
        .cloned()
        .collect();

    if selected.is_empty() {
        state.pending_clean = None;
        state.status = Some(StatusMessage::warn("没有选中任何项，未执行清理"));
        return;
    }

    let store = match molt_act::AuditStore::open(&crate::paths::audit_db()) {
        Ok(s) => s,
        Err(e) => {
            state.pending_clean = None;
            state.status = Some(StatusMessage::error(format!(
                "打开审计库失败，未执行清理：{e}"
            )));
            return;
        }
    };
    let batch = molt_act::now_iso8601().replace(':', "-");
    let q = molt_act::Quarantine::new(crate::paths::quarantine_root(), batch);

    state.pending_clean = None;
    // 后台逐项回报进度，摘要条直接呈现，不另弹窗口遮住清单。
    // 每批进度从零开始，体积会在完成报告中改为实际处理量。
    state.clean_total_items = selected.len();
    state.clean_total_bytes = selected.iter().map(|c| c.alloc_bytes.max(c.bytes)).sum();
    state.clean_anim = 0.0;
    state.clean_done_secs = 0.0;
    state.clean_job = Some(crate::worker::CleanJob::start(
        selected,
        store,
        q,
        pending.mode,
        crate::paths::DEFAULT_RETAIN_DAYS,
    ));
}

/// 保留旧名给测试与调用方：语义等价于「按当前 `pending_clean` 执行」。
pub fn do_clean(state: &mut AppState) {
    if let Some(pending) = state.pending_clean.clone() {
        start_clean(state, &pending);
    }
}

/// 取回后台清理的结果。
pub fn pump_clean(state: &mut AppState) {
    let Some(job) = state.clean_job.take() else {
        return;
    };
    let mut job = job;
    match job.poll().clone() {
        crate::worker::CleanProgress::Done(report) => {
            state.session_freed += report.deleted_bytes;
            let op = report_to_op(&report);
            state.status = Some(StatusMessage {
                kind: op.kind,
                text: op.summary.clone(),
            });
            state.last_op = Some(Arc::new(op));
            // 完成结果保留 1.8 秒；后台重扫不该盖住清理成功提示。
            state.clean_anim = 1.0;
            state.clean_done_secs = 1.8;
            // 「放入临时空间」不释放空间（只是 rename），圆心不能写成「已腾出」。
            // 只有直接清理才是真的腾出来了；临时空间那批只报「移走了多少」。
            state.clean_was_delete = report.mode == CleanMode::Delete;
            state.clean_total_bytes = if state.clean_was_delete {
                report.deleted_bytes
            } else {
                report.moved_bytes
            };
            // 清完立刻重扫：清单里现在应该是清理后的真实状态
            crate::pages::workbench::start_scan(state);
            crate::pages::quarantine::reload(state);
            crate::pages::history::reload(state);
            state.refresh_disk();
        }
        crate::worker::CleanProgress::Working { done, total } => {
            state.clean_anim = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            state.clean_job = Some(job);
        }
    }
}

fn report_to_op(report: &crate::worker::CleanReport) -> crate::state::OpReport {
    let failed = report.failures.len();
    let (done, done_bytes) = match report.mode {
        CleanMode::Delete => (report.deleted, report.deleted_bytes),
        CleanMode::Quarantine => (report.moved, report.moved_bytes),
    };

    let summary = match report.mode {
        CleanMode::Delete => format!(
            "已永久删除 {} 项，释放 {}",
            report.deleted,
            molt_scan::human_bytes(report.deleted_bytes)
        ),
        CleanMode::Quarantine => format!(
            "已放入临时空间 {} 项，共 {}（{} 天内可还原）",
            report.moved,
            molt_scan::human_bytes(report.moved_bytes),
            crate::paths::DEFAULT_RETAIN_DAYS
        ),
    };

    if done == 0 {
        let first = report
            .failures
            .first()
            .map(|(path, reason)| format!("{}：{reason}", crate::paths::display_path_str(path)))
            .unwrap_or_else(|| "没有可处理的内容".to_string());
        return crate::state::OpReport {
            summary: format!("清理没有生效：{first}"),
            done: 0,
            done_bytes: 0,
            failures: report.failures.clone(),
            kind: StatusKind::Error,
            next: Some(crate::state::NextStep::Retry),
        };
    }

    if failed > 0 {
        crate::state::OpReport {
            summary: format!("{summary}；{failed} 项未处理，原因见下方"),
            done,
            done_bytes,
            failures: report.failures.clone(),
            kind: StatusKind::Warn,
            next: Some(crate::state::NextStep::Retry),
        }
    } else {
        // 做成之后把用户送到「刚移走的东西去哪了」——
        // 移入隔离区的去隔离区，直接删的只能去历史看记录。
        let next = match report.mode {
            CleanMode::Quarantine => crate::state::NextStep::Quarantine,
            CleanMode::Delete => crate::state::NextStep::History,
        };
        crate::state::OpReport {
            summary,
            done,
            done_bytes,
            failures: Vec::new(),
            kind: StatusKind::Info,
            next: Some(next),
        }
    }
}

/// 每帧推进 AI 任务：把解释落进状态，供清单逐行读取。
pub fn pump_ai(state: &mut AppState) {
    let Some(job) = state.ai.job.as_mut() else {
        return;
    };
    match job.poll().clone() {
        crate::ai_worker::AiProgress::Done(outcome) => {
            let home = state.home.clone();
            state.ai.notes = outcome
                .notes
                .iter()
                .map(|(p, n)| (p.display().to_string(), n.clone()))
                .collect();
            // 「AI 认为不能动」的路径单独存一份，用于排序时把它们挪进「建议保留」。
            // 只认模型显式表态过的条目；没表态的按本地判定走。
            state.ai.safe = outcome
                .report
                .explanations
                .iter()
                .filter_map(|e| {
                    let abs = if let Some(rest) = e.path.strip_prefix("~/") {
                        std::path::Path::new(&home).join(rest)
                    } else {
                        std::path::PathBuf::from(&e.path)
                    };
                    e.safe_to_clean.map(|v| (abs.display().to_string(), v))
                })
                .collect();
            state.ai.last_model = Some(outcome.model.clone());
            state.ai.last_sent = outcome.sent_items;
            state.ai.last_at = Some(molt_act::now_iso8601());
            state.ai.failure = None;
            state.ai.job = None;
            state.status = Some(StatusMessage::info(format!(
                "AI 已解释 {} 条，说明显示在对应条目上",
                state.ai.notes.len()
            )));
            // 解释可能把某些行挪进「建议保留」，重新按默认勾选那批对齐
            if let Some(r) = state.result.clone() {
                let keep: std::collections::HashSet<String> = r
                    .items
                    .iter()
                    .filter(|c| {
                        matches!(state.priority_of(c), Priority::Safe | Priority::Suggested)
                    })
                    .map(|c| c.path.display().to_string())
                    .collect();
                state.checked = keep;
            }
        }
        crate::ai_worker::AiProgress::Failed(f) => {
            state.ai.failure = Some(f.clone());
            state.ai.job = None;
            state.status = Some(StatusMessage::error(format!(
                "AI 解释失败：{}（本地清理不受影响）",
                f.message
            )));
        }
        crate::ai_worker::AiProgress::Stage(_) => {}
    }
}

/// 取后台扫描的进展。主循环与端到端测试都调它——
/// 测试必须走用户真会走的那条路径（包括「清理后自动重扫」），
/// 否则验的是另一套时序。
pub fn pump_scan(state: &mut AppState) {
    let Some(job) = state.job.take() else {
        return;
    };
    let mut job = job;
    match job.poll().clone() {
        crate::worker::Progress::Done(result) => {
            // 落一份缓存：下次打开窗口先用它铺页面，不必再等十几秒。
            result.save_cache(&crate::paths::scan_cache());
            state.apply_result(result);
            state.refresh_disk();
            // 记下这份结果的时刻：下次扫描失败时要用它说明「旧数据是哪来的」。
            state.last_scan_at = Some(crate::paths::now_hhmm());
            state.scanned_now = true;
            if state.error.is_none() {
                state.status = None;
            }
        }
        crate::worker::Progress::Failed(msg) => {
            if msg == "扫描已取消" {
                // 用户自己按的取消，不是故障：清掉旧错误、给一条中性提示，
                // 并且**保留**上一次的结果——把清单清空会让人以为盘变干净了。
                state.error = None;
                state.status = Some(StatusMessage::info("已取消这次扫描"));
            } else {
                state.error = Some(msg.clone());
                state.status = Some(StatusMessage::error(msg));
            }
            state.refresh_disk();
        }
        _ => {
            state.job = Some(job);
        }
    }
}

/// 取后台重复文件扫描的进展，回填结果。
pub fn pump_duplicates(state: &mut AppState) {
    let Some(job) = state.duplicate_job.take() else {
        return;
    };
    let mut job = job;
    match job.poll().clone() {
        crate::worker::DuplicateProgress::Done(scan) => {
            state.duplicate_scan = Some(scan);
            state.status = Some(StatusMessage::info("重复文件检测完成"));
        }
        crate::worker::DuplicateProgress::Failed(msg) => {
            state.status = Some(StatusMessage::error(msg));
        }
        _ => {
            state.duplicate_job = Some(job);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use std::sync::Arc;

    fn painted_text(ctx: &egui::Context, state: &mut AppState, p: &Palette) -> Vec<String> {
        let mut dump = Vec::new();
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760.0, 2600.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut acts = Vec::new();
                    show(ui, state, p, &mut acts);
                });
            });
            out.textures_delta.clear();
            dump.clear();
            for cs in &out.shapes {
                collect_text(&cs.shape, &mut dump);
            }
        }
        dump
    }

    /// 首页是「摘要在上、清单在下」的一条竖线。
    ///
    /// 旧版这条守的是「圆环在图例上面」——环和图例都删掉之后，要守的是
    /// 新的版面顺序：先看到盘的数字，再看到条目。
    #[test]
    fn summary_sits_above_the_list() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.disk = Some(Ok(molt_scan::disk::DiskUsage {
            total: 500 << 30,
            used: 320 << 30,
            available: 180 << 30,
        }));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 2200.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut acts = Vec::new();
                show(ui, &state, &Palette::light(), &mut acts);
            });
        });
        out.textures_delta.clear();

        let mut texts: Vec<(String, f32)> = Vec::new();
        collect_text_pos(&out.shapes, &mut texts);
        let y = |needle: &str| -> Option<f32> {
            texts
                .iter()
                .find(|(t, _)| t.contains(needle))
                .map(|(_, y)| *y)
        };
        let avail_y = y("可用").expect("摘要条要有「可用」");
        let item_y = y("cargo build 的输出目录").expect("清单要默认展开");
        assert!(
            avail_y < item_y,
            "摘要要在清单上面：摘要 {avail_y}，条目 {item_y}"
        );
    }

    #[test]
    fn home_pending_release_uses_only_loaded_restorable_records() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let palette = Palette::light();
        let loading = painted_text(&ctx, &mut state, &palette).join(" ");
        assert!(loading.contains("正在读取临时空间待释放量"));
        assert!(!loading.contains("待释放 0 项"));

        state.quarantine_loading = false;
        let record = |id, bytes, restorable| crate::state::QuarantineRow {
            id,
            original: "/Users/me/cache".into(),
            bytes,
            at: "2026-11-06T08:00:00Z".into(),
            purge_after: "2026-11-13T08:00:00Z".into(),
            batch: None,
            restorable,
        };
        state.quarantine = vec![record(1, 620 << 20, true), record(2, 3 << 30, false)];
        assert_eq!(state.pending_release(), Some((1, 620 << 20)));
        let loaded = painted_text(&ctx, &mut state, &palette).join(" ");
        let label = format!("待释放 {} · 1 项", molt_scan::human_bytes(620 << 20));
        assert!(
            loaded.contains(&label),
            "首页应显示尚可还原的体积：{loaded}"
        );

        state.quarantine_error = Some("审计库读取失败".into());
        assert_eq!(state.pending_release(), None);
        let failed = painted_text(&ctx, &mut state, &palette).join(" ");
        assert!(failed.contains("审计库读取失败"));
        assert!(!failed.contains("待释放 0 项"));

        state.quarantine_error = None;
        state.quarantine.clear();
        let empty = painted_text(&ctx, &mut state, &palette).join(" ");
        assert!(empty.contains("待释放 0 项"));
    }

    fn collect_text_pos(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<(String, f32)>) {
        for cs in shapes {
            walk_text_pos(&cs.shape, out);
        }
    }

    fn walk_text_pos(shape: &egui::epaint::Shape, out: &mut Vec<(String, f32)>) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                out.push((t.galley.text().to_string(), t.pos.y));
            }
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    walk_text_pos(s, out);
                }
            }
            _ => {}
        }
    }

    /// 清理进行中：摘要条同时给真实项数与由该项数计算的百分比。
    #[test]
    fn cleaning_shows_progress_in_the_summary() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.clean_total_items = 24;
        state.clean_total_bytes = 3_600_000_000;
        state.clean_job = Some(crate::worker::CleanJob::preview_progress(11, 24));
        pump_clean(&mut state);
        assert!((state.clean_anim - 11.0 / 24.0).abs() < f32::EPSILON);

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("正在清理 11/24 项 · 46%")),
            "清理中必须显示已处理的真实项数及相应百分比：{dump:#?}"
        );
        // 旧版那只圆环已经不在，摘要条不该继承它的居中大字。
        assert!(
            !dump.iter().any(|l| l.contains("还剩")),
            "摘要条不该再出现旧版的「还剩」：{dump:#?}"
        );
    }

    /// 清完那一刻：摘要条显示「清理完成」与本次实际处理量。
    #[test]
    fn finished_clean_announces_the_result() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.clean_anim = 1.0;
        state.clean_done_secs = 1.5;
        state.clean_was_delete = true;
        state.clean_total_bytes = 3_600_000_000;

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        let completion = dump
            .iter()
            .find(|l| l.contains("清理完成"))
            .expect("完成反馈");
        assert!(
            completion.contains(&format!(
                "释放 {}",
                molt_scan::human_bytes(state.clean_total_bytes)
            )) && !completion.contains('+'),
            "永久删除要明确说明实际释放量，不能再用 + 体积：{completion}"
        );
    }

    /// 放进临时空间只是换了个位置，不能把移入体积说成释放的空间。
    #[test]
    fn quarantine_run_does_not_claim_freed_space() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.clean_anim = 1.0;
        state.clean_done_secs = 1.5;
        state.clean_was_delete = false;
        state.clean_total_bytes = 3_600_000_000;

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        let completion = dump
            .iter()
            .find(|l| l.contains("清理完成"))
            .expect("完成反馈");
        assert!(
            completion.contains(&format!(
                "已移入临时空间 {}，可还原",
                molt_scan::human_bytes(state.clean_total_bytes)
            )) && !completion.contains("释放")
                && !completion.contains('+'),
            "移入临时空间要说明实际体积和可还原，不能误写成释放量：{completion}"
        );
    }

    fn collect_text(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    collect_text(s, out);
                }
            }
            _ => {}
        }
    }

    fn state_with_notes() -> AppState {
        let mut state = AppState::new("/Users/me".to_string());
        let mut stale = molt_core::Candidate::new(
            "/Users/me/project/mac/write-T/target",
            molt_core::ItemKind::Dir,
            molt_core::Verdict::Costly {
                rebuild: molt_core::RebuildCost::new(
                    molt_core::CostKind::Recompile,
                    "这些都会重新编译出来",
                ),
            },
        )
        .with_size(26 << 30, 26 << 30, 100);
        stale.suggest_clean = true;
        stale.rule_id = Some("rust-target".into());
        stale.evidence.push(molt_core::Evidence::RuleHit {
            rule_id: "rust-target".into(),
            pattern: "~/project/**/target".into(),
        });
        stale
            .evidence
            .push(molt_core::Evidence::LastUsed { days_ago: 62 });
        let items = vec![
            stale,
            molt_core::Candidate::new(
                "/Users/me/Library/Caches/com.example.tool",
                molt_core::ItemKind::Dir,
                molt_core::Verdict::Reclaimable,
            )
            .with_size(1536 << 20, 1536 << 20, 10),
        ];
        state.apply_result(Arc::new(crate::worker::ScanResult {
            items,
            rule_ids: vec![],
            denied_roots: vec![],
            elapsed_secs: 12.0,
            from_cache: false,
        }));
        state.ai.notes.insert(
            "/Users/me/project/mac/write-T/target".into(),
            "cargo build 的输出目录，删掉后下次构建会重新生成".into(),
        );
        state
    }

    /// 确认框按下「确认直接清理」必须真的开始清理。
    ///
    /// 回归点：确认分支曾经推 `[CancelPendingClean, RunPendingClean]` 两条动作，
    /// 前一条把批次清空，后一条拿不到东西就静默返回——弹窗关掉、盘上没动、
    /// 界面也没有任何提示。这里不检查「push 了哪几个 Action」，
    /// 而是把确认产物喂回 `apply`，验证它真的换出了 `Effect::Clean`。
    #[test]
    fn confirming_the_clean_dialog_yields_a_real_clean_effect() {
        let mut s = state_with_notes();
        // 走真路径装批次，模拟用户按的是「直接清理…」
        crate::action::apply(&mut s, Action::PrepareClean(molt_act::CleanMode::Delete));
        let wanted = s.pending_clean.as_ref().expect("勾选项应装成批次").count();
        assert!(wanted > 0);

        let mut effects = Vec::new();
        for act in confirm_actions(crate::components::ConfirmOutcome::Confirmed) {
            effects.extend(crate::action::apply(&mut s, act));
        }

        assert_eq!(effects.len(), 1, "确认应当且只应当产生一个执行意图");
        match effects.pop().unwrap() {
            crate::action::Effect::Clean(pending) => {
                assert_eq!(pending.count(), wanted, "交给后台的应是用户确认的那批");
                assert_eq!(pending.mode, molt_act::CleanMode::Delete);
            }
            other => panic!("确认后应产生 Effect::Clean，实际是 {other:?}"),
        }
        assert!(s.pending_clean.is_none(), "批次交给后台后不该留在状态里");
    }

    /// 取消则什么都不启动，勾选也保留。
    #[test]
    fn cancelling_the_dialog_starts_nothing() {
        let mut s = state_with_notes();
        crate::action::apply(&mut s, Action::PrepareClean(molt_act::CleanMode::Delete));

        let mut effects = Vec::new();
        for act in confirm_actions(crate::components::ConfirmOutcome::Cancelled) {
            effects.extend(crate::action::apply(&mut s, act));
        }
        assert!(effects.is_empty(), "取消不该产生任何执行意图");
        assert!(s.pending_clean.is_none());
        assert!(s.checked_count() > 0, "取消确认不等于放弃勾选");
    }

    #[test]
    fn workbench_puts_the_priority_sections_on_screen() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let dump = painted_text(&ctx, &mut state, &Palette::light());

        // 摘要条回答「盘还剩多少、能清多少」——一行说清，不再占一整屏。
        assert!(
            dump.iter().any(|l| l.contains("可用")),
            "摘要条要有可用空间：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可清理")),
            "摘要条要有可清理那一档：{dump:#?}"
        );

        // 清单默认展开：分节与条目第一屏就在，不需要再点「看看都清什么」。
        assert!(
            dump.iter().any(|l| l.contains("建议清掉")),
            "必须有一档写清「建议清掉」：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("可以直接清")),
            "必须有可回收那一段：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("cargo build 的输出目录")),
            "AI 解释要直接显示在条目上：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l == "AI"),
            "AI 说的话要有标记与本地判定区分：{dump:#?}"
        );

        // 主操作在底栏，且说清可还原。
        assert!(
            dump.iter().any(|l| l.contains("清理这些")),
            "底栏要有主操作：{dump:#?}"
        );
    }

    #[test]
    fn rows_show_both_the_local_verdict_and_why() {
        // 判定理由从「每行重复一遍」改成「分节标题说一次」——十一行写同一句话
        // 是噪声，不是信息。但理由本身必须还在页面上，否则用户又得靠猜。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("删了会自动重建")),
            "可回收那一段的标题要给出理由：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("重新编译")),
            "有代价那一段的标题要给出代价：{dump:#?}"
        );
    }

    #[test]
    fn summary_count_matches_what_is_actually_checked() {
        // 回归：顶部曾把「建议清理」算进「可回收」再加一次，
        // 于是写着「可以先清 3 项」而列表里只有 2 项勾着。
        let state = state_with_notes();
        let (n, _) = state.reclaimable_summary();
        let (sug, _) = state.suggested_summary();
        assert_eq!(n + sug, state.checked_count(), "顶部数字必须与勾选数一致");
    }

    #[test]
    fn stale_result_is_labelled_as_stale() {
        // 重扫失败但旧结果还在：数字保留，但必须先说清它是旧的。
        // 不说的话，用户会把 10:42 那份当成刚刚扫出来的。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.last_scan_at = Some("10:42".into());
        state.error = Some("遍历 ~/Library/Caches 时被系统拒绝".into());

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("这次扫描失败")),
            "失败要写在最上面：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("10:42")),
            "要说清下面这份是什么时候的结果：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("重试")),
            "失败必须给一个可点的下一步：{dump:#?}"
        );
    }

    #[test]
    fn freed_bytes_are_shown_once_something_was_cleaned() {
        // session_freed 一直在累加，但从没显示过；清完用户没有任何持久证据。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.session_freed = 1_240_000_000;
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("本次运行已腾出")),
            "清完要说清腾出了多少：{dump:#?}"
        );
    }

    #[test]
    fn filtered_view_always_offers_a_way_back() {
        // hero 里的两个链接会切筛选；切过去之后必须有一条回到「全部」的路。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.filter = Filter::Costly;
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("显示全部")),
            "筛到某一档后要有出口：{dump:#?}"
        );
    }

    #[test]
    fn unconfigured_ai_offers_a_way_to_configure_it() {
        // 未配置时以前只显示 resolve() 报错的最后 30 个字——
        // 用户看到的是「…API_KEY，或在配置里填写 api_key」这种半句，
        // 既不知道怎么读，也没有一条去配置页的路。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.ai.config.active_provider.clear();
        state.ai.config.active_model.clear();

        // 首页不再挂「AI 未配置」的尾巴：那不是清理的必要条件，
        // 挂在首页只会让人以为主流程被挡住了。首页必须照常给出清理入口。
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("放入临时空间")),
            "没配 AI 也要能清理：{dump:#?}"
        );
        assert!(
            !dump.iter().any(|l| l.contains("未配置")),
            "首页不该提 AI 配置：{dump:#?}"
        );

        // 但「让 AI 解释」这个入口仍然在清单里，只是点不动——
        // 用户想知道为什么点不了时，设置页有密钥输入框。
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        // 这份 state 里已经有解释，按钮因此写「重新解释」——两种文案都算入口在。
        assert!(
            dump.iter()
                .any(|l| l.contains("解释") && l.contains("AI") || l == "重新解释"),
            "清单里要保留 AI 解释入口：{dump:#?}"
        );
    }

    #[test]
    fn explained_rows_never_claim_ai_is_unconfigured() {
        // 回归：已经解释过 24 条、配置此刻读不出来时，工具条上同时显示
        // 「AI 已解释 24 条」和「AI 未配置」。两句话互相打架，
        // 用户会以为那些解释是假的。
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        // 解释还在，但配置读不出来
        state.ai.config.active_provider.clear();
        state.ai.config.active_model.clear();

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("解释")),
            "已解释的条数要显示：{dump:#?}"
        );
        assert!(
            !dump.iter().any(|l| l.contains("AI 未配置")),
            "已经解释过就不能说「未配置」，两者矛盾：{dump:#?}"
        );
        assert!(
            dump.iter().any(|l| l.contains("去设置")),
            "点不了的时候要给一条去配置的路：{dump:#?}"
        );
    }

    /// 详情抽屉展示一行放不下的信息：完整路径、判定理由、勾选状态。
    ///
    /// 只读：抽屉打开不该改动勾选，也不该出现执行按钮。
    #[test]
    fn detail_drawer_shows_the_full_record() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let path = "/Users/me/project/mac/write-T/target";
        state.detail_path = Some(path.into());
        let checked_before = state.is_checked(path);

        let dump = painted_text(&ctx, &mut state, &Palette::light());
        for needle in [
            "路径",
            "大小",
            "判定",
            "命中规则",
            "rust-target",
            "~/project/**/target",
            "最近使用",
            "62 天前",
            "本次勾选",
            "在访达中显示",
        ] {
            assert!(
                dump.iter().any(|l| l.contains(needle)),
                "抽屉要写「{needle}」：{dump:#?}"
            );
        }
        assert!(
            dump.iter().any(|l| l == path),
            "抽屉要写完整路径：{dump:#?}"
        );
        assert_eq!(state.is_checked(path), checked_before, "看详情不该改变勾选");
    }

    #[test]
    fn five_thousand_rows_only_paint_the_visible_window() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = AppState::new("/Users/me".into());
        let items = (0..5000)
            .map(|i| {
                molt_core::Candidate::new(
                    format!("/Users/me/Library/Caches/cache-{i:04}"),
                    molt_core::ItemKind::Dir,
                    molt_core::Verdict::Reclaimable,
                )
                .with_size(4096, 4096, 1)
            })
            .collect();
        state.apply_result(Arc::new(crate::worker::ScanResult {
            items,
            rule_ids: vec![],
            denied_roots: vec![],
            elapsed_secs: 1.0,
            from_cache: false,
        }));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 680.0),
            )),
            ..Default::default()
        };
        // 第一次建立滚动区域，第二帧断言实际提交给画布的文字数量。
        for frame in 0..2 {
            let started = std::time::Instant::now();
            let mut out = ctx.run_ui(raw.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut acts = Vec::new();
                    show(ui, &state, &Palette::light(), &mut acts);
                });
            });
            let frame_time = started.elapsed();
            println!("5000 项第 {frame} 帧 UI 耗时：{frame_time:?}");
            #[cfg(not(debug_assertions))]
            {
                let budget = if frame == 0 {
                    std::time::Duration::from_millis(100)
                } else {
                    std::time::Duration::from_micros(16_667)
                };
                assert!(
                    frame_time < budget,
                    "5000 项 UI 帧超时：{frame_time:?} >= {budget:?}"
                );
            }
            if frame == 1 {
                let mut texts = Vec::new();
                for shape in &out.shapes {
                    collect_text(&shape.shape, &mut texts);
                }
                let rendered = texts.iter().filter(|t| t.contains("cache-")).count();
                assert!(
                    rendered > 0 && rendered < 60,
                    "必须显示可见行且不能画完 5000 行：{rendered}"
                );
            }
            out.textures_delta.clear();
        }
    }

    #[test]
    fn project_and_tool_groups_are_stable_across_related_paths() {
        let candidate = |path| {
            molt_core::Candidate::new(
                path,
                molt_core::ItemKind::Dir,
                molt_core::Verdict::Reclaimable,
            )
        };
        let project = source_group("/Users/me", &candidate("~/project/gallery/target"));
        assert_eq!(
            project,
            source_group("/Users/me", &candidate("~/project/gallery/node_modules"))
        );
        assert_eq!(project.1, "项目 · gallery");
        assert_eq!(
            source_group(
                "/Users/me",
                &candidate("/Users/me/project/mac/write-T/target")
            )
            .1,
            "项目 · write-T"
        );
        assert_eq!(
            source_group(
                "/Users/me",
                &candidate("~/Library/Caches/Cursor/CachedData/part-1")
            )
            .1,
            "应用缓存 · Cursor"
        );
        assert_eq!(
            source_group("/Users/me", &candidate("~/Library/Caches/cache-0000")).1,
            "其他应用缓存"
        );
    }

    #[test]
    fn folding_a_subgroup_hides_its_rows_without_changing_selection() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        let path = "/Users/me/project/mac/write-T/target";
        let before = state.is_checked(path);
        state
            .collapsed_groups
            .insert((Priority::Suggested, "project:project/mac/write-T".into()));
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(dump.iter().any(|t| t.contains("项目 · write-T")));
        assert!(!dump.iter().any(|t| t.contains("cargo build 的输出目录")));
        assert_eq!(state.is_checked(path), before);
    }

    #[test]
    fn virtual_row_height_matches_the_painted_row() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let state = state_with_notes();
        let result = state.result.as_ref().unwrap();
        for scale in [1.0, crate::theme::Theme::DEFAULT_SCALE] {
            let p = Palette::light().with_scale(scale);
            for width in [1200.0, 620.0] {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 1800.0),
                    )),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(raw, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        for (i, c) in result.items.iter().enumerate() {
                            let before = ui.cursor().top();
                            row(ui, &state, &p, result, i);
                            let actual = ui.cursor().top() - before;
                            let reserved = row_height(&state, &p, c);
                            assert!(
                                (actual - reserved).abs() < 1.0,
                                "{}: {scale} 倍、宽 {width}，实际行高 {actual}，占位 {reserved}",
                                c.path.display()
                            );
                        }
                    });
                });
                out.textures_delta.clear();
            }
        }
    }

    /// 重复文件区块：从未运行过时给「查找重复文件」入口；扫过且无重复时
    /// 保留「重新查找」。点击 → Action 的映射由 action.rs 单测与
    /// tests/duplicates_flow.rs 端到端覆盖，这里守住入口必须在界面上。
    #[test]
    fn duplicate_section_offers_an_entry_until_it_runs() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let p = Palette::light();

        // 从未运行过：区块标题、说明与启动按钮都要在。
        let mut state = state_with_notes();
        state.duplicate_job = None;
        state.duplicate_scan = None;
        let text = painted_text(&ctx, &mut state, &p).join(" ");
        assert!(text.contains("重复文件"), "从未运行过也要显示区块标题");
        assert!(
            text.contains("只读取文件做比对"),
            "说明必须常驻：没有它，用户只看得到一个按钮，不知道会发生什么"
        );
        assert!(text.contains("查找重复文件"), "从未运行过要给出启动入口");

        // 跑过但没找到：说明看过多少文件，并给出「重新查找」。
        let mut state = state_with_notes();
        state.duplicate_job = None;
        state.duplicate_scan = Some(Arc::new(molt_scan::DuplicateScan {
            files_seen: 1234,
            ..Default::default()
        }));
        let text = painted_text(&ctx, &mut state, &p).join(" ");
        assert!(text.contains("没有发现内容完全相同的文件"));
        assert!(text.contains("1234"), "要报出实际看了多少个文件");
        assert!(text.contains("重新查找"));
    }

    /// 扫描被时间截断时，绝不能报「没有发现重复」。
    ///
    /// 这两件事在版面上很像，含义完全相反：一个是「比过了，真的没有」，
    /// 另一个是「还没比完」。混成一个会让用户放心地去删别的东西。
    #[test]
    fn truncated_scan_never_claims_there_are_no_duplicates() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = state_with_notes();
        state.duplicate_job = None;
        state.duplicate_scan = Some(Arc::new(molt_scan::DuplicateScan {
            truncated: true,
            files_seen: 42,
            ..Default::default()
        }));
        let text = painted_text(&ctx, &mut state, &Palette::light()).join(" ");
        assert!(text.contains("没比完"), "必须说明没比完：{text}");
        assert!(
            !text.contains("没有发现内容完全相同的文件"),
            "截断时不能说「没有重复」：{text}"
        );
    }

    #[test]
    fn empty_state_is_not_drawn_as_a_clean_disk() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = AppState::new("/Users/me".to_string());
        let dump = painted_text(&ctx, &mut state, &Palette::light());
        assert!(
            dump.iter().any(|l| l.contains("还没有扫描结果")),
            "没扫过要说「还没有」，不能说「很干净」：{dump:#?}"
        );
        assert!(!dump.iter().any(|l| l.contains("很干净")));
    }

    /// 还没扫过的时候也必须能看见重复文件入口。
    ///
    /// 这条守的是「功能被藏起来了」这类回归：区块曾经挂在清单末尾，
    /// 而清单要「扫过且非空」才画，于是刚启动的软件里根本没有入口，
    /// 看起来就像这个功能被删掉了。
    #[test]
    fn duplicate_entry_is_visible_before_any_scan() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::Theme::install(&ctx);
        let mut state = AppState::new("/Users/me".to_string());
        assert!(state.result.is_none(), "前提：还没有扫描结果");
        let dump = painted_text(&ctx, &mut state, &Palette::light()).join(" ");
        assert!(
            dump.contains("查找重复文件"),
            "未扫描时入口就要在，否则会被当成功能没了：{dump}"
        );
    }
}
