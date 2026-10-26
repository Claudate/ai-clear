//! AI 配置：用哪一家、哪个模型、密钥从哪来。
//!
//! 配置文件放在 `~/.config/molt/ai.toml`，与规则库同一个目录——
//! 用户已经知道「molt 的配置都在 .config/molt 里」，不必再记第二个位置。
//!
//! 三条硬约束：
//! - **模型列表完全来自配置**，代码里不写死任何模型 id。内置文件只是首次启动的模板，
//!   用户加一个模型、换一个中转地址，都不用改程序。
//! - **密钥优先读环境变量**。把 key 明文写进配置文件是最常见的泄漏来源
//!   （备份、截图、误提交），所以 `api_key_env` 优先；`api_key` 只是给
//!   「我就是想放本地」的人留的，且会在界面上被提示风险。
//! - **没配置就不假装能分析**。`resolve()` 失败时界面必须显示原因，
//!   而本地扫描与手动处理完全不依赖它。

use std::path::{Path, PathBuf};

/// 删除判断默认使用的服务与模型。放在这里而不是散在调用点，
/// 是为了让「默认用谁」只有一处可改，测试也能直接引用同一个常量。
pub const JEV_PROVIDER_ID: &str = "typesafe";
pub const JEV_MODEL_ID: &str = "jev";

/// 接口协议。只区分两种：OpenAI 兼容（绝大多数第三方与中转都用它）
/// 与 Anthropic 原生。多一种协议就多一处解析分叉，没有必要。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKind {
    /// `POST {base_url}/chat/completions`
    OpenAiCompatible,
    /// `POST {base_url}/v1/messages`
    Anthropic,
    /// `POST {base_url}/v1/systemone`：TypeSafe 的 System One 协议，
    /// 发状态与「带类型的问题」，拿回 choice / score / noul 这类结构化判断。
    /// 它不是聊天接口，所以不能塞进上面两条分支里假装是 chat。
    SystemOne,
}

impl ApiKind {
    pub fn label(self) -> &'static str {
        match self {
            ApiKind::OpenAiCompatible => "OpenAI 兼容",
            ApiKind::Anthropic => "Anthropic",
            ApiKind::SystemOne => "TypeSafe System One",
        }
    }
}

/// 一个可选模型。`context_tokens` 用来决定摘要能发多大——
/// 不按模型的实际窗口裁剪，小窗口模型会直接被 API 拒绝，用户看不出原因。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_context")]
    pub context_tokens: u32,
    /// 该模型是否支持 JSON 模式。不支持时请求里不带 `response_format`，
    /// 而是靠提示词约束 + 本地容错解析。
    #[serde(default)]
    pub json_mode: bool,
}

fn default_context() -> u32 {
    32_000
}

impl Model {
    /// 界面显示名：没写 label 就退回 id，不留空。
    pub fn label_or_id(&self) -> String {
        if self.label.trim().is_empty() {
            self.id.clone()
        } else {
            self.label.clone()
        }
    }
}

/// 一家服务。`official` 只影响界面分组文案，不影响调用方式。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Provider {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub kind: ApiKind,
    /// 例如 `https://api.anthropic.com` 或 `https://api.deepseek.com/v1`。
    pub base_url: String,
    /// 环境变量名。优先于 `api_key`。
    #[serde(default)]
    pub api_key_env: String,
    /// 直接写在配置里的密钥。留空即表示只用环境变量。
    #[serde(default)]
    pub api_key: String,
    /// 是否属于「官方」服务。仅用于界面上分组。
    #[serde(default)]
    pub official: bool,
    #[serde(default, rename = "model")]
    pub models: Vec<Model>,
}

impl std::fmt::Debug for Provider {
    /// 手写 Debug：`api_key` 明文绝不能进日志、panic 消息或用户的错误截图。
    /// derive 出来的 Debug 会把它原样打出来，而这份配置正好是最容易被整份打印的东西。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key_env", &self.api_key_env)
            .field(
                "api_key",
                &if self.api_key.trim().is_empty() {
                    "<未设置>"
                } else {
                    "<已隐去>"
                },
            )
            .field("models", &self.models.len())
            .finish()
    }
}

impl Provider {
    pub fn display(&self) -> &str {
        if self.label.is_empty() {
            &self.id
        } else {
            &self.label
        }
    }

    /// 密钥的实际来源。返回 `None` 表示这一家还没配。
    pub fn key(&self) -> Option<String> {
        if !self.api_key_env.is_empty() {
            if let Ok(v) = std::env::var(&self.api_key_env) {
                if !v.trim().is_empty() {
                    return Some(v.trim().to_string());
                }
            }
        }
        let inline = self.api_key.trim();
        if inline.is_empty() {
            None
        } else {
            Some(inline.to_string())
        }
    }

    /// 密钥是不是来自环境变量。界面据此显示「来自 $XXX」或「写在配置文件里」。
    pub fn key_from_env(&self) -> bool {
        !self.api_key_env.is_empty()
            && std::env::var(&self.api_key_env)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
    }
}

/// 整份配置。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct AiConfig {
    pub schema_version: u32,
    /// 当前选中的服务 id。空表示用户还没选。
    #[serde(default)]
    pub active_provider: String,
    /// 当前选中的模型 id。必须属于选中的服务。
    #[serde(default)]
    pub active_model: String,
    /// 分析时是否允许采样文件头部内容。默认关闭——
    /// 默认只发元数据，token 与隐私风险都可控。
    #[serde(default)]
    pub allow_content_sample: bool,
    #[serde(default, rename = "provider")]
    pub providers: Vec<Provider>,
}

/// 配置出错的原因。每种都要能直接显示给用户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// 配置文件不存在——这不是错误，只是「还没配」
    Missing(PathBuf),
    /// 文件读不了或写不成
    Io(String),
    /// TOML 语法或字段不对
    Parse(String),
    /// 没有选中任何服务
    NoProvider,
    /// 选中的服务 id 在配置里找不到
    UnknownProvider(String),
    /// 没有选中模型
    NoModel,
    /// 选中的模型不属于当前服务
    UnknownModel { provider: String, model: String },
    /// 服务存在、模型也有，但密钥没配
    NoKey { provider: String, env: String },
    /// 服务缺 base_url
    NoBaseUrl(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Missing(p) => write!(
                f,
                "还没有 AI 配置（{}）。可点「AI 配置」选一家并填密钥；不配置也能正常扫描与手动清理。",
                p.display()
            ),
            ConfigError::Io(e) => write!(f, "读写 AI 配置失败：{e}"),
            ConfigError::Parse(e) => write!(f, "AI 配置格式有误：{e}"),
            ConfigError::NoProvider => write!(f, "还没选 AI 服务，请到「AI 配置」里选一家"),
            ConfigError::UnknownProvider(id) => {
                write!(f, "配置里选的服务 {id} 不存在，请重新选择")
            }
            ConfigError::NoModel => write!(f, "还没选模型，请到「AI 配置」里选一个"),
            ConfigError::UnknownModel { provider, model } => {
                write!(f, "服务 {provider} 下没有模型 {model}，请重新选择")
            }
            ConfigError::NoKey { provider, env } => write!(
                f,
                "服务 {provider} 还没有可用密钥：请设置环境变量 {env}，或在配置里填写 api_key"
            ),
            ConfigError::NoBaseUrl(id) => write!(f, "服务 {id} 缺少 base_url"),
        }
    }
}

/// 已经可以发起请求的配置。
#[derive(Clone)]
pub struct Resolved {
    pub provider_id: String,
    pub provider_label: String,
    pub kind: ApiKind,
    pub base_url: String,
    pub model: String,
    pub model_label: String,
    pub context_tokens: u32,
    pub json_mode: bool,
    /// 不要 Debug 打印它——见 `impl std::fmt::Debug for Resolved`。
    key: String,
}

impl std::fmt::Debug for Resolved {
    /// 手写 Debug：密钥绝不能进日志、进 panic 消息、进用户的错误截图。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("provider_id", &self.provider_id)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("key", &"<已隐去>")
            .finish()
    }
}

impl Resolved {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// 界面上显示「Claude Sonnet（官方）」这类一行摘要。
    pub fn summary(&self) -> String {
        format!("{} · {}", self.provider_label, self.model_label)
    }
}

impl Default for AiConfig {
    /// 首次启动的模板：一家官方 + 两家第三方，密钥全部留空。
    ///
    /// 模板写进配置文件而不是藏在代码里，是为了让用户能直接看到
    /// 「原来还能这样加一家」。代码里不出现任何模型 id 的硬编码使用——
    /// `active_model` 默认留空，强制用户自己选一次。
    fn default() -> Self {
        Self {
            schema_version: 1,
            active_provider: String::new(),
            active_model: String::new(),
            allow_content_sample: false,
            providers: vec![
                Provider {
                    id: "anthropic".into(),
                    label: "Claude（官方）".into(),
                    kind: ApiKind::Anthropic,
                    base_url: "https://api.anthropic.com".into(),
                    api_key_env: "ANTHROPIC_API_KEY".into(),
                    api_key: String::new(),
                    official: true,
                    models: vec![
                        Model {
                            id: "claude-sonnet-4-5".into(),
                            label: "Claude Sonnet 4.5".into(),
                            context_tokens: 200_000,
                            json_mode: false,
                        },
                        Model {
                            id: "claude-haiku-4-5".into(),
                            label: "Claude Haiku 4.5".into(),
                            context_tokens: 200_000,
                            json_mode: false,
                        },
                    ],
                },
                Provider {
                    id: "openai".into(),
                    label: "OpenAI（官方）".into(),
                    kind: ApiKind::OpenAiCompatible,
                    base_url: "https://api.openai.com/v1".into(),
                    api_key_env: "OPENAI_API_KEY".into(),
                    api_key: String::new(),
                    official: true,
                    models: vec![
                        Model {
                            id: "gpt-4.1".into(),
                            label: "GPT-4.1".into(),
                            context_tokens: 128_000,
                            json_mode: true,
                        },
                        Model {
                            id: "gpt-4o-mini".into(),
                            label: "GPT-4o mini".into(),
                            context_tokens: 128_000,
                            json_mode: true,
                        },
                    ],
                },
                Provider {
                    id: "deepseek".into(),
                    label: "DeepSeek（第三方）".into(),
                    kind: ApiKind::OpenAiCompatible,
                    base_url: "https://api.deepseek.com/v1".into(),
                    api_key_env: "DEEPSEEK_API_KEY".into(),
                    api_key: String::new(),
                    official: false,
                    models: vec![Model {
                        id: "deepseek-chat".into(),
                        label: "DeepSeek Chat".into(),
                        context_tokens: 64_000,
                        json_mode: true,
                    }],
                },
                Provider {
                    id: "ollama".into(),
                    label: "本地 Ollama（第三方）".into(),
                    kind: ApiKind::OpenAiCompatible,
                    base_url: "http://127.0.0.1:11434/v1".into(),
                    // 本地服务通常不校验密钥；留空字符串即可
                    api_key_env: "OLLAMA_API_KEY".into(),
                    api_key: "local".into(),
                    official: false,
                    models: vec![Model {
                        id: "qwen2.5:14b".into(),
                        label: "Qwen2.5 14B".into(),
                        context_tokens: 32_000,
                        json_mode: true,
                    }],
                },
                Provider {
                    id: "typesafe".into(),
                    label: "TypeSafe Jev（第三方）".into(),
                    kind: ApiKind::SystemOne,
                    // System One 的路径是 /v1/systemone，客户端在这里只拼后缀，
                    // 所以 base_url 只写到主机名，和官方文档的 base URL 一致。
                    base_url: "https://api.typesafe.ai".into(),
                    api_key_env: "TYPESAFE_API_KEY".into(),
                    api_key: String::new(),
                    official: false,
                    models: vec![Model {
                        id: "jev".into(),
                        label: "jev".into(),
                        // Jev 的上下文窗口按官方 1.14 的 32k 计；
                        // 删除判断的摘要本来就按上限裁剪，不靠这里放大。
                        context_tokens: 32_000,
                        // System One 返回的是 typed answers，不是聊天 JSON，
                        // 请求里不会带 response_format。
                        json_mode: false,
                    }],
                },
            ],
        }
    }
}

/// 配置文件路径。与规则库同目录。
///
/// 放在这里而不是 `molt-act::paths`：那是「写盘数据的归属」，
/// 而配置是只读的偏好；让 `molt-ai` 为一条路径依赖整个写盘层（含 rusqlite）
/// 不划算，也让「AI 层不写盘」这条承诺变得含糊。
pub fn default_path() -> PathBuf {
    // `MOLT_HOME` 优先，和 `molt-act::paths::home_dir` 用同一套规则：
    // 它的作用是「把整份数据指到一个假家目录」，端到端验证靠它才不会
    // 把测试写进真实的 `~/.config/molt/ai.toml`。这里各算各的，就会
    // 出现「清理在假 home、配置在真 home」这种半截隔离，而配置里放的是密钥。
    let home = std::env::var_os("MOLT_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    home.join(".config").join("molt").join("ai.toml")
}

impl std::fmt::Debug for AiConfig {
    /// 同 `Provider`：整份配置也不允许被直接打印出密钥。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiConfig")
            .field("active_provider", &self.active_provider)
            .field("active_model", &self.active_model)
            .field("allow_content_sample", &self.allow_content_sample)
            .field("providers", &self.providers)
            .finish()
    }
}

impl AiConfig {
    /// 读配置。文件不存在时返回内置模板（不写盘），让界面能显示「有这些可选」。
    ///
    /// 老版本写下的 `ai.toml` 里没有 jev 这一家。读进来后只**在内存里**补上缺失的
    /// jev 条目：用户的其他服务、密钥、选择一概不动，也不替用户写盘——
    /// 他下次点「保存」时才落盘。不补的话，升级后删除判断会报「还没配置」，
    /// 而用户看到的配置界面里也确实没有 jev。
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut cfg: Self =
                    toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string()))?;
                cfg.add_missing_jev();
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut cfg = Self::default();
                // 用户可能已经设了某个环境变量：选中第一家「密钥已就绪」的服务，
                // 减少一次配置。但模型仍然强制用户自己选。
                if let Some(p) = cfg.providers.iter().find(|p| p.key().is_some()) {
                    cfg.active_provider = p.id.clone();
                }
                Ok(cfg)
            }
            Err(e) => Err(ConfigError::Io(e.to_string())),
        }
    }

    /// 只补 jev 这一条，且只在完全没有该服务时补。
    ///
    /// 用户若已经自己加过 `typesafe` 或改过它的地址/密钥，一律按他的来——
    /// 这里不做「升级成默认值」这种事，那会把别人的配置悄悄改掉。
    fn add_missing_jev(&mut self) {
        if self.provider(JEV_PROVIDER_ID).is_some() {
            return;
        }
        let Some(default_provider) = Self::default()
            .providers
            .into_iter()
            .find(|p| p.id == JEV_PROVIDER_ID)
        else {
            return;
        };
        self.providers.push(default_provider);
    }

    /// 写回配置。只有用户在界面上点「保存」时才调用。
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| ConfigError::Io(e.to_string()))?;
        std::fs::write(path, text).map_err(|e| ConfigError::Io(e.to_string()))
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn active_provider(&self) -> Option<&Provider> {
        self.provider(&self.active_provider)
    }

    /// 当前服务下选中的模型。
    pub fn active_model(&self) -> Option<&Model> {
        let p = self.active_provider()?;
        p.models.iter().find(|m| m.id == self.active_model)
    }

    /// 切换服务。模型跟着切到该服务的第一个——否则「服务换了、模型还是旧的」
    /// 会变成一条必然失败的配置，而用户会以为是程序坏了。
    pub fn set_provider(&mut self, id: &str) {
        self.active_provider = id.to_string();
        self.active_model = self
            .provider(id)
            .and_then(|p| p.models.first())
            .map(|m| m.id.clone())
            .unwrap_or_default();
    }

    /// 删除判断用的配置。与 `resolve()` 分开，是因为这两件事的默认值不同：
    /// 通用分析沿用用户当前选择；删除判断在用户没指定模型时落到 jev。
    ///
    /// 只在「默认模板 + 还没选模型」这一种情况下兜底，其余情况仍按原规则报错，
    /// 避免把一条错误的配置悄悄改成另一条。
    pub fn resolve_for_delete_judge(&self) -> Result<Resolved, ConfigError> {
        if self.can_default_to_jev() {
            let mut cfg = self.clone();
            cfg.active_provider = JEV_PROVIDER_ID.to_string();
            cfg.active_model = JEV_MODEL_ID.to_string();
            return cfg.resolve();
        }
        self.resolve()
    }

    /// 是否可以把删除判断回落到默认模板里的 jev。
    ///
    /// 三个条件同时成立才算「用户没选」：配置来自内置模板、没选服务、没选模型。
    /// 用户自己改了任何一项，都按他自己的选择走。
    fn can_default_to_jev(&self) -> bool {
        self.active_provider.trim().is_empty()
            && self.active_model.trim().is_empty()
            && self
                .provider(JEV_PROVIDER_ID)
                .map(|p| p.models.iter().any(|m| m.id == JEV_MODEL_ID))
                .unwrap_or(false)
    }

    /// 解析成可以直接发请求的样子。所有失败都带可读原因。
    pub fn resolve(&self) -> Result<Resolved, ConfigError> {
        if self.active_provider.trim().is_empty() {
            return Err(ConfigError::NoProvider);
        }
        let p = self
            .provider(&self.active_provider)
            .ok_or_else(|| ConfigError::UnknownProvider(self.active_provider.clone()))?;
        if p.base_url.trim().is_empty() {
            return Err(ConfigError::NoBaseUrl(p.id.clone()));
        }
        if self.active_model.trim().is_empty() {
            return Err(ConfigError::NoModel);
        }
        let m = p
            .models
            .iter()
            .find(|m| m.id == self.active_model)
            .ok_or_else(|| ConfigError::UnknownModel {
                provider: p.id.clone(),
                model: self.active_model.clone(),
            })?;
        let key = p.key().ok_or_else(|| ConfigError::NoKey {
            provider: p.display().to_string(),
            env: if p.api_key_env.is_empty() {
                "（未指定）".to_string()
            } else {
                p.api_key_env.clone()
            },
        })?;

        Ok(Resolved {
            provider_id: p.id.clone(),
            provider_label: p.display().to_string(),
            kind: p.kind,
            base_url: p.base_url.trim_end_matches('/').to_string(),
            model: m.id.clone(),
            model_label: if m.label.is_empty() {
                m.id.clone()
            } else {
                m.label.clone()
            },
            context_tokens: m.context_tokens,
            json_mode: m.json_mode,
            key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AiConfig {
        AiConfig::default()
    }

    #[test]
    fn default_template_offers_both_official_and_third_party() {
        let c = cfg();
        assert!(c.providers.iter().any(|p| p.official), "应提供官方服务");
        assert!(c.providers.iter().any(|p| !p.official), "应提供第三方服务");
        assert!(
            c.providers.iter().all(|p| !p.models.is_empty()),
            "每家都应带可选模型"
        );
    }

    #[test]
    fn resolve_requires_an_explicit_model_choice() {
        let mut c = cfg();
        c.active_provider = "deepseek".into();
        // 没有 active_model：必须报「还没选模型」，而不是偷偷用第一个
        assert_eq!(c.resolve().unwrap_err(), ConfigError::NoModel);
    }

    #[test]
    fn resolve_uses_the_model_from_config_not_a_hardcoded_one() {
        let mut c = cfg();
        c.active_provider = "deepseek".into();
        // 用户在配置里写了一个模板里没有的模型
        c.providers[2].models.push(Model {
            id: "deepseek-reasoner".into(),
            label: "DeepSeek Reasoner".into(),
            context_tokens: 64_000,
            json_mode: true,
        });
        c.active_model = "deepseek-reasoner".into();
        c.providers[2].api_key = "sk-test".into();

        let r = c.resolve().expect("应能解析");
        assert_eq!(r.model, "deepseek-reasoner");
        assert_eq!(r.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn switching_provider_moves_the_model_with_it() {
        let mut c = cfg();
        c.active_provider = "deepseek".into();
        c.active_model = "deepseek-chat".into();
        c.set_provider("openai");
        assert_eq!(c.active_provider, "openai");
        assert_eq!(c.active_model, "gpt-4.1", "换服务后模型必须属于新服务");
        // 不断言 resolve 成功：openai 那家在这个测试里未必有密钥。
        // 这里要守的是「配置自洽」，不是「密钥齐备」。
        assert!(c.active_model().is_some(), "模型必须能在新服务下找到");
    }

    #[test]
    fn missing_key_is_reported_with_the_env_var_name() {
        let mut c = cfg();
        c.active_provider = "openai".into();
        c.active_model = "gpt-4.1".into();
        c.providers[1].api_key.clear();
        // 环境变量名要出现在错误里，否则用户不知道该设什么
        match c.resolve() {
            Err(ConfigError::NoKey { env, .. }) => assert_eq!(env, "OPENAI_API_KEY"),
            other => panic!("应报缺密钥，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_model_is_rejected_instead_of_silently_falling_back() {
        let mut c = cfg();
        c.active_provider = "deepseek".into();
        c.active_model = "not-a-model".into();
        c.providers[2].api_key = "sk-test".into();
        assert!(matches!(c.resolve(), Err(ConfigError::UnknownModel { .. })));
    }

    #[test]
    fn resolved_debug_never_leaks_the_key() {
        let mut c = cfg();
        c.active_provider = "deepseek".into();
        c.active_model = "deepseek-chat".into();
        c.providers[2].api_key = "sk-super-secret-value".into();
        let r = c.resolve().unwrap();
        assert!(!format!("{r:?}").contains("sk-super-secret-value"));
        assert!(
            !format!("{c:?}").contains("sk-super-secret"),
            "整份配置也不该被这样打印"
        );
    }

    #[test]
    fn default_template_ships_jev() {
        let c = cfg();
        let p = c.provider(JEV_PROVIDER_ID).expect("默认模板应带 typesafe");
        assert_eq!(p.kind, ApiKind::SystemOne);
        assert_eq!(p.base_url, "https://api.typesafe.ai");
        let m = p
            .models
            .iter()
            .find(|m| m.id == JEV_MODEL_ID)
            .expect("应带 jev 模型");
        assert_eq!(m.label, "jev");
        assert!(!m.json_mode, "System One 不用聊天 JSON 模式");
    }

    #[test]
    fn delete_judge_falls_back_to_jev_only_when_nothing_is_chosen() {
        let mut c = cfg();
        c.providers
            .iter_mut()
            .find(|p| p.id == JEV_PROVIDER_ID)
            .unwrap()
            .api_key = "sk-test".into();

        let r = c.resolve_for_delete_judge().expect("未选择时应落到 jev");
        assert_eq!(r.provider_id, JEV_PROVIDER_ID);
        assert_eq!(r.model, JEV_MODEL_ID);

        // 用户选了别的模型：必须按他的来，不能被改回 jev。
        let mut chosen = c.clone();
        chosen.active_provider = "deepseek".into();
        chosen.active_model = "deepseek-chat".into();
        chosen.providers[2].api_key = "sk-test".into();
        let r = chosen.resolve_for_delete_judge().expect("应解析用户的选择");
        assert_eq!(r.model, "deepseek-chat");
    }

    #[test]
    fn loading_an_old_config_adds_jev_without_touching_other_providers() {
        let dir = std::env::temp_dir().join(format!("molt-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ai.toml");
        // 老版本的配置：只有 deepseek，没有 typesafe。
        std::fs::write(
            &path,
            r#"
schema_version = 1
active_provider = "deepseek"
active_model = "deepseek-chat"

[[provider]]
id = "deepseek"
kind = "open_ai_compatible"
base_url = "https://api.deepseek.com/v1"
api_key = "sk-old"

[[provider.model]]
id = "deepseek-chat"
label = "DeepSeek Chat"
"#,
        )
        .unwrap();

        let c = AiConfig::load(&path).unwrap();
        assert_eq!(c.active_provider, "deepseek", "用户的选择不能被改");
        assert_eq!(c.active_model, "deepseek-chat");
        assert_eq!(c.providers[0].api_key, "sk-old", "已有密钥不能被改");
        assert!(c.provider(JEV_PROVIDER_ID).is_some(), "应补上 jev");
        assert_eq!(
            c.resolve().unwrap().model,
            "deepseek-chat",
            "通用解析仍按用户选择"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn user_edited_typesafe_is_never_overwritten() {
        let mut c = cfg();
        let p = c
            .providers
            .iter_mut()
            .find(|p| p.id == JEV_PROVIDER_ID)
            .unwrap();
        p.base_url = "https://relay.example.com".into();
        c.add_missing_jev();
        let p = c.provider(JEV_PROVIDER_ID).unwrap();
        assert_eq!(
            p.base_url, "https://relay.example.com",
            "不能覆盖用户改过的地址"
        );
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut c = cfg();
        c.active_provider = "ollama".into();
        c.active_model = "qwen2.5:14b".into();
        c.allow_content_sample = true;
        let text = toml::to_string_pretty(&c).unwrap();
        let back: AiConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.active_provider, "ollama");
        assert_eq!(back.active_model, "qwen2.5:14b");
        assert!(back.allow_content_sample);
        assert_eq!(back.providers.len(), c.providers.len());
    }
}
