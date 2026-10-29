//! 内置工具（M3：read、write、edit、bash、glob、grep）。

pub mod bash;
pub mod diff;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod read_tracker;
pub mod task;
/// 各工具单测共用的 `test_requester()` / `ctx_for()`（issue #13），只在测试构建里存在。
#[cfg(test)]
pub mod testkit;
pub mod write;

use std::sync::Arc;

pub use bash::BashTool;
pub use edit::EditFileTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use read::ReadFileTool;
pub use read_tracker::ReadTracker;
pub use task::TaskTool;
pub use write::WriteFileTool;

use mrorcode_kernel::tool::{Tool, ToolRegistry};

/// M3 内置编码工具集合。read/write/edit 共享同一 read-before-write 追踪器。
pub fn builtin_tools() -> Vec<Arc<dyn mrorcode_kernel::tool::Tool>> {
    let tracker = ReadTracker::new();
    vec![
        Arc::new(ReadFileTool::new(tracker.clone())),
        Arc::new(WriteFileTool::new(tracker.clone())),
        Arc::new(EditFileTool::new(tracker)),
        Arc::new(BashTool),
        Arc::new(GlobTool),
        Arc::new(GrepTool),
    ]
}

/// 注册全部内置工具到一个新注册表。
pub fn builtin_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for tool in builtin_tools() {
        reg.insert(tool);
    }
    reg
}

/// 只读工具子集（M4-10）：供 `task` 子 agent 使用，绝不含写/改/bash。
pub fn read_only_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFileTool::default()),
        Arc::new(GlobTool),
        Arc::new(GrepTool),
    ]
}

/// 由只读工具列表构造注册表。
pub fn read_only_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for tool in read_only_tools() {
        reg.insert(tool);
    }
    reg
}
