//! 五个页面。按职责分文件——每页只关心自己怎么画，
//! 状态与业务规则都在 `state` / `worker` / `schedule` / `molt-act` 里。
//!
//! 原来的「概览 / 清单 / AI 分析」三页合成了一页 `workbench`：
//! AI 解释不再是独立页面，而是扫描结果每一行上的说明。

pub mod automation;
pub mod history;
pub mod quarantine;
pub mod settings;
pub mod workbench;
