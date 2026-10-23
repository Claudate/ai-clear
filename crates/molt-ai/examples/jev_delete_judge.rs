//! 删除判断走 jev 的端到端验证。
//!
//! 验的是三件事：
//! 1. 默认配置（用户什么都没选）下，删除判断解析出的模型是 `jev`，服务是 `typesafe`；
//! 2. 请求真的发到了 `/v1/systemone`，且服务端收到的 `model` 字段就是 `jev`；
//! 3. 服务端按 System One 形状回答案后，能还原成方案，路径仍来自本次扫描结果。
//!
//! 用本机假服务端而不是真端点：验收要能重复跑、不花钱，也不依赖谁的密钥。
//! 请求形状与鉴权头都按真实协议发，所以「协议接错了」这件事仍然会被抓到。
//!
//! 用法：`cargo run -p molt-ai --example jev_delete_judge`

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use molt_ai::{AiConfig, AiReport, ApiKind, Digest, Plan, PlanRoots};

fn main() {
    // ---- 1. 默认配置必须解析到 jev ----
    let cfg = AiConfig::default();
    assert!(
        cfg.active_provider.is_empty() && cfg.active_model.is_empty(),
        "前提：默认模板不预设选择"
    );

    // 密钥从环境变量来，这里只在进程内给一个假值，避免依赖本机环境。
    std::env::set_var("TYPESAFE_API_KEY", "test-key-not-real");

    let resolved = cfg
        .resolve_for_delete_judge()
        .expect("默认配置应能解析出删除判断用的模型");
    assert_eq!(resolved.provider_id, "typesafe", "应使用 jev 所在的服务");
    assert_eq!(resolved.model, "jev", "模型名必须就是 jev");
    assert_eq!(resolved.kind, ApiKind::SystemOne, "jev 走 System One 协议");
    assert_eq!(resolved.base_url, "https://api.typesafe.ai");
    println!(
        "① 解析：{} · {}（协议 {}）",
        resolved.provider_label,
        resolved.model,
        resolved.kind.label()
    );

    // 用户自己选了别的模型时，不能被这条默认值改写。
    let mut chosen = AiConfig::default();
    chosen.set_provider("deepseek");
    let chosen_resolved = chosen.resolve_for_delete_judge();
    match chosen_resolved {
        Err(molt_ai::ConfigError::NoKey { .. }) => {
            println!("② 尊重用户选择：选了 deepseek 后没有被改回 jev（只是缺密钥）");
        }
        other => panic!("用户已选服务时不应回落到 jev，实际 {other:?}"),
    }

    // ---- 2. 起假服务端，按真实协议收请求 ----
    let seen: Arc<Mutex<Vec<SeenRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let (port, server) = spawn_server(seen.clone());

    let home = std::env::temp_dir().join(format!("molt-jev-{}", std::process::id()));
    std::fs::create_dir_all(home.join("cache")).unwrap();
    let items = vec![molt_core::Candidate::new(
        home.join("cache"),
        molt_core::ItemKind::Dir,
        molt_core::Verdict::Reclaimable,
    )
    .with_size(20_000, 20_000, 3)];
    let mut digest = Digest::build(home.to_str().unwrap(), &items, None, None);
    digest.fit_to(resolved.context_tokens);

    // 指向假服务端：只改 base_url，其余（协议、模型名、鉴权方式）保持与线上一致。
    let mut local = cfg.clone();
    if let Some(p) = local.providers.iter_mut().find(|p| p.id == "typesafe") {
        p.base_url = format!("http://127.0.0.1:{port}");
    }
    let resolved = local
        .resolve_for_delete_judge()
        .expect("本地端点也应可解析");
    assert_eq!(resolved.model, "jev");

    let reply = molt_ai::client::Request::new(&resolved, &digest)
        .send()
        .expect("删除判断请求应成功");
    drop(server);

    // ---- 3. 断言实际发出去的东西 ----
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "应只发一次请求");
    let req = &got[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/systemone", "jev 的路径必须是 System One");
    assert_eq!(
        req.authorization.as_deref(),
        Some("Bearer test-key-not-real"),
        "鉴权头应按 Bearer 发送"
    );
    assert_eq!(req.model, "jev", "服务端实际收到的模型名必须是 jev");
    assert!(
        req.question_count >= 1,
        "至少要为一条摘要发一个带类型的问题"
    );
    println!(
        "③ 请求：{} {}，model={}，{} 个问题",
        req.method, req.path, req.model, req.question_count
    );

    // ---- 4. 回答能还原成方案 ----
    let report = AiReport::parse(&reply).expect("System One 回答应能解析");
    let roots = PlanRoots {
        quarantine_root: home.join("quarantine"),
        archive_root: home.join("archive"),
        batch: "batch-1".into(),
    };
    let plan = Plan::build(
        &report,
        &items,
        home.to_str().unwrap(),
        &roots,
        &resolved.model,
    );
    assert_eq!(plan.items.len(), 1, "可回收项应进入方案");
    assert_eq!(plan.model, "jev", "方案要标明出自 jev");
    assert!(
        plan.items[0].source_display.contains("cache"),
        "路径要来自本次扫描结果：{:?}",
        plan.items[0].source_display
    );
    println!(
        "④ 方案：{} → {}（模型 {}）",
        plan.items[0].source_display,
        plan.items[0].action.label(),
        plan.model
    );

    std::fs::remove_dir_all(&home).ok();
    println!("\n删除判断走通，且实际调用的是 jev。");
}

#[derive(Debug, Clone)]
struct SeenRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    model: String,
    question_count: usize,
}

/// 只处理一次请求的最小 HTTP 服务端，按 System One 的形状回答案。
fn spawn_server(seen: Arc<Mutex<Vec<SeenRequest>>>) -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());

        let mut request_line = String::new();
        reader.read_line(&mut request_line).ok();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();

        let mut len = 0usize;
        let mut authorization = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let lower = line.to_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
            if lower.strip_prefix("authorization:").is_some() {
                authorization = Some(
                    line.split_once(':')
                        .map(|(_, v)| v.trim().to_string())
                        .unwrap_or_default(),
                );
            }
            if line == "\r\n" {
                break;
            }
        }

        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).ok();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let model = body["model"].as_str().unwrap_or("<缺失>").to_string();
        let questions = body["questions"].as_object().cloned().unwrap_or_default();

        // 第一条判为可清理，其余保持不动；形状与官方 answers 一致。
        let mut answers = serde_json::Map::new();
        for (i, name) in questions.keys().enumerate() {
            answers.insert(
                name.clone(),
                serde_json::json!({
                    "type": "choice",
                    "choice": if i == 0 { "quarantine" } else { "keep" },
                    "confidence": 0.9,
                    "probabilities": {"quarantine": 0.9, "compress": 0.05, "keep": 0.05}
                }),
            );
        }

        seen.lock().unwrap().push(SeenRequest {
            method,
            path,
            authorization,
            model,
            question_count: questions.len(),
        });

        let payload = serde_json::json!({
            "model": "jev",
            "answers": answers,
            "usage": {"input_tokens": 100, "output_tokens": 10}
        })
        .to_string();
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
