//! 七段流程端到端：扫描 → 分析 → 方案 → 预览 → 执行 → 撤销 → 复查清理。
//!
//! 全程在临时目录里跑，不碰用户家目录——删除/移动类流程的验收必须在假环境里做，
//! 这条是项目里的硬规矩（真机上误点过一次，把 218 项移进了隔离区）。
//!
//! 其中「分析」那一步用一个本机假服务端替身：这里要验的是**流程本身**
//! （摘要 → 请求 → 解析 → 方案 → 权限核对），不是模型的质量。
//!
//! 用法：`cargo run -p molt-ai --example pipeline`

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use molt_ai::{AiConfig, AiReport, ContentPolicy, Digest, Model, Plan, PlanRoots};

fn main() {
    let base = tmpdir();
    println!("工作目录：{}", base.display());

    // ---- 第 1 步：扫描（这里直接用构造好的候选代表扫描结果）----
    // 真实扫描由 molt-scan 负责、已有单测；这一步要验的是后面的链路。
    let cache = base.join("cache");
    let logs = base.join("logs");
    let keep = base.join("keeper");
    for d in [&cache, &logs, &keep] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(cache.join("a.bin"), vec![7u8; 20_000]).unwrap();
    std::fs::write(logs.join("run.log"), "line\n".repeat(4000)).unwrap();
    std::fs::write(keep.join("doc.txt"), b"precious").unwrap();

    let scan = vec![
        molt_core::Candidate::new(
            &cache,
            molt_core::ItemKind::Dir,
            molt_core::Verdict::Reclaimable,
        )
        .with_size(20_000, 20_000, 1)
        .with_rule("cache-dir"),
        molt_core::Candidate::new(
            &logs,
            molt_core::ItemKind::Dir,
            molt_core::Verdict::Reclaimable,
        )
        .with_size(20_000, 20_000, 1)
        .with_rule("log-dir"),
        molt_core::Candidate::new(
            &keep,
            molt_core::ItemKind::Dir,
            molt_core::Verdict::Protected {
                reason: molt_core::ProtectReason::UserRecord,
            },
        )
        .with_size(7, 7, 1),
    ];
    println!("① 扫描：{} 项（其中 1 项受保护）", scan.len());

    // ---- 第 2 步：分析（摘要只发元数据）----
    let digest = Digest::build(base.to_str().unwrap(), &scan, None, None);
    assert!(!digest.sampled, "默认不读文件内容");
    assert!(digest.items.iter().all(|i| i.sample.is_none()));
    println!(
        "② 分析：摘要 {} 条，约 {} token，内容采样 = {}",
        digest.items.len(),
        digest.est_tokens,
        digest.sampled
    );

    // 内容采样打开时，带密钥的文件必须被拒
    std::fs::write(cache.join("secret.txt"), "api_key = sk-live-abc123\n").unwrap();
    let d2 = Digest::build(
        base.to_str().unwrap(),
        &scan,
        None,
        Some(ContentPolicy::default()),
    );
    let leaked = d2.items.iter().any(|i| {
        i.sample
            .as_deref()
            .map(|s| s.contains("sk-live"))
            .unwrap_or(false)
    });
    assert!(!leaked, "密钥不能被采样上传");
    println!("②' 内容采样开启时仍拒绝上传疑似密钥");

    // ---- 接到假服务端，拿回结构化结果 ----
    let (port, server) = spawn_server();
    let mut cfg = AiConfig::default();
    cfg.providers.push(molt_ai::Provider {
        id: "local".into(),
        label: "本地假服务端".into(),
        kind: molt_ai::ApiKind::OpenAiCompatible,
        base_url: format!("http://127.0.0.1:{port}/v1"),
        api_key_env: String::new(),
        api_key: "test".into(),
        official: false,
        models: vec![Model {
            id: "pipeline-model".into(),
            label: "pipeline-model".into(),
            context_tokens: 32_000,
            json_mode: true,
        }],
    });
    cfg.set_provider("local");
    let resolved = cfg.resolve().unwrap();
    let reply = molt_ai::client::Request::new(&resolved, &digest)
        .send()
        .expect("请求应成功");
    let report = AiReport::parse(&reply).expect("应解析成结构化结果");
    drop(server);
    println!(
        "②'' 模型 {} 返回 {} 条建议",
        resolved.model,
        report.proposals.len()
    );

    // ---- 第 3 步：生成方案（逐条核对）----
    let roots = PlanRoots {
        quarantine_root: base.join("quarantine"),
        archive_root: base.join("archive"),
        batch: "batch-1".into(),
    };
    let plan = Plan::build(
        &report,
        &scan,
        base.to_str().unwrap(),
        &roots,
        &resolved.model,
    );

    assert_eq!(plan.items.len(), 2, "两项可回收应进方案");
    assert!(
        plan.rejected.iter().any(|r| r.reason.contains("受保护")),
        "受保护项要被拒并说明原因"
    );
    assert!(
        plan.items.iter().all(|i| i.target.starts_with(&base)),
        "目标路径必须在本地算出的落点下"
    );
    assert_eq!(plan.checked_count(), 2, "可回收项默认勾选");
    println!(
        "③ 方案：{} 项待处理（{}），{} 项被拒",
        plan.items.len(),
        plan.items
            .iter()
            .map(|i| format!("{} → {}", i.source_display, i.action.label()))
            .collect::<Vec<_>>()
            .join("；"),
        plan.rejected.len()
    );

    // ---- 第 4 步：预览确认（未确认前不动盘）----
    assert!(cache.exists() && logs.exists(), "生成方案不能改动文件");
    println!("④ 预览：确认前文件仍在原位");

    // ---- 第 5 步：执行（移入隔离区 / 压缩归档）----
    let store = molt_act::AuditStore::open(&base.join("audit.sqlite")).unwrap();
    let exec = molt_act::Executor::new(
        store,
        molt_act::Quarantine::new(base.join("quarantine"), "b"),
        75,
    );

    let quarantine_items: Vec<_> = plan
        .items
        .iter()
        .filter(|i| matches!(i.action, molt_ai::PlanAction::Quarantine))
        .map(|i| {
            molt_core::Candidate::new(
                &i.source,
                molt_core::ItemKind::Dir,
                molt_core::Verdict::Reclaimable,
            )
            .with_size(i.bytes, i.bytes, i.files)
        })
        .collect();
    let conf = molt_act::Confirmation::new(&quarantine_items);
    let out = exec.clean(&quarantine_items, &conf);
    assert_eq!(out.moved.len(), 1, "失败：{:?}", out.failed);
    let moved_id = out.moved[0].id;

    let compress_items: Vec<_> = plan
        .items
        .iter()
        .filter(|i| matches!(i.action, molt_ai::PlanAction::Compress))
        .map(|i| {
            molt_core::Candidate::new(
                &i.source,
                molt_core::ItemKind::Dir,
                molt_core::Verdict::Reclaimable,
            )
            .with_size(i.bytes, i.bytes, i.files)
        })
        .collect();
    let mut archive_id = None;
    if !compress_items.is_empty() {
        let conf = molt_act::Confirmation::new(&compress_items);
        let out = exec.compress(&compress_items, &conf);
        assert_eq!(out.moved.len(), 1, "压缩失败：{:?}", out.failed);
        archive_id = Some(out.moved[0].id);
    }
    assert!(!cache.exists());
    assert!(!logs.exists());
    println!(
        "⑤ 执行：移入隔离区 {} 项，压缩归档 {} 项",
        1,
        archive_id.iter().count()
    );

    // ---- 第 6 步：撤销（回到原位置）----
    exec.restore(moved_id).unwrap();
    assert_eq!(std::fs::read(cache.join("a.bin")).unwrap().len(), 20_000);
    if let Some(id) = archive_id {
        exec.restore(id).unwrap();
        assert!(logs.join("run.log").exists(), "解包后原文件应回来");
    }
    println!("⑥ 撤销：两项都回到原位置，内容一致");

    // 再执行一次，准备第 7 步
    let conf = molt_act::Confirmation::new(&quarantine_items);
    let out = exec.clean(&quarantine_items, &conf);
    let pending_id = out.moved[0].id;

    // ---- 第 7 步：复查后彻底清理 ----
    let pending = exec.store().list_pending(10).unwrap();
    assert!(!pending.is_empty());
    let purge = exec.purge_ids(&[pending_id]);
    assert_eq!(purge.deleted.len(), 1, "失败：{:?}", purge.failed);
    assert!(!cache.exists(), "彻底清理后内容不该再在隔离区");
    println!(
        "⑦ 复查：确认后彻底清理 {} 项，空间释放",
        purge.deleted.len()
    );

    std::fs::remove_dir_all(&base).ok();
    println!("\n七段全部通过。");
}

fn tmpdir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "molt-pipeline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// 假服务端：回一份结构化结果，其中故意包含一条受保护项的建议，
/// 用来验证方案层会把它拒掉。
fn spawn_server() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let lower = line.to_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                let _ = v;
                len = line
                    .split(':')
                    .nth(1)
                    .unwrap_or("0")
                    .trim()
                    .parse()
                    .unwrap_or(0);
            }
            if line == "\r\n" {
                break;
            }
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).ok();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let user = body["messages"][1]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let home = home_dir_from_digest(&user);

        // 两条可动 + 一条受保护（应被拒）
        let content = format!(
            r#"{{"summary":"测试方案",
"categories":[{{"name":"缓存","items":[],"bytes":40000,"note":"可重建"}}],
"duplicates":[],
"risks":[{{"level":"low","path":"/x","why":"无"}}],
"proposals":[
 {{"path":"{home}/cache","action":"quarantine","reason":"缓存可重建","est_bytes":20000}},
 {{"path":"{home}/logs","action":"compress","reason":"日志少见但可压缩","est_bytes":20000}},
 {{"path":"{home}/keeper","action":"quarantine","reason":"占空间","est_bytes":7}}
]}}"#
        );
        let payload = serde_json::json!({"choices":[{"message":{"content":content}}]}).to_string();
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );
        let mut out = stream;
        out.write_all(resp.as_bytes()).ok();
        out.flush().ok();
    });
    (port, handle)
}

/// 从提示词里取出 home：摘要 JSON 里有 `"home"` 字段。
fn home_dir_from_digest(prompt: &str) -> String {
    let start = prompt.find('{').unwrap_or(0);
    serde_json::from_str::<serde_json::Value>(&prompt[start..])
        .ok()
        .and_then(|v| v.get("home").and_then(|h| h.as_str()).map(String::from))
        .unwrap_or_else(|| "/tmp".to_string())
}

#[allow(dead_code)]
fn assert_exists(p: &Path) {
    assert!(p.exists(), "{} 应当存在", p.display());
}
