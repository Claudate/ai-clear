//! 「蜕」的界面库。
//!
//! 做成库而不是全塞进 `main.rs`，是为了让 `src/bin/shots.rs`
//! 能用同一份界面代码在无屏幕录制权限时出图——验证的画面和用户看到的
//! 必须是同一份实现，否则验了也不作数。

pub mod ai_worker;
pub mod app;
pub mod fonts;
pub mod pages;
pub mod paths;
pub mod schedule;
pub mod state;
pub mod theme;
pub mod widgets;
pub mod worker;

pub use app::MoltApp;
