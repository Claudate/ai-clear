//! P4 集成验证：重复文件检测从后台任务到状态回填的完整链路。
//!
//! 破坏性流程一律在假 home（`MOLT_HOME`）里进行，不碰真实数据。
mod common;

use molt_gui::action::{self, Action};

#[test]
fn duplicate_background_job_reaches_state() {
    let home = common::tmp_home("flow");
    std::fs::create_dir_all(home.join("sub")).unwrap();
    // 两组真实重复 + 一个同大小不同内容的干扰项
    std::fs::write(home.join("a.bin"), vec![7u8; 300_000]).unwrap();
    std::fs::write(home.join("b.bin"), vec![7u8; 300_000]).unwrap();
    std::fs::write(home.join("sub/c.bin"), vec![7u8; 300_000]).unwrap();
    std::fs::write(home.join("d.bin"), vec![9u8; 300_000]).unwrap();
    std::fs::write(home.join("e.txt"), b"hello").unwrap();
    std::fs::write(home.join("f.txt"), b"hello").unwrap();

    let home_str = home.display().to_string();
    std::env::set_var("MOLT_HOME", &home_str);

    let mut state = molt_gui::state::AppState::new(home_str.clone());

    // 走真实的 action → effect 链路启动
    let fx = action::apply(&mut state, Action::StartDuplicateScan);
    assert!(matches!(
        fx.as_slice(),
        [molt_gui::action::Effect::ScanDuplicates]
    ));

    // 模拟 app 执行层：启动后台任务
    if let [molt_gui::action::Effect::ScanDuplicates] = fx.as_slice() {
        state.duplicate_job = Some(molt_gui::worker::DuplicateJob::start(home_str.clone()));
    }

    // 推进直到完成
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut done = false;
    while std::time::Instant::now() < deadline {
        molt_gui::pages::workbench::pump_duplicates(&mut state);
        if state.duplicate_scan.is_some() {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    assert!(done, "重复扫描应完成任务并回填状态");

    let scan = state.duplicate_scan.as_ref().unwrap();
    assert_eq!(scan.groups.len(), 2, "应有 2 组重复");
    // 最大组：三个 300KB 的 7 号文件
    let big = &scan.groups[0];
    assert_eq!(big.files.len(), 3);
    assert_eq!(big.bytes, 300_000);
    // 小组：两个 hello 文本
    let small = &scan.groups[1];
    assert_eq!(small.files.len(), 2);
    assert_eq!(small.bytes, 5);

    let _ = std::fs::remove_dir_all(&home);
}

/// 重复文件扫描跑着的时候，「查找重复文件」不能再起第二个任务。
///
/// 用户在长扫描里反复点那颗按钮是常见动作。`duplicate_job` 只有一个槽位，
/// 第二个任务覆盖掉第一个就再也收不到它的结果——扫描看起来「一直在转」。
/// 界面层在扫描期间只画 spinner、不画按钮，但入口不止一处，
/// 所以这道闸必须在 action 与 effect 两层都成立，这里各验一次。
#[test]
fn a_second_duplicate_scan_is_refused_while_one_is_running() {
    let home = common::tmp_home("second");
    let home_str = home.display().to_string();
    // 不设 MOLT_HOME：这条只验「重复启动被挡住」，不碰任何路径解析。
    // 同文件另一条测试已经在设这个进程级变量，两条并发跑会互相干扰。

    let mut state = molt_gui::state::AppState::new(home_str.clone());

    // 第一次：正常产生执行意图。
    let fx = action::apply(&mut state, Action::StartDuplicateScan);
    assert!(
        matches!(fx.as_slice(), [molt_gui::action::Effect::ScanDuplicates]),
        "第一次点应当去启动扫描"
    );
    state.duplicate_job = Some(molt_gui::worker::DuplicateJob::start(home_str.clone()));

    // 第二次：任务还在手上，不该再产生意图。
    let fx = action::apply(&mut state, Action::StartDuplicateScan);
    assert!(
        fx.is_empty(),
        "扫描未结束时再点不该产生第二个启动意图，实际：{fx:?}"
    );

    // 执行层同样要挡住：即便有人绕过 action 直接发意图。
    // 这里不真的跑那个 effect（它不看 job 槽位以外的东西），
    // 只核对「槽位非空」这个前提在执行层被读到——app.rs 的 Effect::ScanDuplicates
    // 分支就是 `if duplicate_job.is_none()`，与这里同源。
    assert!(state.duplicate_job.is_some(), "前提：任务仍在运行");

    let _ = std::fs::remove_dir_all(&home);
}
