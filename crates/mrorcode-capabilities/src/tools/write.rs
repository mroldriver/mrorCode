//! write_file：整文件写入（Risky）。覆盖已存在文件时产出 unified diff。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};

use crate::path_guard::resolve_safe;
use crate::tools::diff::unified_diff;
use crate::tools::read_tracker::ReadTracker;

/// 单次写入内容上限（1 MiB）。
pub const MAX_WRITE_BYTES: usize = 1024 * 1024;

pub struct WriteFileTool {
    tracker: Arc<ReadTracker>,
}

impl WriteFileTool {
    pub fn new(tracker: Arc<ReadTracker>) -> Self {
        Self { tracker }
    }
}

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write a UTF-8 text file within the working directory (creates or overwrites). \
         Overwriting an existing file requires reading it first with read_file. \
         Returns a unified diff when replacing an existing file."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative to working directory or absolute)" },
                "content": { "type": "string", "description": "Full file content to write" }
            },
            "required": ["path", "content"]
        })
    }

    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        // 工作区内的文件写入：只读模式拒绝，工作区读写模式直行
        RiskLevel::WorkspaceWrite
    }

    async fn execute(&self, arguments: &str, ctx: &ToolContext<'_>) -> ToolResult {
        let args: Args = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::error(
                    "write_file",
                    json!({"error": format!("bad arguments: {e}")}).to_string(),
                );
            }
        };
        if args.content.len() > MAX_WRITE_BYTES {
            return ToolResult::error(
                "write_file",
                json!({"error": format!("content too large ({} bytes > {MAX_WRITE_BYTES})", args.content.len())})
                    .to_string(),
            );
        }

        let path = match resolve_safe(ctx.working_dir, &args.path) {
            Ok(p) => p,
            Err(message) => {
                return ToolResult::error(
                    "write_file",
                    json!({"path": args.path, "error": message}).to_string(),
                );
            }
        };

        let existing = std::fs::read_to_string(&path).ok();
        if existing.is_some() && !self.tracker.is_fresh(&path) {
            return ToolResult::error(
                "write_file",
                json!({
                    "path": args.path,
                    "error": "file changed on disk or was not read; read it with read_file before overwriting",
                })
                .to_string(),
            );
        }

        let result = tokio::task::spawn_blocking({
            let path = path.clone();
            let content = args.content.clone();
            move || write_blocking(&path, &content)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                self.tracker.mark_written(&path);
                let summary = match &existing {
                    Some(old) => match unified_diff(old, &args.content, &args.path, &args.path) {
                        Some(diff) => format!("Wrote {} (replaced)\n{diff}", args.path),
                        None => format!("Wrote {} (no changes)", args.path),
                    },
                    None => format!("Wrote {} ({} bytes)", args.path, args.content.len()),
                };
                ToolResult::ok("write_file", summary)
            }
            Ok(Err(message)) => ToolResult::error(
                "write_file",
                json!({"path": args.path, "error": message}).to_string(),
            ),
            Err(e) => ToolResult::error(
                "write_file",
                json!({"error": format!("join error: {e}")}).to_string(),
            ),
        }
    }
}

fn write_blocking(path: &std::path::Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            return Err(format!(
                "parent directory does not exist: {}",
                parent.display()
            ));
        }
    }
    std::fs::write(path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::request::RequestBroker;

    async fn test_requester() -> mrorcode_kernel::Requester {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        RequestBroker::new(tx).handle()
    }

    fn ctx_for<'a>(
        dir: &'a std::path::Path,
        cancel: &'a tokio_util::sync::CancellationToken,
        req: &'a mrorcode_kernel::Requester,
    ) -> ToolContext<'a> {
        ToolContext {
            working_dir: dir,
            cancel,
            request: req,
            tool_id: "",
            event_sink: None,
        }
    }

    #[tokio::test]
    async fn creates_new_file() {
        let dir = std::env::temp_dir().join(format!("mrorcode-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tracker = ReadTracker::new();
        let tool = WriteFileTool::new(tracker);
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ctx_for(&dir, &cancel, &req);

        let r = tool
            .execute(r#"{"path":"new.txt","content":"hello\n"}"#, &ctx)
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(
            std::fs::read_to_string(dir.join("new.txt")).unwrap(),
            "hello\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn overwrite_requires_read_first() {
        let dir = std::env::temp_dir().join(format!("mrorcode-write-rbw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "old").unwrap();
        let tracker = ReadTracker::new();
        let tool = WriteFileTool::new(tracker.clone());
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ctx_for(&dir, &cancel, &req);

        let r = tool
            .execute(r#"{"path":"a.txt","content":"new"}"#, &ctx)
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("read it with read_file"));

        // 标记已读后放行，并产出 diff
        tracker.mark(&dir.join("a.txt").canonicalize().unwrap());
        let r2 = tool
            .execute(r#"{"path":"a.txt","content":"new"}"#, &ctx)
            .await;
        assert!(!r2.is_error, "{}", r2.content);
        assert!(r2.content.contains("-old"));
        assert!(r2.content.contains("+new"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn rejects_escape() {
        let dir = std::env::temp_dir().join(format!("mrorcode-write-esc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = WriteFileTool::new(ReadTracker::new());
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ctx_for(&dir, &cancel, &req);
        let r = tool
            .execute(r#"{"path":"../../evil.txt","content":"x"}"#, &ctx)
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("escapes working directory"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
