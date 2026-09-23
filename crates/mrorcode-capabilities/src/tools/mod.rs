//! 内置工具（M1：read_file、bash）。

pub mod bash;
pub mod read;

use std::sync::Arc;

pub use bash::BashTool;
pub use read::ReadFileTool;

use mrorcode_kernel::tool::ToolRegistry;

/// M1 内置编码工具集合。
pub fn builtin_tools() -> Vec<Arc<dyn mrorcode_kernel::tool::Tool>> {
    vec![Arc::new(ReadFileTool), Arc::new(BashTool)]
}

/// 注册全部内置工具到一个新注册表。
pub fn builtin_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for tool in builtin_tools() {
        reg.insert(tool);
    }
    reg
}
