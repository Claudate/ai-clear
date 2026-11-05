//! 可复用组件。
//!
//! 这里的每个组件都只吃显式参数（props），**不读 `AppState`**。
//! `widgets` 里的小零件是「画一个控件」，而这里的东西是「一块有自己语义的区域」：
//! 提示条、确认框、横幅、勾选框。它们之所以能复用，正是因为不知道状态从哪来——
//! 谁调用谁负责把数据准备好、把结果收走。
//!
//! 为什么要分出来：这些块原先分别散在 `app.rs` 与几个页面文件里，
//! 结构几乎一样但各自演化（`short_time` 有三份一模一样的实现，
//! 两个确认框连内边距都是复制的）。复制三遍之后，改一处就会漏掉另外两处。

pub mod banner;
pub mod checkbox;
pub mod confirm_dialog;
pub mod create_window;
pub mod status_bar;
pub mod time;
pub mod tools_menu;

pub use confirm_dialog::{ConfirmDialog, ConfirmKind, ConfirmOutcome};
pub use create_window::{CreateOutcome, CreateProps, CreateWindow};
pub use status_bar::{StatusBar, StatusBarOutcome, StatusTone};
pub use tools_menu::{ToolGroup, ToolId, ToolState, ToolsMenu, ToolsMenuProps, GROUPS};
