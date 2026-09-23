//! mrorcode-capabilities: 挂载到 kernel 扩展缝上的具体能力（L1）。
//!
//! 编译期只依赖 kernel 与第三方 crate，不依赖 agent/cli/tui。

#[cfg(feature = "providers")]
pub mod provider;

#[cfg(feature = "tools")]
pub mod approval;

#[cfg(feature = "tools")]
pub mod tools;
