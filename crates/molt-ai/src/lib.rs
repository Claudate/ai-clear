//! `molt-ai`：AI 深度分析。
//!
//! 这一层只做三件事，且**不写盘**：
//! 1. 读配置，回答「用哪家、哪个模型、密钥在哪」（`config`）；
//! 2. 把扫描结果压成一份受控的元数据摘要，必要时按上限采样少量文本（`digest`）；
//! 3. 把 AI 的回复解析成结构化结果，再用确定性规则逐条核对成可执行方案（`report` / `plan`）。
//!
//! 为什么判定与执行都不在这里：AI 的输出是概率性的，而删除是不可逆动作。
//! 所以 AI 只能提出建议，**能不能动、动多少、目标路径是什么** 全部由本地代码
//! 依据扫描事实重新算一遍；受保护项、配置/密钥、唯一副本一律在核对阶段被拒。
//! 这条边界与 README 里「AI 不参与判定」一致，只是把「解释与建议」交给了 AI。

pub mod client;
pub mod config;
pub mod digest;
pub mod plan;
pub mod report;

pub use config::{AiConfig, ApiKind, ConfigError, Model, Provider, Resolved};
pub use digest::{ContentPolicy, Digest, DigestItem};
pub use plan::{explanations, Plan, PlanAction, PlanItem, PlanRoots, RejectedItem};
pub use report::{AiReport, Category, DuplicateGroup, Explanation, Proposal, Risk};
