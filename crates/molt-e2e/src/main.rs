//! 端到端验证：在一个隔离的临时家目录里跑完整链路。
//!
//! 走的是与 CLI 完全相同的公开 API——扫描、判定、清理、还原，
//! 只是绕开了交互式确认（那部分另有测试覆盖）。
//! 这个程序只在验证时手动运行，不参与发布。

use std::path::PathBuf;

fn main() {
    let home = std::env::args().nth(1).expect("usage: e2e <fake-home>");
    let home = PathBuf::from(home);

    println!("== 1. 准备 ==");
    let cache = home.join(".npm/_cacache/index");
    std::fs::create_dir_all(&cache).unwrap();
    let blob = cache.join("blob.bin");
    // 用固定字节，便于逐字节比对
    let content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&blob, &content).unwrap();
    let before = molt_act::hash::path_sha256(&blob).unwrap();
    println!("   写入 {} 字节，哈希 {}…", content.len(), &before[..16]);

    // 一个必须被保护的东西
    let secret = home.join("proj/.ssh");
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::write(
        secret.join("id_rsa"),
        b"-----BEGIN OPENSSH PRIVATE KEY-----",
    )
    .unwrap();

    println!("== 2. 扫描 ==");
    let home_str = home.to_string_lossy().to_string();
    let rules = molt_rules::load(&home_str, None).unwrap();
    let opts = molt_scan::WalkOptions {
        skip: molt_safety::skip_subtrees(&home_str),
        ..Default::default()
    };
    let found = molt_scan::discover(&rules, &opts, &home_str);
    let roots = molt_safety::normalize_roots(std::slice::from_ref(&home));

    let mut candidates = Vec::new();
    for (d, meta) in &found {
        let signals = molt_safety::collect_signals(&d.path, &home_str, &roots, false);
        let c = molt_core::decide(meta, &rules, &signals, &roots);
        println!(
            "   {:<12} {:<24} {}",
            format!("{:?}", c.verdict)
                .split(' ')
                .next()
                .unwrap()
                .trim_end_matches(','),
            d.rule_id,
            d.path.strip_prefix(&home).unwrap_or(&d.path).display()
        );
        candidates.push(c);
    }

    // 密钥目录被硬禁区在扫描阶段就剪掉了，所以它根本不出现在候选清单里。
    // 这是有意的：用户看到 ~/.ssh 被列出来本身就会不安。
    // 这里要验的是「它绝不进入可操作集合」，而不是「它出现在清单里」。
    let secret_path = home.join("proj/.ssh");
    assert!(
        !candidates.iter().any(|c| c.path == secret_path),
        "硬禁区内的路径不应出现在候选清单里"
    );
    assert!(
        candidates
            .iter()
            .all(|c| c.verdict.is_actionable() || !c.path.starts_with(&secret_path)),
        "任何落在密钥目录下的条目都必须是受保护状态"
    );
    println!("   ✓ 密钥目录已被硬禁区剪枝，不进候选");

    println!("== 3. 清理（移入隔离区）==");
    let audit = home.join("Library/Application Support/app.molt/audit.sqlite");
    let store = molt_act::AuditStore::open(&audit).unwrap();
    let q = molt_act::Quarantine::new(home.join("q"), "batch");
    let exec = molt_act::Executor::new(store, q, 7);

    let actionable: Vec<_> = candidates
        .iter()
        .filter(|c| c.verdict.is_actionable())
        .cloned()
        .collect();
    let conf = molt_act::Confirmation::new(&candidates);
    let outcome = exec.clean(&actionable, &conf);

    println!(
        "   移走 {} 项，失败 {} 项",
        outcome.moved.len(),
        outcome.failed.len()
    );
    for f in &outcome.failed {
        println!("   失败：{} — {}", f.path.display(), f.reason);
    }
    assert!(outcome.failed.is_empty(), "不应有失败项");
    assert!(!blob.exists(), "原位置应当已经空了");
    assert!(
        secret.join("id_rsa").exists(),
        "受保护的密钥文件必须原地不动"
    );
    println!("   ✓ 缓存已移走，密钥仍在原地");

    let id = outcome.moved[0].id;

    println!("== 4. 还原 ==");
    let restored = exec.restore(id).expect("还原应当成功");
    // 清理的粒度是规则命中的那个目录（`.npm/_cacache`），还原自然也是整个目录回来
    assert_eq!(restored, home.join(".npm/_cacache"));
    let after = molt_act::hash::path_sha256(&blob).unwrap();
    let reread = std::fs::read(&blob).unwrap();
    assert_eq!(reread, content, "还原后字节必须完全一致");
    assert_eq!(before, after, "还原后哈希必须一致");
    println!("   ✓ 逐字节还原，哈希一致");

    println!("== 5. 审计 ==");
    let all = exec.store().list_all(10).unwrap();
    assert_eq!(all.len(), 2, "应当有 quarantine 与 restore 两条记录");
    assert!(all
        .iter()
        .any(|r| r.action == molt_act::ActionKind::Restore));
    println!("   ✓ 审计时间线完整（{} 条）", all.len());

    println!("== 6. 无人值守清理 ==");
    // 这一段验的是「自动清理只碰规则自己声明过的项」这条约束，
    // 而不是「auto 能删东西」。前者才是产品承诺。
    {
        let home_str = home.to_string_lossy().to_string();
        let rules = molt_rules::load(&home_str, None).unwrap();

        // 构造两个可回收候选：一个来自声明了 auto 的规则，一个没有
        let auto_path = home.join(".codex/tmp/session-cache");
        let manual_path = home.join("proj/some-cache");
        std::fs::create_dir_all(&auto_path).unwrap();
        std::fs::create_dir_all(&manual_path).unwrap();
        std::fs::write(auto_path.join("a.bin"), vec![1u8; 4096]).unwrap();
        std::fs::write(manual_path.join("b.bin"), vec![2u8; 4096]).unwrap();

        let mut auto_c = molt_core::Candidate::new(
            &auto_path,
            molt_core::ItemKind::Dir,
            molt_core::Verdict::Reclaimable,
        )
        .with_size(4096, 4096, 1);
        auto_c.rule_id = Some("codex-logs".into());

        let mut manual_c = molt_core::Candidate::new(
            &manual_path,
            molt_core::ItemKind::Dir,
            molt_core::Verdict::Reclaimable,
        )
        .with_size(4096, 4096, 1);
        // `generic-build-caches` 是纯缓存，但刻意没有标 auto：
        // 它跨了 node-gyp / electron-builder / Homebrew 好几种工具，
        // 没人看着的时候不该替用户决定这些能不能清。
        manual_c.rule_id = Some("generic-build-caches".into());

        let conf = molt_act::Confirmation::for_auto(&[auto_c.clone(), manual_c.clone()], &rules);
        assert_eq!(conf.len(), 1, "只有声明了 auto 的项能进自动清理");
        assert!(conf.is_accepted(&auto_path), "codex-logs 应被接受");
        assert!(
            !conf.is_accepted(&manual_path),
            "npm-cache 未声明 auto，必须拒绝"
        );

        let store = molt_act::AuditStore::open(
            &home.join("Library/Application Support/app.molt/audit2.sqlite"),
        )
        .unwrap();
        let q = molt_act::Quarantine::new(home.join("q2"), "auto");
        let auto_exec =
            molt_act::Executor::new(store, q, 7).with_source(molt_act::CleanSource::Auto);
        let out = auto_exec.clean(&[auto_c, manual_c], &conf);
        assert_eq!(out.moved.len(), 1, "只应移走一项");

        let recs = auto_exec.store().list_all(10).unwrap();
        assert_eq!(
            recs[0].source,
            molt_act::CleanSource::Auto,
            "审计必须标明来源是 auto"
        );
        assert!(manual_path.exists(), "未声明 auto 的项必须原地不动");
        assert!(!auto_path.exists(), "声明了 auto 的项应已移走");
        println!("   ✓ 只清 auto 声明项，审计标记 source=auto，其余原地不动");
    }

    println!("\n全部通过。");
}
