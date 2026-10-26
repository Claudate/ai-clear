//! 端到端走查：在假 home 里把两条清理路径都真的跑一遍。
//!
//! 这个测试**会删文件**，所以每个路径都建在临时目录里，并且把 `HOME`
//! 指到那里——隔离区、审计库、授权根全部跟着 `HOME` 走，碰不到真实缓存。
//! 这正是 2026-10-21 那次误清留下的规矩：破坏性流程的验收只在假 home 里做。
//!
//! 它跑的是界面层真正会调用的那几个函数（`do_clean` / `pump_clean` /
//! `quarantine::reload` / `do_purge` / `pump_purge`），而不是绕过界面直接调
//! `molt-act`——否则验的是底层，不是用户按下去会发生什么。
//!
//! 整个进程只有一个测试：改 `HOME` 是进程级的，并行跑会互相污染。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use molt_core::{Candidate, ItemKind, Verdict};
use molt_gui::state::{AppState, StatusKind};

fn tmp_home(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "molt-flow-{}-{}-{}",
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

fn cand(path: &Path, bytes: u64) -> Candidate {
    Candidate::new(path, ItemKind::Dir, Verdict::Reclaimable).with_size(bytes, bytes, 3)
}

fn apply(state: &mut AppState, items: Vec<Candidate>) {
    state.apply_result(Arc::new(molt_gui::worker::ScanResult {
        items,
        rule_ids: vec![],
        denied_roots: vec![],
        elapsed_secs: 1.0,
    }));
}

/// 等一次清理跑完，并等它触发的自动重扫也结束。
///
/// 清理完成后界面会重扫一次（清单要显示清理后的真实状态），
/// 那段时间里互斥会把下一次操作挡住——测试必须照用户会遇到的那样等它结束。
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

fn wait_purge(state: &mut AppState) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        molt_gui::pages::quarantine::pump_purge(state);
        if !state.is_purging() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("彻底删除没有在 20 秒内完成");
}

#[test]
fn both_clean_paths_run_for_real_in_a_fake_home() {
    let home = tmp_home("paths");
    // 先设 HOME：后面所有 paths::* 都会读它
    std::env::set_var("HOME", &home);
    let home_str = home.to_string_lossy().to_string();

    // ---- 准备三份内容：两份走隔离区，一份走直接清理 ----
    let qa = home.join(".npm/_cacache");
    let qb = home.join("Library/Caches/Cursor/CachedData");
    let del = home.join("proj/target");
    make_dir_with_file(&qa, "a.bin", 4096);
    make_dir_with_file(&qb, "b.bin", 8192);
    make_dir_with_file(&del, "d.bin", 16384);

    let mut state = AppState::new(home_str.clone());
    state.rules = Some(molt_rules::load(&home_str, None).unwrap());
    let items = vec![cand(&qa, 4096), cand(&qb, 8192), cand(&del, 16384)];
    apply(&mut state, items);

    // 默认只勾可回收项，三项都是，所以已选 3
    assert_eq!(state.checked_count(), 3, "三项可回收应默认全选");
    assert_eq!(state.checked_bytes(), 4096 + 8192 + 16384);

    // ---- 路径一：移入隔离区 ----
    // 只选前两项。取消第三项，模拟用户只清缓存、保留构建产物。
    state.toggle(&del.display().to_string());
    assert_eq!(state.checked_count(), 2, "取消后应实时少一项");

    let pending = state
        .pending_from_checked(molt_act::CleanMode::Quarantine)
        .expect("应生成待确认批次");
    assert_eq!(pending.count(), 2, "确认框要显示将处理的对象数");
    assert_eq!(pending.bytes, 4096 + 8192);
    assert!(
        pending.items.iter().any(|i| i.path == qa),
        "确认框要能列出具体对象"
    );

    state.pending_clean = Some(pending);
    molt_gui::pages::workbench::do_clean(&mut state);
    assert!(state.is_cleaning(), "清理应转到后台执行");
    wait_clean(&mut state);

    // 原位置已空，构建产物还在
    assert!(!qa.exists(), "放入临时空间后原位置应为空");
    assert!(!qb.exists());
    assert!(del.exists(), "没选中的项必须原地不动");

    // 结果反馈：说清是放入临时空间，且不说「释放」
    let op = state.last_op.as_ref().expect("应有结果报告");
    assert_eq!(op.kind, StatusKind::Info, "全部成功应是 Info");
    assert!(op.summary.contains("放入临时空间"), "实际：{}", op.summary);
    assert!(
        !op.summary.contains("释放"),
        "隔离不释放空间：{}",
        op.summary
    );
    assert!(op.failures.is_empty());

    // 隔离区状态已刷新（pump_clean 里会调 reload）
    molt_gui::pages::quarantine::reload(&mut state);
    assert_eq!(state.quarantine.len(), 2, "隔离区里应有两条");
    assert!(
        state.quarantine.iter().all(|r| r.restorable),
        "刚移进去的条目都应是可还原状态"
    );
    let q_bytes: u64 = state.quarantine.iter().map(|r| r.bytes).sum();
    assert_eq!(q_bytes, 4096 + 8192);

    // 清单侧的状态也应刷新：重扫后那两项不该再出现在结果里
    // （pump_clean 已触发重扫，这里直接读磁盘事实，不依赖扫描时序）

    // ---- 路径二：直接清理（不经过隔离区）----
    apply(&mut state, vec![cand(&del, 16384)]);
    assert_eq!(state.checked_count(), 1, "重扫后应默认勾选可回收项");
    let pending = state
        .pending_from_checked(molt_act::CleanMode::Delete)
        .expect("应生成删除批次");
    assert_eq!(pending.count(), 1);
    state.pending_clean = Some(pending);
    molt_gui::pages::workbench::do_clean(&mut state);
    wait_clean(&mut state);

    assert!(!del.exists(), "直接清理必须真的从磁盘上删掉");
    let op = state.last_op.as_ref().expect("应有结果报告");
    assert_eq!(op.kind, StatusKind::Info);
    assert!(op.summary.contains("已永久删除"), "实际：{}", op.summary);
    assert!(
        op.summary.contains("16.0 KB"),
        "要说清释放了多少：{}",
        op.summary
    );

    // 直接清理的不该进隔离区：隔离区仍然只有前面那两条
    molt_gui::pages::quarantine::reload(&mut state);
    assert_eq!(
        state.quarantine.len(),
        2,
        "永久删除不经过隔离区，里面不该多东西"
    );

    // 历史：应有 2 条 quarantine + 1 条 purge（直接删除记为 purge 且无隔离路径）
    molt_gui::pages::history::reload(&mut state);
    assert_eq!(state.history.len(), 3, "历史应记全三次操作");
    use molt_gui::state::HistoryAction;
    assert_eq!(
        state
            .history
            .iter()
            .filter(|r| r.action == HistoryAction::Quarantined)
            .count(),
        2
    );
    assert_eq!(
        state
            .history
            .iter()
            .filter(|r| r.action == HistoryAction::Deleted)
            .count(),
        1,
        "直接清理要在历史里标成「直接清理」，不能混进隔离区"
    );

    // ---- 隔离区：还原一条 ----
    let restore_id = state
        .quarantine
        .iter()
        .find(|r| r.original.ends_with(".npm/_cacache"))
        .expect("应能找到 npm 那条")
        .id;
    molt_gui::pages::quarantine::restore(&mut state, restore_id);
    assert!(qa.exists(), "还原后内容必须回到原位置");
    assert!(
        state.status.as_ref().map(|s| s.kind) == Some(StatusKind::Info),
        "还原成功要给 Info 反馈"
    );
    molt_gui::pages::quarantine::reload(&mut state);
    assert_eq!(state.quarantine.len(), 1, "还原后隔离区应少一条");

    // ---- 隔离区：彻底删除 ----
    state.select_all_quarantine();
    assert_eq!(state.quarantine_checked_count(), 1, "只剩一条可还原");
    let pending = state
        .pending_purge_from_checked()
        .expect("应生成彻底删除批次");
    assert_eq!(pending.count(), 1);
    assert!(
        pending.items[0].path.to_string_lossy().contains("Cursor"),
        "确认框要列出将被彻底删除的对象"
    );
    state.pending_purge = Some(pending);
    molt_gui::pages::quarantine::do_purge(&mut state);
    wait_purge(&mut state);

    let op = state.last_op.as_ref().expect("应有结果报告");
    assert_eq!(op.kind, StatusKind::Info, "实际：{}", op.summary);
    assert!(op.summary.contains("彻底删除"), "实际：{}", op.summary);
    molt_gui::pages::quarantine::reload(&mut state);
    assert!(state.quarantine.is_empty(), "隔离区应已清空");
    assert!(!qb.exists(), "彻底删除后内容不能还在");

    // 审计里也不该再有待还原记录——否则界面会把已删的东西继续列出来
    let store = molt_act::AuditStore::open(&molt_act::paths::audit_db()).unwrap();
    assert!(
        store.list_pending(100).unwrap().is_empty(),
        "彻底删除后不该留下待还原记录"
    );

    // ---- 部分失败：一项删不掉，其余照常处理 ----
    let good = home.join("proj/good-cache");
    make_dir_with_file(&good, "g.bin", 1024);
    // 父目录去掉写权限 → 删不掉这一项（rmdir 需要父目录可写）
    let locked_parent = home.join("locked");
    let locked = locked_parent.join("locked-cache");
    make_dir_with_file(&locked, "l.bin", 1024);
    let mut perms = std::fs::metadata(&locked_parent).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o555);
    std::fs::set_permissions(&locked_parent, perms).unwrap();

    apply(&mut state, vec![cand(&good, 1024), cand(&locked, 1024)]);
    state.pending_clean = state.pending_from_checked(molt_act::CleanMode::Delete);
    molt_gui::pages::workbench::do_clean(&mut state);
    wait_clean(&mut state);

    assert!(!good.exists(), "能删的那一项必须照常处理");
    assert!(locked.exists(), "删不掉的那一项必须原地不动");
    let op = state.last_op.as_ref().expect("应有结果报告");
    assert_eq!(
        op.kind,
        StatusKind::Warn,
        "部分失败要报 Warn，不能显示成全部成功：{}",
        op.summary
    );
    assert_eq!(op.done, 1);
    assert_eq!(op.failures.len(), 1, "失败明细要逐条列出");
    assert!(
        !op.failures[0].1.is_empty(),
        "失败必须带原因，不能只给一个数字"
    );

    // 收尾：把权限恢复，否则临时目录删不掉
    let mut perms = std::fs::metadata(&locked_parent).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&locked_parent, perms).unwrap();
    let _ = std::fs::remove_dir_all(&home);
}
