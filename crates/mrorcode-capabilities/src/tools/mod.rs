//! 内置工具（M3：read_file、write_file、edit_file、bash、glob、grep）。

pub mod bash;
pub mod diff;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod read_tracker;
pub mod write;

use std::sync::Arc;

pub use bash::BashTool;
pub use edit::EditFileTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use read::ReadFileTool;
pub use read_tracker::ReadTracker;
pub use write::WriteFileTool;

use mrorcode_kernel::tool::ToolRegistry;

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
