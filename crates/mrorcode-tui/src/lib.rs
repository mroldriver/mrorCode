//! mrorcode-tui: ratatui 终端界面（L3 driver）。
//!
//! 视觉/交互对标 opencode：左竖线卡片、粘底滚动、审批弹窗；
//! 渲染采用脏标记驱动 + block 级缓存 + 窗口化绘制。

mod app;
pub mod bench_support;
mod clipboard;
mod commands;
mod fuzzy;
mod input;
mod keymap;
pub mod render;
mod selection;
mod storage;
mod store;
mod theme;
mod widgets;

pub use app::run;
pub use mrorcode_agent::Assembly;
