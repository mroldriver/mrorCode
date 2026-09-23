//! 内核错误类型。

use thiserror::Error;

/// 内核层统一错误。
#[derive(Debug, Error)]
pub enum KernelError {
    #[error("agent has shut down")]
    Shutdown,
    #[error("tool execution failed: {0}")]
    Tool(String),
    #[error("provider error: {0}")]
    Provider(String),
}

pub type KernelResult<T> = Result<T, KernelError>;
