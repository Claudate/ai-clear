//! 端到端走查：在假 home 里真的卸载一个应用。
//!
//! 与 `clean_paths.rs` 分开放，不是分类洁癖：这两个测试都改进程级的 `HOME`，
//! 同一个测试二进制里会并行跑、互相污染（先是「清理应转到后台执行」莫名失败，
//! 再是卸载那条读到了别人建出来的目录）。一个测试一个进程，彼此才碰不到。
//!
//! 它走的是界面层真正会调用的那几个函数——`action::apply` 装批次、
//! `workbench::start_uninstall` 起任务、`uninstall::pump_done` 收尾——
//! 而不是绕开界面直接调 `molt-act`，否则验的是底层，不是用户右键「卸载」之后
//! 按下确定会发生什么。破坏性流程只在假 home 里验，这是 2026-10-21 那次误清留下的规矩。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use molt_gui::action::{apply, Action, Effect};
use molt_gui::state::{AppRow, AppState, LeftoverRow};

fn tmp_home(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "molt-uninstall-{}-{}-{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn make_dir_with_file(path: &Path, name: &str, bytes: usize) -> PathBuf {
    std::fs::create_dir_all(path).unwrap();
    let f = path.join(name);
    std::fs::write(&f, vec![7u8; bytes]).unwrap();
    f
}

fn leftover(path: &Path, category: &'static str, bytes: u64) -> LeftoverRow {
    LeftoverRow {
        path: path.display().to_string(),
        display: path.display().to_string(),
        category,
        bytes,
        needs_admin: false,
        unmeasured: false,
    }
}

#[test]
fn uninstall_moves_body_and_leftovers_to_quarantine_in_a_fake_home() {
    let home = tmp_home("flow");
    // 先设 HOME：隔离区、审计库、授权根全部跟着它走，碰不到真实缓存。
    std::env::set_var("HOME", &home);
    let home_str = home.to_string_lossy().to_string();

    let app = home.join("Applications/Demo.app");
    let cache = home.join("Library/Caches/com.example.demo");
    let support = home.join("Library/Application Support/Demo");
    make_dir_with_file(&app, "Demo", 8192);
    make_dir_with_file(&cache, "c.bin", 1024);
    make_dir_with_file(&support, "s.bin", 2048);

    let mut state = AppState::new(home_str.clone());
    state.rules = Some(molt_rules::load(&home_str, None).unwrap());
    state.uninstall.apps = vec![AppRow {
        name: "Demo".into(),
        path: app.display().to_string(),
        bundle_id: Some("com.example.demo".into()),
        bytes: 8192,
        bytes_known: true,
        leftovers: vec![
            leftover(&cache, "缓存", 1024),
            leftover(&support, "应用数据", 2048),
        ],
        leftover_bytes: 1024 + 2048,
        icon_file: None,
        // 夹具的残留是现成的，等同于「已经扫过」
        leftovers_loaded: true,
    }];

    // 右键「卸载」装批次；确认框按下确定。
    let fx = apply(
        &mut state,
        Action::PrepareUninstall(app.display().to_string()),
    );
    assert!(fx.is_empty(), "残留已扫过，不该再派扫描：{fx:?}");
    assert_eq!(
        state.pending_clean.as_ref().map(|p| p.count()),
        Some(3),
        "本体 + 两份残留一起进批次"
    );

    let fx = apply(&mut state, Action::RunUninstall);
    assert_eq!(fx.len(), 1, "应当只派一个卸载效果");
    let Effect::Uninstall(pending) = &fx[0] else {
        panic!("卸载必须走 Effect::Uninstall，实际：{fx:?}");
    };
    molt_gui::pages::workbench::start_uninstall(&mut state, pending);

    // 收尾靠磁盘事实：本体从盘上消失才算卸掉。
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        molt_gui::pages::workbench::pump_clean(&mut state);
        molt_gui::pages::uninstall::pump_done(&mut state);
        if state.uninstall.removing.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(!app.exists(), "应用本体必须从原位置移走");
    assert!(!cache.exists(), "残留缓存必须一起移走");
    assert!(!support.exists(), "残留应用数据必须一起移走");

    // 隔离区里有三条且都能还原：卸载不是不可逆的。
    molt_gui::pages::quarantine::reload(&mut state);
    assert_eq!(state.quarantine.len(), 3, "三项都应进临时空间");
    assert!(
        state.quarantine.iter().all(|r| r.restorable),
        "刚移进去的都应是可还原状态"
    );
    let q_bytes: u64 = state.quarantine.iter().map(|r| r.bytes).sum();
    assert_eq!(q_bytes, 8192 + 1024 + 2048);

    // 界面：网格里不该再有这个应用，状态条要说清东西去了哪。
    assert!(
        state
            .uninstall
            .apps
            .iter()
            .all(|a| a.path != app.display().to_string()),
        "卸掉的应已从网格移除"
    );
    let status = state.status.as_ref().expect("应当有结果提示");
    assert!(status.text.contains("Demo"), "实际：{}", status.text);
    assert!(status.text.contains("可还原"), "实际：{}", status.text);

    let _ = std::fs::remove_dir_all(&home);
}
