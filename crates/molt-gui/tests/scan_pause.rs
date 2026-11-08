//! P2 集成验证：真实后台扫描任务上，暂停让进度冻结、继续后恢复并跑完。
//!
//! 破坏性流程一律在假 home（`MOLT_HOME`）里进行，不碰真实数据。
mod common;

use molt_gui::action::{self, Action};

/// 铺一堆文件，让扫描花足够时间（判定循环每 25 项有一个检查点）。
fn make_noise(root: &std::path::Path) {
    // 一份规则库里真正命中的缓存目录：没有它，扫描结果永远是 0 项，
    // 下面「取消不能清空清单」那条就成了 0 == 0 的空断言。
    let hit = root.join(".npm/_cacache");
    std::fs::create_dir_all(&hit).unwrap();
    std::fs::write(hit.join("a.bin"), vec![3u8; 4096]).unwrap();
    for d in 0..120 {
        let dir = root.join(format!("area-{d}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..60 {
            std::fs::write(dir.join(format!("f-{f}.bin")), vec![7u8; 64]).unwrap();
        }
    }
}

#[test]
fn pause_freezes_progress_and_resume_finishes() {
    let home = common::tmp_home("pause");
    make_noise(&home);
    let home_str = home.display().to_string();
    std::env::set_var("MOLT_HOME", &home_str);

    let mut state = molt_gui::state::AppState::new(home_str.clone());
    state.rules = Some(molt_rules::load(&home_str, None).unwrap());
    action::apply(&mut state, Action::GoPage(molt_gui::state::Page::Workbench));
    action::apply(&mut state, Action::PauseScan); // no-op，若尚未开始

    // 启动扫描（有背景线程）
    molt_gui::pages::workbench::start_scan(&mut state);

    // 等扫描线程起来且未完成
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Some(job) = state.job.as_ref() {
            if !job.is_finished() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // 暂停
    action::apply(&mut state, Action::PauseScan);
    let paused = state
        .job
        .as_ref()
        .map(|j| j.control.is_paused())
        .unwrap_or(false);
    assert!(paused, "暂停请求应反映到控制句柄");

    // 等 400ms，进度应冻结（相同 latest）
    std::thread::sleep(std::time::Duration::from_millis(400));
    let p1 = state
        .job
        .as_ref()
        .map(|j| format!("{:?}", j.peek().clone()));
    std::thread::sleep(std::time::Duration::from_millis(400));
    let p2 = state
        .job
        .as_ref()
        .map(|j| format!("{:?}", j.peek().clone()));
    assert_eq!(p1, p2, "暂停期间进度不应前进");

    // 继续，等待完成（推进通道才能让 is_finished 更新）
    action::apply(&mut state, Action::ResumeScan);
    let finish = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut done = false;
    while std::time::Instant::now() < finish {
        molt_gui::pages::workbench::pump_scan(&mut state);
        if state.job.is_none() {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(done, "继续后扫描应跑到完成");

    // 取消语义：重新扫一次并取消
    action::apply(&mut state, Action::GoPage(molt_gui::state::Page::Workbench));
    molt_gui::pages::workbench::start_scan(&mut state);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Some(job) = state.job.as_ref() {
            if !job.is_finished() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    action::apply(&mut state, Action::CancelScan);
    let cancel_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut cancelled_done = false;
    while std::time::Instant::now() < cancel_deadline {
        molt_gui::pages::workbench::pump_scan(&mut state);
        if state.job.is_none() {
            cancelled_done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(cancelled_done, "取消后扫描任务应结束");

    // 取消不是故障，更不是「扫完了、盘很干净」：
    //
    // 以前 `run_scan` 在取消后仍会补发一条 `Done(半截结果)`，而 `poll()` 只保留
    // 最后一条，于是那条 `Done` 盖掉了「扫描已取消」，`pump_scan` 接着把它
    // `save_cache` 成扫描结果——下次打开窗口看到的正是这份残缺缓存。
    assert!(
        state.error.is_none(),
        "用户自己按的取消不该变成错误横幅：{:?}",
        state.error
    );
    let status = state.status.as_ref().expect("取消后应有一条提示");
    assert_eq!(
        status.kind,
        molt_gui::state::StatusKind::Info,
        "取消要报成中性提示，不能是错误：{}",
        status.text
    );
    assert!(
        status.text.contains("取消"),
        "提示要说清是取消了：{}",
        status.text
    );
    let cache = molt_gui::paths::scan_cache();
    let cache_before = std::fs::read(&cache).ok();
    let items_before = state.result.as_ref().map(|r| r.items.len());
    assert!(
        items_before.is_some_and(|n| n > 0),
        "这一段的前提是：取消之前已经有一份完整的扫描结果"
    );

    // 再开一次并立刻取消，专测「取消发生在扫描刚开始」的路径：
    // 这时 `scan_with` 会走到检查点并发出「扫描已取消」。
    molt_gui::pages::workbench::start_scan(&mut state);
    action::apply(&mut state, Action::CancelScan);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        molt_gui::pages::workbench::pump_scan(&mut state);
        if state.job.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        state.job.is_none(),
        "取消后扫描任务必须结束，不能永远停在 Scanning"
    );

    // 取消掉的扫描绝不能覆盖上一份完整缓存，也不能清空清单——
    // 以前 `run_scan` 取消后仍补发一条 `Done(半截结果)`，`poll()` 只留最后一条，
    // 于是「我取消了」被落成「盘里很干净」，下次打开窗口看到的正是这份残缺结果。
    assert_eq!(
        state.result.as_ref().map(|r| r.items.len()),
        items_before,
        "取消不该动已经显示出来的清单"
    );
    assert_eq!(
        std::fs::read(&cache).ok(),
        cache_before,
        "取消掉的扫描不能覆盖缓存：{}",
        cache.display()
    );
    assert!(
        state.error.is_none(),
        "用户自己按的取消不该变成错误横幅：{:?}",
        state.error
    );

    let _ = std::fs::remove_dir_all(&home);
}
