//! 发请求。只做一件事：把摘要与提示词发出去，拿回一段文本。
//!
//! 两种协议（OpenAI 兼容 / Anthropic 原生）的差异只在这里处理：
//! - 路径与鉴权头不同；
//! - OpenAI 用 `messages` + 可选 `response_format`；
//! - Anthropic 把 system 单独放，`max_tokens` 必填。
//!
//! 不在这里做重试：分析失败时用户要的是「原因 + 重试按钮」，
//! 而不是一个转了半天最后仍然失败、还多花了 token 的按钮。

use std::time::Duration;

use crate::config::{ApiKind, Resolved};
use crate::digest::Digest;

/// 调用失败。每一种都要能直接显示给用户，并且说清「下一步该做什么」。
#[derive(Debug, Clone)]
pub enum AiError {
    /// 网络层没通（DNS / 代理 / 超时）
    Network(String),
    /// 上游返回了错误状态码。带上状态码与响应体开头——
    /// 「401」和「429」对用户的意义完全不同。
    Status { code: u16, body: String },
    /// 响应结构对不上（不是 OpenAI/Anthropic 的形态）
    Shape(String),
    /// 模型回了空内容
    Empty,
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Network(e) => write!(f, "连不上 AI 服务：{e}"),
            AiError::Status { code, body } => {
                let hint = match *code {
                    401 | 403 => "密钥无效或没有权限",
                    404 => "地址或模型不存在，检查 base_url 与模型名",
                    429 => "触发限流或余额不足，稍后再试",
                    500..=599 => "服务端出错，稍后重试",
                    _ => "请求被拒绝",
                };
                write!(f, "AI 服务返回 {code}（{hint}）：{body}")
            }
            AiError::Shape(e) => write!(f, "AI 返回的响应格式不符：{e}"),
            AiError::Empty => write!(f, "AI 返回了空内容"),
        }
    }
}

/// 一次分析请求。
pub struct Request<'a> {
    pub resolved: &'a Resolved,
    pub digest: &'a Digest,
    /// 超时。默认 120 秒：长 JSON 回复比对话慢得多。
    pub timeout: Duration,
}

const SYSTEM_PROMPT: &str = "\
你在为一个 macOS 磁盘清理工具做只读分析。你只能读取下面给出的元数据摘要，不能访问文件系统。

请严格按下面的 JSON 结构回复，不要输出 Markdown、不要加解释文字：

{
  \"summary\": \"两三句话说明这份磁盘内容的主要构成\",
  \"categories\": [{\"name\": \"分类名\", \"items\": [\"路径\"], \"bytes\": 0, \"note\": \"这一类的处理建议与代价\"}],
  \"duplicates\": [{\"paths\": [\"路径\"], \"reason\": \"为什么认为它们重复\"}],
  \"risks\": [{\"level\": \"high|medium|low\", \"path\": \"路径\", \"why\": \"风险说明\"}],
  \"proposals\": [{\"path\": \"路径\", \"action\": \"quarantine|compress|keep\", \"reason\": \"处理原因\", \"est_bytes\": 0}],
  \"explanations\": [{\"path\": \"路径\", \"purpose\": \"这一条是干什么用的\", \"safe_to_clean\": true, \"reason\": \"为什么能清 / 为什么不能清\"}]
}

规则：
- path 必须逐字来自摘要里的 items，不要编造、不要改写。
- **explanations 要为摘要里的每一条都写一条**（最多 120 条），界面会把这句话直接显示在那一行数据旁边。purpose 要具体到「删了会发生什么」，不要写「这是一个缓存目录」这种空话；safe_to_clean 与 risk level 不要互相矛盾。
- 标为 protected 的条目不要出现在 proposals 里；它们只能在 categories/risks 里被解释。
- action 只在你确信可重建、且删了不会丢用户数据时才用 quarantine；compress 用于「内容还有价值但很少用」。
- 不确定就用 keep，并在 reason 里说明为什么不确定。
- 不要建议删除系统目录、密钥/证书/配置、照片或文档的唯一副本。";

/// 把摘要渲染成提示词正文。
///
/// 用 JSON 而不是自然语言列表：模型对 JSON 的字段边界更敏感，
/// 也更容易保持 path 逐字不变。
fn user_prompt(digest: &Digest) -> String {
    let body = serde_json::to_string_pretty(digest).unwrap_or_default();
    format!(
        "以下是磁盘扫描摘要。磁盘总量 {total}，可用 {free}。共 {n} 条，其中 {omitted} 条因体积较小未列出。\n\n{body}",
        total = digest.disk_total,
        free = digest.disk_available,
        n = digest.items.len(),
        omitted = digest.omitted,
        body = body
    )
}

impl<'a> Request<'a> {
    pub fn new(resolved: &'a Resolved, digest: &'a Digest) -> Self {
        Self {
            resolved,
            digest,
            timeout: Duration::from_secs(120),
        }
    }

    /// 真的发出去。同步阻塞——调用方必须在后台线程里跑。
    pub fn send(&self) -> Result<String, AiError> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .build()
            .into();

        let prompt = user_prompt(self.digest);
        let (url, body) = match self.resolved.kind {
            ApiKind::OpenAiCompatible => {
                let url = format!("{}/chat/completions", self.resolved.base_url);
                let mut body = serde_json::json!({
                    "model": self.resolved.model,
                    "messages": [
                        {"role": "system", "content": SYSTEM_PROMPT},
                        {"role": "user", "content": prompt}
                    ],
                    "temperature": 0.2,
                    "max_tokens": 4096
                });
                // 只在模型声称支持时加：不支持 JSON 模式的模型收到它会直接 400
                if self.resolved.json_mode {
                    body["response_format"] = serde_json::json!({"type": "json_object"});
                }
                (url, body)
            }
            ApiKind::Anthropic => {
                let url = format!("{}/v1/messages", self.resolved.base_url);
                let body = serde_json::json!({
                    "model": self.resolved.model,
                    "system": SYSTEM_PROMPT,
                    "messages": [{"role": "user", "content": prompt}],
                    "max_tokens": 4096
                });
                (url, body)
            }
            ApiKind::SystemOne => {
                // System One 没有「聊天」这一步：它只回答带类型的问题。
                // 删除判断要的恰好是逐条动作，所以用 choice 问题问「这一项怎么处理」，
                // 再把 choice 映射回本地已有的 quarantine / compress / keep 动作。
                let url = format!("{}/v1/systemone", self.resolved.base_url);
                (url, system_one_body(&self.resolved.model, self.digest))
            }
        };

        let mut req = agent.post(&url).header("content-type", "application/json");
        req = match self.resolved.kind {
            ApiKind::OpenAiCompatible | ApiKind::SystemOne => {
                req.header("authorization", &format!("Bearer {}", self.resolved.key()))
            }
            ApiKind::Anthropic => req
                .header("x-api-key", self.resolved.key())
                .header("anthropic-version", "2023-06-01"),
        };

        let resp = req.send_json(body).map_err(|e| match e {
            ureq::Error::StatusCode(code) => AiError::Status {
                code,
                body: String::new(),
            },
            other => AiError::Network(other.to_string()),
        });

        // 4xx/5xx 需要读响应体：OpenAI 与 Anthropic 的错误说明都在里面，
        // 只报状态码会让「模型名写错了」看起来像「密钥错了」。
        let text = match resp {
            Ok(mut r) => r
                .body_mut()
                .read_to_string()
                .map_err(|e| AiError::Network(e.to_string()))?,
            Err(AiError::Status { code, .. }) => {
                return Err(AiError::Status {
                    code,
                    body: "（响应体未能读取）".to_string(),
                });
            }
            Err(e) => return Err(e),
        };

        extract_text(self.resolved.kind, &text, self.digest)
    }
}

/// 从响应 JSON 里取出文本。三种协议的字段位置不同。
fn extract_text(kind: ApiKind, body: &str, digest: &Digest) -> Result<String, AiError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| AiError::Shape(format!("不是 JSON：{e}")))?;

    let text = match kind {
        ApiKind::OpenAiCompatible => v
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string()),
        ApiKind::Anthropic => v
            .pointer("/content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            }),
        ApiKind::SystemOne => {
            return system_one_report(&v, digest).map_err(|e| AiError::Shape(e.to_string()));
        }
    };

    match text {
        Some(t) if !t.trim().is_empty() => Ok(t),
        Some(_) => Err(AiError::Empty),
        None => {
            // 上游常把错误放在 200 响应里（某些中转会这样）。
            // 把 body 的前一段带出来，比只说「格式不符」有用得多。
            let hint: String = body.chars().take(300).collect();
            Err(AiError::Shape(format!("找不到回复文本：{hint}")))
        }
    }
}

/// 把摘要变成 System One 的请求体。
///
/// 只问「这一项该不该动、怎么动」，因为删除判断最后要落到本地执行器认识的
/// 三个动作上。`keep` 也要作为选项给出：没有这个出口，模型只能在
/// quarantine / compress 里二选一，等于逼它对每一条都动手。
fn system_one_body(model: &str, digest: &Digest) -> serde_json::Value {
    // 32k 窗口要同时装下 state 与每个问题的 criteria。按摘要上限 120 条全发会超，
    // 所以只问最大的这一批；其余条目保持不动，并在 omitted 里如实报出来。
    const MAX_SYSTEM_ONE_ITEMS: usize = 40;
    let items = &digest.items[..digest.items.len().min(MAX_SYSTEM_ONE_ITEMS)];
    let omitted = digest.omitted + (digest.items.len() - items.len());

    let mut questions = serde_json::Map::new();
    // 逐条问：问题名就是条目的下标，回答按同一个名字回来。
    // 元数据只在 state 里发一次，问题里只写下标——两边都带一份会让 40 项就吃掉半个窗口。
    for (i, _) in items.iter().enumerate() {
        questions.insert(
            format!("item_{i}"),
            serde_json::json!({
                "type": "choice",
                "instructions": format!(
                    "state.items[{i}] 是磁盘上的一项。对备份/清理工具来说，这一项该怎么处理？\
                     只看 state 里的元数据，不要假设文件内容。"
                ),
                "criteria": {
                    "quarantine": "可重建的缓存或构建产物，删了会自动重建，可以移入隔离区",
                    "compress": "内容还有价值但很少使用，压缩归档更合适",
                    "keep": "可能含用户数据、无法确认可重建，或本地已判为受保护，保持不动"
                }
            }),
        );
    }

    // 零候选时也必须至少有一个问题：System One 要求 questions 非空。
    // 这个回答不参与方案（`system_one_report` 只认 item_ 前缀），只是让请求合法。
    if questions.is_empty() {
        questions.insert(
            "summary".into(),
            serde_json::json!({
                "type": "choice",
                "instructions": "这次扫描没有可判断的条目。是否还有需要人看一眼的风险？",
                "criteria": {
                    "nothing_to_do": "没有需要处理的项",
                    "review_needed": "有风险但无法从元数据判断，需要人工确认"
                }
            }),
        );
    }

    serde_json::json!({
        "model": model,
        "state": {
            "task": "判断这些磁盘项目是否可以安全清理",
            "home": digest.home,
            "disk_total": digest.disk_total,
            "disk_available": digest.disk_available,
            "omitted": omitted,
            "items": items,
        },
        "questions": questions,
    })
}

/// 把 System One 的 `answers` 还原成 `AiReport` 的 JSON 文本。
///
/// 走 JSON 而不是直接构造 `AiReport`：`extract_text` 的调用方统一按
/// 「拿到一段文本再解析」处理，这里保持同一个形状，后续解析与拒绝逻辑不用分叉。
fn system_one_report(v: &serde_json::Value, digest: &Digest) -> Result<String, AiError> {
    let answers = v
        .get("answers")
        .and_then(|a| a.as_object())
        .ok_or_else(|| AiError::Shape("System One 响应里没有 answers".to_string()))?;

    let mut proposals = Vec::new();
    let mut explanations = Vec::new();
    let mut kept = 0usize;
    for (name, answer) in answers {
        // 问题名是 item_{i}；拿不到下标就跳过——这类条目本来也无法对应回扫描结果。
        let Some(idx) = name
            .strip_prefix("item_")
            .and_then(|s| s.parse::<usize>().ok())
        else {
            continue;
        };
        // 模型只回 choice，不回路径。路径必须由本地按下标取回，
        // 否则 `Plan::build` 会把它当成幻觉路径拒掉。
        let Some(item) = digest.items.get(idx) else {
            continue;
        };
        let choice = answer
            .get("choice")
            .and_then(|c| c.as_str())
            .unwrap_or("keep");
        let action = match choice {
            "quarantine" => "quarantine",
            "compress" => "compress",
            _ => {
                kept += 1;
                "keep"
            }
        };
        let confidence = answer
            .get("confidence")
            .and_then(|c| c.as_f64())
            .unwrap_or(1.0);
        proposals.push(serde_json::json!({
            "path": item.path,
            "action": action,
            "reason": format!("System One 判断为 {choice}（置信度 {confidence:.2}）"),
        }));

        // System One 只给一个有类型的答案，没有自由文本。界面需要的「这一条是干什么的」
        // 就从答案本身翻过来：用户看到的是一句可读的判断，而不是一个内部分类名。
        let (purpose, reason) = match choice {
            "quarantine" => (
                "可重建的缓存或构建产物，删除后会自动重新生成",
                "判定层确认它不属于用户数据，可以移入隔离区",
            ),
            "compress" => (
                "内容仍有价值但很少使用",
                "压缩归档更合适：原件腾空，需要时还能解回来",
            ),
            _ => (
                "可能含用户数据，或无法确认能重建",
                "本地判定为不宜自动处理，保持不动",
            ),
        };
        explanations.push(serde_json::json!({
            "path": item.path,
            "purpose": purpose,
            "safe_to_clean": choice != "keep",
            "reason": format!("{reason}（置信度 {confidence:.2}）"),
        }));
    }

    let report = serde_json::json!({
        "summary": format!("共 {} 条判断，其中 {} 条建议保持不动", answers.len(), kept),
        "proposals": proposals,
        "explanations": explanations,
    });
    Ok(report.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_core::{Candidate, ItemKind, Verdict};

    fn digest_of(paths: &[&str]) -> Digest {
        let items: Vec<Candidate> = paths
            .iter()
            .map(|p| {
                Candidate::new(*p, ItemKind::Dir, Verdict::Reclaimable).with_size(1024, 1024, 1)
            })
            .collect();
        Digest::build("/Users/me", &items, None, None)
    }

    #[test]
    fn extracts_openai_shape() {
        let body = r#"{"choices":[{"message":{"content":"hello"}}]}"#;
        assert_eq!(
            extract_text(ApiKind::OpenAiCompatible, body, &digest_of(&[])).unwrap(),
            "hello"
        );
    }

    #[test]
    fn extracts_anthropic_shape() {
        let body = r#"{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}"#;
        assert_eq!(
            extract_text(ApiKind::Anthropic, body, &digest_of(&[])).unwrap(),
            "ab"
        );
    }

    #[test]
    fn empty_content_is_an_error() {
        let body = r#"{"choices":[{"message":{"content":""}}]}"#;
        assert!(matches!(
            extract_text(ApiKind::OpenAiCompatible, body, &digest_of(&[])),
            Err(AiError::Empty)
        ));
    }

    #[test]
    fn unexpected_shape_carries_a_hint() {
        let body = r#"{"error":{"message":"invalid model"}}"#;
        let e = extract_text(ApiKind::OpenAiCompatible, body, &digest_of(&[])).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("invalid model"), "要把上游原话带出来：{msg}");
    }

    #[test]
    fn system_one_asks_one_choice_question_per_item() {
        let digest = digest_of(&["/Users/me/a", "/Users/me/b"]);
        let body = system_one_body("jev", &digest);
        let questions = body["questions"].as_object().expect("应有 questions");
        assert_eq!(questions.len(), 2, "每条摘要对应一个问题");
        assert_eq!(body["model"], "jev", "模型名必须原样来自配置");
        for (name, q) in questions {
            assert!(name.starts_with("item_"), "问题名要能映射回下标：{name}");
            assert_eq!(q["type"], "choice");
            for option in ["quarantine", "compress", "keep"] {
                assert!(q["criteria"].get(option).is_some(), "缺少出口 {option}");
            }
        }
    }

    #[test]
    fn system_one_maps_answers_back_to_real_paths() {
        let digest = digest_of(&["/Users/me/cache", "/Users/me/docs"]);
        let body = serde_json::json!({
            "model": "jev",
            "answers": {
                "item_0": {"type": "choice", "choice": "quarantine", "confidence": 0.91},
                "item_1": {"type": "choice", "choice": "keep", "confidence": 0.77}
            },
            "usage": {"input_tokens": 10, "output_tokens": 2}
        });
        let text = extract_text(ApiKind::SystemOne, &body.to_string(), &digest).unwrap();
        let report = crate::report::AiReport::parse(&text).unwrap();
        assert_eq!(report.proposals.len(), 2, "keep 也要保留，不能静默消失");
        assert_eq!(report.proposals[0].path, "~/cache", "路径要从摘要取回");
        assert_eq!(
            report.proposals[0].action,
            Some(crate::report::ProposalAction::Quarantine)
        );
        assert_eq!(report.proposals[1].path, "~/docs");
        assert_eq!(
            report.proposals[1].action,
            Some(crate::report::ProposalAction::Keep)
        );
    }

    #[test]
    fn system_one_without_answers_is_a_shape_error() {
        let body = r#"{"model":"jev"}"#;
        let e = extract_text(ApiKind::SystemOne, body, &digest_of(&[])).unwrap_err();
        assert!(e.to_string().contains("answers"), "要说清缺了什么：{e}");
    }

    #[test]
    fn system_one_with_no_items_still_sends_a_valid_question() {
        let digest = digest_of(&[]);
        let body = system_one_body("jev", &digest);
        let questions = body["questions"].as_object().expect("应有 questions");
        assert!(
            !questions.is_empty(),
            "System One 要求 questions 非空，零候选时也要有合法请求"
        );
        assert!(
            !questions.contains_key("item_0"),
            "占位问题不能用 item_ 前缀，否则会被当成第 0 条"
        );
    }

    #[test]
    fn system_one_does_not_send_metadata_twice() {
        let digest = digest_of(&["/Users/me/cache"]);
        let body = system_one_body("jev", &digest);
        let q = &body["questions"]["item_0"];
        assert!(
            !q["instructions"]
                .as_str()
                .unwrap_or_default()
                .contains("/Users/me/cache"),
            "路径只在 state 里发一次，问题里不必重复"
        );
        assert_eq!(body["state"]["items"][0]["path"], "~/cache");
    }

    #[test]
    fn status_messages_tell_the_user_what_to_do() {
        let e = AiError::Status {
            code: 401,
            body: "unauthorized".into(),
        };
        assert!(e.to_string().contains("密钥"));
        let e = AiError::Status {
            code: 404,
            body: String::new(),
        };
        assert!(e.to_string().contains("模型"));
    }
}
