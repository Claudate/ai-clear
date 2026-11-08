//! 确认框 → 真执行 的端到端验收：点下去之后，磁盘上到底发生了什么。
//!
//! 已有测试各自守了一段：`clean_page_buttons.rs` 守「主按钮产生 PrepareClean」，
//! `action.rs` 的单测守「确认只发 RunPendingClean」，`clean_paths.rs` 守
//! 「do_clean 真的移走 / 删掉」。缺的是把它们串起来的那一段——
//! **用户在确认框里按下确认之后，界面上那串动作真的会让文件消失**。
//! 用户报告里的「点了没反应」，坏就坏在这一段。
//!
//! 破坏性流程一律在假 home 里做，碰不到真实缓存。

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use molt_core::{Candidate, ItemKind, Verdict};
use molt_gui::action::Action;
use molt_gui::state::{AppState, StatusKind};

fn cand(path: &std::path::Path, bytes: u64) -> Candidate {
    Candidate::new(path, ItemKind::Dir, Verdict::Reclaimable).with_size(bytes, bytes, 2)
}

fn apply_result(state: &mut AppState, items: Vec<Candidate>) {
    state.apply_result(Arc::new(molt_gui::worker::ScanResult {
        items,
        rule_ids: vec![],
        denied_roots: vec![],
        elapsed_secs: 1.0,
        from_cache: false,
    }));
}

fn wait_clean(state: &mut AppState) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        molt_gui::pages::workbench::pump_clean(state);
        molt_gui::pages::workbench::pump_scan(state);
        if !state.is_cleaning() && !state.is_scanning() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("清理或其后重扫没有在 30 秒内完成");
}

/// 走完整条链：勾选 → 底栏主按钮 → 确认框确认 → 文件真的进了临时空间。
///
/// 每一步都用界面上那颗按钮真实发出的 `Action`，不直接调 `do_clean`——
/// 那样验的是底层，绕过了「按钮到动作」这一段，而缺陷恰恰出在那一段。
#[test]
fn confirming_the_modal_actually_moves_the_files() {
    let home = common::tmp_home("confirm");
    std::env::set_var("HOME", &home);
    std::env::set_var("MOLT_HOME", &home);

    let cache = home.join(".npm/_cacache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("blob.bin"), vec![5u8; 8192]).unwrap();
    let keep = home.join("project/target");
    std::fs::create_dir_all(&keep).unwrap();
    std::fs::write(keep.join("out.bin"), vec![6u8; 4096]).unwrap();

    let mut state = AppState::new(home.to_string_lossy().to_string());
    state.rules = Some(molt_rules::load(&state.home, None).unwrap());
    apply_result(&mut state, vec![cand(&cache, 8192), cand(&keep, 4096)]);

    // 只清缓存，把构建产物取消掉：模拟用户真的挑了一遍。
    state.toggle(&keep.display().to_string());
    assert_eq!(state.checked_count(), 1, "前提：只勾了缓存那一项");

    // 底栏主按钮：进入「放入临时空间」的确认。
    for act in molt_gui::action::apply(
        &mut state,
        Action::PrepareClean(molt_act::CleanMode::Quarantine),
    ) {
        let _ = act;
    }
    let pending = state.pending_clean.clone().expect("应生成待确认批次");
    assert_eq!(pending.count(), 1, "确认框里只该有勾上的那一项");

    // 用户在确认框里按下确认：这一步只发 RunPendingClean。
    let fx = molt_gui::action::apply(&mut state, Action::RunPendingClean);
    assert_eq!(fx.len(), 1, "确认后应恰好产生一个执行意图：{fx:?}");

    // 执行层：`app.rs` 收到 `Effect::Clean(pending)` 就直接调 `start_clean`，
    // 不再经过 `do_clean`（那一步只服务「已有 pending_clean」的旧入口）。
    // 测试照执行层这么做，才是用户按下去真正发生的事。
    match fx.into_iter().next().expect("应有一个执行意图") {
        molt_gui::action::Effect::Clean(pending) => {
            molt_gui::pages::workbench::start_clean(&mut state, &pending)
        }
        other => panic!("确认后应产生 Effect::Clean，实际：{other:?}"),
    }
    assert!(state.is_cleaning(), "应转到后台执行");
    wait_clean(&mut state);

    assert!(!cache.exists(), "确认之后缓存必须真的离开原位置");
    assert!(keep.exists(), "没勾的那一项必须原地不动");

    let op = state.last_op.as_ref().expect("应有结果报告");
    assert_eq!(
        op.kind,
        StatusKind::Info,
        "全部成功应报 Info：{}",
        op.summary
    );
    assert!(
        op.summary.contains("放入临时空间"),
        "结果要说清去了哪：{}",
        op.summary
    );
    assert!(
        !op.summary.contains("释放"),
        "隔离不释放空间，不能这么写：{}",
        op.summary
    );
    assert!(
        state.pending_clean.is_none(),
        "批次执行完必须被取走，否则确认框会再次弹出"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// 在确认框里取消：什么都不该动。
///
/// 这条和上面那条是一对。只有「确认」那条时，一个把取消也当成确认的实现
/// 同样能通过；两条一起才锁住「这一步真的需要用户点头」。
#[test]
fn cancelling_the_modal_touches_nothing() {
    let home = common::tmp_home("cancel");
    std::env::set_var("HOME", &home);
    std::env::set_var("MOLT_HOME", &home);

    let cache = home.join(".npm/_cacache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("blob.bin"), vec![5u8; 4096]).unwrap();

    let mut state = AppState::new(home.to_string_lossy().to_string());
    state.rules = Some(molt_rules::load(&state.home, None).unwrap());
    apply_result(&mut state, vec![cand(&cache, 4096)]);

    molt_gui::action::apply(
        &mut state,
        Action::PrepareClean(molt_act::CleanMode::Quarantine),
    );
    assert!(state.pending_clean.is_some());

    let fx = molt_gui::action::apply(&mut state, Action::CancelPendingClean);
    assert!(fx.is_empty(), "取消不该产生任何执行意图：{fx:?}");
    assert!(state.pending_clean.is_none(), "取消后批次必须被丢掉");
    assert!(cache.exists(), "取消了就什么东西都不能动");
    assert!(
        !state.is_cleaning(),
        "取消了绝不能还在后台跑清理：{}",
        state.activity().blocking_hint()
    );

    let _ = std::fs::remove_dir_all(&home);
}
