//! 用本地假服务端验证「配置里的模型确实生效」。
//!
//! 为什么需要它：这一条是需求里的硬检查点——如果代码某处写死了模型，
//! 界面照样能跑、单元测试照样能过，只有真的发一次请求才看得出来。
//!
//! 做法：起一个只实现 `/chat/completions` 的本机 HTTP 服务，把收到的
//! `model` 字段记下来并按 OpenAI 的形状回一段 JSON；然后用两份不同的配置
//! 各调用一次，断言服务端收到的模型名就是配置里选的那个。
//! 全程不离线、不花钱，也不碰真实密钥。
//!
//! 用法：`cargo run -p molt-ai --example model_roundtrip`

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use molt_ai::{AiConfig, Digest, Model};

const REPLY: &str = r#"{"summary":"ok","proposals":[{"path":"~/a","action":"keep","reason":"x"}]}"#;

fn main() {
    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let (port, handle) = spawn_server(received.clone());

    for model_id in ["model-alpha", "model-beta"] {
        let mut cfg = AiConfig::default();
        cfg.providers.push(molt_ai::Provider {
            id: "local".into(),
            label: "本地假服务端".into(),
            kind: molt_ai::ApiKind::OpenAiCompatible,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            api_key_env: String::new(),
            api_key: "test-key".into(),
            official: false,
            models: vec![Model {
                id: model_id.into(),
                label: model_id.into(),
                context_tokens: 32_000,
                json_mode: true,
            }],
        });
        cfg.set_provider("local");

        let resolved = cfg.resolve().expect("配置应可解析");
        assert_eq!(resolved.model, model_id, "解析出的模型应来自配置");

        let digest = Digest::build("/Users/me", &[], None, None);
        let text = molt_ai::client::Request::new(&resolved, &digest)
            .send()
            .expect("请求应成功");
        assert!(text.contains("ok"), "应拿到回复：{text}");
        println!("配置选择 {model_id} → 服务端收到请求，回复解析成功");
    }

    let seen = received.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec!["model-alpha".to_string(), "model-beta".to_string()],
        "服务端收到的模型必须逐个对应配置里的选择，说明没有被硬编码"
    );
    println!("服务端实际收到：{seen:?}");

    drop(handle);
    println!("模型来自配置，且真实生效。");
}

/// 起一个最小的 HTTP/1.1 服务，读一条请求、记下 model、回 OpenAI 形状的 JSON。
fn spawn_server(received: Arc<Mutex<Vec<String>>>) -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口");
    let port = listener.local_addr().unwrap().port();

    let handle = std::thread::spawn(move || {
        // 只处理两次请求（对应上面两个模型），然后退出
        for _ in 0..2 {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());

            // 先读头，拿到 Content-Length
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Some(v) = line.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                } else if let Some(v) = line.strip_prefix("Content-Length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }

            let mut body = vec![0u8; len];
            use std::io::Read;
            reader.read_exact(&mut body).ok();
            let body = String::from_utf8_lossy(&body).to_string();

            // 记录模型名：这就是「配置是否真的生效」的证据
            let model = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_else(|| "<缺失>".into());
            received.lock().unwrap().push(model);

            let payload = serde_json::json!({
                "choices": [{"message": {"content": REPLY}}]
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
        }
    });

    (port, handle)
}
