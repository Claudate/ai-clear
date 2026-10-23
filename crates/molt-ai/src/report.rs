//! 结构化分析结果：AI 必须按这组类型回话，而不是自由文本。
//!
//! 为什么要强类型而不是解析一段 Markdown：界面要把「分类 / 重复 / 风险 / 建议」
//! 分别画成不同的块，逐条给复选框。纯文本只能整段显示，用户没法逐项确认，
//! 也没法把某一项交给后面的执行器——那会让「预览确认」退化成「让你自己读」。
//!
//! 解析是**容错**的：模型常把 JSON 包在 ```json 里、或在前后加一句解释。
//! 这里会剥掉围栏再解析；仍然失败时把原文带回界面，让用户至少能看到它说了什么，
//! 而不是只显示「解析失败」。

use serde::{Deserialize, Serialize};

/// 一类内容。AI 自己起名字，我们不预设分类表——
/// 预设分类会让它把「Rust target」硬塞进「缓存」，而这两者的处理方式并不一样。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Category {
    #[serde(default)]
    pub name: String,
    /// 归到这一类的路径
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub bytes: u64,
    /// 这一类的处理建议与代价
    #[serde(default)]
    pub note: String,
}

/// 一组疑似重复的内容。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DuplicateGroup {
    #[serde(default)]
    pub paths: Vec<String>,
    /// 为什么认为它们重复（同名 / 同大小 / 同用途）
    #[serde(default)]
    pub reason: String,
}

/// 风险等级。只有三级：高 = 可能丢数据，中 = 会打断工作流，低 = 只是浪费空间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    High,
    Medium,
    #[default]
    Low,
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            RiskLevel::High => "高",
            RiskLevel::Medium => "中",
            RiskLevel::Low => "低",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Risk {
    #[serde(default)]
    pub level: RiskLevel,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub why: String,
}

/// AI 建议的一个动作。**它只是建议**——能不能做由 `plan` 层重新核对。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAction {
    /// 移入隔离区（可还原）
    Quarantine,
    /// 压缩归档（可还原）
    Compress,
    /// 保持不动，只是解释
    Keep,
}

impl ProposalAction {
    pub fn label(self) -> &'static str {
        match self {
            ProposalAction::Quarantine => "移入隔离区",
            ProposalAction::Compress => "压缩归档",
            ProposalAction::Keep => "保持不动",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Proposal {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub action: Option<ProposalAction>,
    #[serde(default)]
    pub reason: String,
    /// AI 估计能释放多少。这个数字只用于展示，**执行时的数字一律按本地实测重算**。
    #[serde(default)]
    pub est_bytes: u64,
}

/// AI 对单条扫描结果的一句话解释。
///
/// 界面要把这段文字直接显示在对应的条目行上，所以它必须短、且必须绑定路径：
/// 一段没有路径的解释无法挂到任何一条数据上，对用户来说等于没给。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Explanation {
    /// 与摘要里逐字一致的路径（`~/...` 形式）
    #[serde(default)]
    pub path: String,
    /// 这条东西是干什么用的
    #[serde(default)]
    pub purpose: String,
    /// AI 判断这一条能不能动。`None` 表示它没表态，界面按本地判定显示。
    #[serde(default)]
    pub safe_to_clean: Option<bool>,
    /// 为什么可以 / 不可以动；可以为空
    #[serde(default)]
    pub reason: String,
}

impl Explanation {
    /// 拼成界面行上那一句。
    ///
    /// 用途与理由分开写、中间用「·」连接：用户扫一眼先看到「这是什么」，
    /// 需要更多信息才读后半句。两者都空时返回 `None`，界面据此不画空行。
    pub fn line(&self) -> Option<String> {
        let purpose = self.purpose.trim();
        let reason = self.reason.trim();
        let parts: Vec<&str> = [purpose, reason].into_iter().filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }
}

/// AI 的完整分析结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiReport {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub duplicates: Vec<DuplicateGroup>,
    #[serde(default)]
    pub risks: Vec<Risk>,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    /// 逐条解释。允许为空——模型可能只给方案不给解释，
    /// 界面要把「没有解释」与「解释是空的」区分成同一件事，都不画。
    #[serde(default)]
    pub explanations: Vec<Explanation>,
}

impl AiReport {
    /// 按路径查解释。路径可能与模型输出的大小写或末尾斜杠不同，
    /// 所以比较前统一收口，避免「同一条却对不上」这种沉默失败。
    pub fn explanation_for(&self, path: &str) -> Option<&Explanation> {
        let want = norm_path(path);
        self.explanations
            .iter()
            .find(|e| norm_path(&e.path) == want)
    }
}

fn norm_path(p: &str) -> String {
    let t = p.trim().trim_end_matches('/');
    // Windows 风格分隔符不该出现在这里，但模型偶尔会给；统一成 `/` 更稳
    t.replace('\\', "/").to_lowercase()
}

/// 解析失败的原因 + 原始回复。原始回复要留给界面展示：
/// 用户至少能看到模型到底说了什么，而不是一个空白的错误框。
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub raw: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl AiReport {
    /// 从模型回复里解析。剥离 Markdown 围栏与前后废话。
    pub fn parse(raw: &str) -> Result<Self, ParseError> {
        let json = extract_json(raw).ok_or_else(|| ParseError {
            message: "模型没有返回可解析的 JSON".to_string(),
            raw: raw.to_string(),
        })?;
        let mut report: AiReport = serde_json::from_str(&json).map_err(|e| ParseError {
            message: format!("JSON 结构与约定不符：{e}"),
            raw: raw.to_string(),
        })?;

        // 空字段补默认值是正常情况（模型可能只给 proposals），
        // 但完全空的回复要当成失败——否则界面会显示一份「分析成功、什么都没有」的方案。
        if report.summary.trim().is_empty()
            && report.proposals.is_empty()
            && report.categories.is_empty()
            && report.risks.is_empty()
        {
            return Err(ParseError {
                message: "模型返回了空结果".to_string(),
                raw: raw.to_string(),
            });
        }
        report.summary = report.summary.trim().to_string();
        Ok(report)
    }

    pub fn proposal_count(&self) -> usize {
        self.proposals
            .iter()
            .filter(|p| !matches!(p.action, Some(ProposalAction::Keep)) && !p.path.is_empty())
            .count()
    }
}

/// 从一段文本里找出最外层的 JSON 对象。
///
/// 用括号计数而不是正则：提示词允许模型在 JSON 前后写一句话，
/// 而正则处理嵌套对象时必然出错。字符串内的括号要跳过，否则
/// 一条路径里带 `}` 就会截断。
fn extract_json(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(raw[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json() {
        let raw = r#"{"summary":"有几个大件","proposals":[{"path":"~/a","action":"quarantine","reason":"可重建","est_bytes":100}]}"#;
        let r = AiReport::parse(raw).unwrap();
        assert_eq!(r.summary, "有几个大件");
        assert_eq!(r.proposals[0].action, Some(ProposalAction::Quarantine));
        assert_eq!(r.proposal_count(), 1);
    }

    #[test]
    fn parses_json_wrapped_in_a_fence() {
        let raw = "好的，分析如下：\n```json\n{\"summary\":\"ok\"}\n```\n希望有帮助。";
        let r = AiReport::parse(raw).unwrap();
        assert_eq!(r.summary, "ok");
    }

    #[test]
    fn braces_inside_strings_do_not_break_the_scan() {
        let raw = r#"{"summary":"路径里有个 } 字符"}"#;
        let r = AiReport::parse(raw).unwrap();
        assert!(r.summary.contains('}'));
    }

    #[test]
    fn keep_action_is_not_counted_as_a_proposal() {
        let raw = r#"{"summary":"x","proposals":[
            {"path":"~/a","action":"keep","reason":"不建议动"},
            {"path":"~/b","action":"compress","reason":"很少用"}]}"#;
        let r = AiReport::parse(raw).unwrap();
        assert_eq!(r.proposal_count(), 1);
    }

    #[test]
    fn empty_result_is_an_error_not_a_success() {
        let raw = r#"{"summary":"","proposals":[],"categories":[],"risks":[]}"#;
        assert!(AiReport::parse(raw).is_err());
    }

    #[test]
    fn garbage_carries_the_raw_text_back() {
        let raw = "抱歉，我无法完成这个请求。";
        let e = AiReport::parse(raw).unwrap_err();
        assert_eq!(e.raw, raw, "界面要能展示模型原话");
        assert!(e.message.contains("JSON"));
    }

    #[test]
    fn unknown_action_fails_loudly() {
        // action 拼错时不能悄悄当成 Keep——那会让「方案里少一项」变得无法解释
        let raw = r#"{"summary":"x","proposals":[{"path":"~/a","action":"delete","reason":"x"}]}"#;
        assert!(AiReport::parse(raw).is_err());
    }
}
