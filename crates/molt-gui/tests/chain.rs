//! 链路验收：扫描 → AI 解释 → 清理 → 定时任务 → 反馈，在假 home 里真跑一遍。
//!
//! 与 `clean_paths.rs` 的分工：那边验两条清理落地路径（隔离 / 永久删除）的
//! 文件事实；这里验这次重做新增的三条约束：
//!
//! 1. **互斥**：有任务在跑时不能再开始任何会冲突的动作，且状态里能拿到理由；
//! 2. **定时清理**：只清勾选的 auto 类型、只移入隔离区、结果写回上次执行记录；
//! 3. **AI 解释落到条目上**：模型给的说明按路径挂到对应数据上，不是另开一页。
//!
//! 破坏性流程只在假 home 里做（`MOLT_HOME`），碰不到真实缓存。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use molt_core::{Candidate, ItemKind, Verdict};
use molt_gui::state::{Activity, AppState, Priority, StatusKind};

fn tmp_home(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "molt-chain-{}-{}-{}",
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

fn make_dir_with_file(path: &Path, name: &str, bytes: usize) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(path.join(name), vec![3u8; bytes]).unwrap();
}

fn cat(path: &Path, bytes: u64, v: Verdict, rule: &str) -> Candidate {
    let mut c = Candidate::new(path, ItemKind::Dir, v).with_size(bytes, bytes, 1);
    c.rule_id = Some(rule.to_string());
    c
}

fn apply(state: &mut AppState, items: Vec<Candidate>) {
    state.apply_result(Arc::new(molt_gui::worker::ScanResult {
        items,
        rule_ids: vec![],
        denied_roots: vec![],
        elapsed_secs: 0.1,
    }));
}

/// 整个进程只有一个测试：改 `MOLT_HOME` 是进程级的，并行会互相污染。
#[test]
fn scan_ai_clean_schedule_feedback_chain() {
    let home = tmp_home("chain");
    std::env::set_var("MOLT_HOME", &home);
    let home_str = home.to_string_lossy().to_string();

    // ---- 1. 互斥：一个任务在跑时，别的动作都开不了，且能说出原因 ----
    let mut state = AppState::new(home_str.clone());
    assert_eq!(state.activity(), Activity::Idle);
    assert!(state.can_start());

    // 用一个刚开始的后台扫描占住活动状态。它会自己跑完，
    // 但下面几行的断言发生在它结束之前，这正是互斥要覆盖的窗口。
    let rules = molt_rules::load(&home_str, None).unwrap();
    state.rules = Some(rules.clone());
    state.job = Some(molt_gui::worker::ScanJob::start(
        rules.clone(),
        home_str.clone(),
        false,
    ));
    assert_eq!(state.activity(), Activity::Scanning);
    assert!(!state.can_start(), "扫描进行中不能再开清理");
    assert!(
        !state.activity().blocking_hint().is_empty(),
        "置灰必须能给出理由"
    );

    // 直接调用入口也要被挡住：不能只靠界面把按钮画灰
    let before = state.job.as_ref().map(|j| j.is_finished());
    molt_gui::pages::workbench::start_scan(&mut state);
    assert_eq!(
        state.job.as_ref().map(|j| j.is_finished()),
        before,
        "扫描中再点扫描不该起第二个任务"
    );
    let status_kind = state.status.as_ref().map(|s| s.kind);
    assert_eq!(
        status_kind,
        Some(StatusKind::Warn),
        "被互斥挡住时要给明确反馈，不能静默无反应"
    );

    // 清理入口同样被挡住，且不会生成待确认批次
    state.pending_clean = state.pending_from_checked(molt_act::CleanMode::Quarantine);
    let n = state.checked_count();
    let _ = n;
    molt_gui::pages::workbench::do_clean(&mut state);
    assert!(!state.is_cleaning(), "扫描进行中不能开始清理");

    // ---- 2. AI 解释落到条目上 ----
    let target = home.join("project/demo/target");
    let cache = home.join(".npm/_cacache");
    make_dir_with_file(&target, "bin", 2048);
    make_dir_with_file(&cache, "a.bin", 1024);

    let mut costly = molt_core::Verdict::Costly {
        rebuild: molt_core::RebuildCost::new(molt_core::CostKind::Recompile, "重编约 1 分钟"),
    };
    let _ = &mut costly;
    let items = vec![
        cat(&target, 2048, costly, "rust-target"),
        cat(&cache, 1024, Verdict::Reclaimable, "npm-cache"),
    ];
    // 等扫描结束再落数据，避免两个来源互相覆盖
    wait_idle(&mut state);
    apply(&mut state, items);

    // 模型回复：逐条解释 + 一条明确「不要动」
    let reply = r#"{
        "summary": "两个目录，一个是构建产物，一个是包缓存",
        "proposals": [{"path": "~/.npm/_cacache", "action": "quarantine", "reason": "可重建"}],
        "explanations": [
            {"path": "~/project/demo/target", "purpose": "cargo build 的输出目录", "safe_to_clean": false, "reason": "下次构建要重编"},
            {"path": "~/.npm/_cacache", "purpose": "npm 下载缓存", "safe_to_clean": true, "reason": "删了只会让下次安装慢一点"}
        ]
    }"#;
    let report = molt_ai::AiReport::parse(reply).expect("应能解析");
    let notes = molt_ai::explanations(&report, &state.result.clone().unwrap().items, &home_str);
    assert_eq!(notes.len(), 2, "两条都要挂上说明");
    assert!(notes
        .get(&target)
        .unwrap()
        .contains("cargo build 的输出目录"));

    // 把它按界面那条路径塞进状态，确认逐行能取到
    state.ai.notes = notes
        .iter()
        .map(|(p, n)| (p.display().to_string(), n.clone()))
        .collect();
    state.ai.safe = vec![(target.display().to_string(), false)]
        .into_iter()
        .collect();

    let r = state.result.clone().unwrap();
    let t_idx = r.items.iter().position(|c| c.path == target).unwrap();
    assert_eq!(
        state.priority_of(&r.items[t_idx]),
        Priority::Keep,
        "AI 说不要动的条目要排进「建议保留」"
    );
    assert_eq!(
        state
            .ai_note(&target.display().to_string())
            .map(|s| s.contains("输出目录")),
        Some(true)
    );

    // ---- 2b. 首页那颗大按钮：走的就是默认勾选那批，不另算一批 ----
    // 方案要求验证「点按钮 → 确认框里的数量 == 建议清掉 + 可以清」。
    // 直接调 hero_action，而不是在测试里重写一遍分支。
    state.ai.safe.clear(); // 上一步 AI 把 target 挪进了「建议留着」，先还原
    molt_gui::pages::workbench::hero_action(&mut state);
    let pending = state
        .pending_clean
        .clone()
        .expect("有可清量时应生成待确认批次");
    assert_eq!(
        pending.items.len(),
        state.cleanable_summary().0,
        "首页按钮的批次必须等于「建议清掉 + 可以清」，否则首页报的 GB 和确认框对不上"
    );
    assert_eq!(
        pending.mode,
        molt_act::CleanMode::Quarantine,
        "放入临时空间是首页的默认选项"
    );
    state.pending_clean = None;

    // ---- 3. 定时清理：只清勾选的 auto 类型，且只走隔离区 ----
    // 规则库里的真实 id：npm-cache 是 auto 且可回收；rust-target 不是 auto。
    let cfg = molt_gui::schedule::ScheduleConfig {
        enabled: true,
        frequency: molt_gui::schedule::Frequency::Daily,
        hour: 3,
        minute: 0,
        // 故意把有代价的 rust-target 也写进来：它必须被过滤掉
        rule_ids: vec!["npm-cache".into(), "rust-target".into()],
        ..Default::default()
    };
    let run = molt_gui::schedule::run_once(&rules, &home_str, &cfg);

    assert!(run.ok, "应当成功：{}", run.message);
    assert_eq!(run.done, 1, "只有 npm 缓存那一项该被处理：{}", run.message);
    assert!(!cache.exists(), "勾选的类型必须真的被移走");
    assert!(
        target.exists(),
        "没标 auto 的类型（哪怕是勾着的）绝不能被动"
    );
    assert!(run.bytes >= 1024, "要报出处理量，实际 {}", run.bytes);

    // 只移入隔离区：审计里应有一条待还原记录，而不是直接删除
    let store = molt_act::AuditStore::open(&molt_act::paths::audit_db()).unwrap();
    let pending = store.list_pending(100).unwrap();
    assert_eq!(pending.len(), 1, "定时清理只能进隔离区，不能直接删");
    assert_eq!(pending[0].source, molt_act::CleanSource::Auto);

    // 上次执行结果要写回磁盘，界面才能显示它
    let saved = molt_gui::schedule::load_last_run().expect("应写出上次执行结果");
    assert!(saved.ok);
    assert_eq!(saved.done, 1);
    assert_eq!(saved.types, 2, "记录的是用户勾了几个类型");
    assert_eq!(
        saved.types_effective, 1,
        "勾选的两个里只有一个允许自动清理，要能看出这一点"
    );

    // ---- 4. 结果反馈：失败要带原因，成功要说清数量 ----
    assert!(
        run.message.contains("1 项"),
        "结论里要有数量：{}",
        run.message
    );

    // 一条删不掉的内容：父目录去掉写权限
    let locked_parent = home.join("locked");
    let locked = locked_parent.join("locked-cache");
    make_dir_with_file(&locked, "l.bin", 512);
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&locked_parent).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&locked_parent, perms).unwrap();

    // ---- 5. 清理这条链路：确认 → 执行 → 反馈 ----
    wait_idle(&mut state);
    apply(
        &mut state,
        vec![
            cat(&locked, 512, Verdict::Reclaimable, "npm-cache"),
            // 一条能成功的：用来确认「成功」时结果条给出的是「去隔离区」，
            // 而不是把用户留在原地自己想下一步。
            cat(
                &home.join("ok-cache"),
                256,
                Verdict::Reclaimable,
                "npm-cache",
            ),
        ],
    );
    make_dir_with_file(&home.join("ok-cache"), "k.bin", 256);
    let n = state.checked_count();
    assert_eq!(n, 2);
    state.pending_clean = state.pending_from_checked(molt_act::CleanMode::Delete);
    molt_gui::pages::workbench::do_clean(&mut state);
    wait_clean(&mut state);

    assert!(locked.exists(), "删不掉的内容必须原地不动");
    let op = state.last_op.as_ref().expect("应有结果报告");
    assert_eq!(
        op.kind,
        StatusKind::Warn,
        "一部分没做成要报 Warn，不能说成全部成功：{}",
        op.summary
    );
    assert!(
        !op.failures.is_empty() && !op.failures[0].1.is_empty(),
        "失败必须带原因"
    );
    // 有失败时下一步是「重试这些」，而不是把用户送去别处
    assert_eq!(
        op.next,
        Some(molt_gui::state::NextStep::Retry),
        "部分失败要给出一条重试的路"
    );

    let mut perms = std::fs::metadata(&locked_parent).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&locked_parent, perms).unwrap();

    // 全部成功时：结果条要指向「刚刚移走的东西去哪了」
    wait_idle(&mut state);
    apply(
        &mut state,
        vec![cat(
            &home.join("ok-cache-2"),
            128,
            Verdict::Reclaimable,
            "npm-cache",
        )],
    );
    make_dir_with_file(&home.join("ok-cache-2"), "k.bin", 128);
    state.pending_clean = state.pending_from_checked(molt_act::CleanMode::Quarantine);
    molt_gui::pages::workbench::do_clean(&mut state);
    wait_clean(&mut state);
    let op = state.last_op.as_ref().expect("应有结果报告");
    assert_eq!(op.kind, StatusKind::Info, "全成时要报成功：{}", op.summary);
    assert_eq!(
        op.next,
        Some(molt_gui::state::NextStep::Quarantine),
        "移入隔离区成功后要能直接去隔离区"
    );
    // 点那一步真的会切页，而不是只画了个按钮
    molt_gui::app::apply_report_action(
        &mut state,
        molt_gui::widgets::ReportAction::Next(molt_gui::state::NextStep::Quarantine),
    );
    assert_eq!(state.page, molt_gui::state::Page::Quarantine);
    assert!(
        state.quarantine.iter().any(|r| r.restorable),
        "切过去之后隔离区里要真的有可还原的东西"
    );

    // ---- 6. 隔离区里能还原那条定时清理移走的内容 ----
    molt_gui::pages::quarantine::reload(&mut state);
    // 不数总数：这轮清理自己也往隔离区放了一条，按路径找才不受前面几步影响。
    let row = state
        .quarantine
        .iter()
        .find(|r| r.restorable && r.original.ends_with("_cacache"))
        .expect("定时清理移走的 npm 缓存应当还在隔离区里");
    let id = row.id;
    molt_gui::pages::quarantine::restore(&mut state, id);
    assert!(cache.exists(), "定时清理走的是可还原路径，必须能拿回来");

    // ---- 6. 设置页填密钥：落盘、收紧权限、立刻生效（不用重启） ----
    // 这是「不许让用户去改环境变量」那条要求的落地证据。
    // 选一个真正需要密钥的服务：本地 ollama / jev 本来就不要密钥，
    // 拿它们验「密钥保存」什么都验不出来。
    state.ai.config.set_provider("deepseek");
    state.key_draft = "sk-test-not-a-real-key".into();
    molt_gui::pages::settings::save_api_key(&mut state);

    assert!(
        state.key_draft.is_empty(),
        "保存后必须清空输入框（状态栏：{:?}）：留着它就有被截图拍到的风险",
        state.status.as_ref().map(|s| &s.text)
    );
    let path = state.ai.config_path.clone();
    let text = std::fs::read_to_string(&path).expect("配置文件应已写出");
    assert!(
        text.contains("sk-test-not-a-real-key"),
        "密钥要写进配置文件，而不是只存在内存里"
    );
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "密钥文件必须只有本人可读，实际 {mode:o}");
    assert!(
        state.ai.resolve().is_ok(),
        "写完就该能解析出可用配置，不需要重启：{:?}（服务 {} / 模型 {}）",
        state.ai.resolve().err(),
        state.ai.config.active_provider,
        state.ai.config.active_model
    );

    let _ = std::fs::remove_dir_all(&home);
}

fn wait_idle(state: &mut AppState) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        molt_gui::pages::workbench::pump_scan(state);
        if !state.is_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("扫描没有在 20 秒内结束");
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
