//! edit_file：精确串替换（Risky）。唯一匹配失败时返回结构化错误供模型自纠。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};

use crate::path_guard::resolve_existing_file;
use crate::tools::diff::unified_diff;
use crate::tools::read_tracker::ReadTracker;

pub struct EditFileTool {
    tracker: Arc<ReadTracker>,
}

impl EditFileTool {
    pub fn new(tracker: Arc<ReadTracker>) -> Self {
        Self { tracker }
    }
}

#[derive(Deserialize)]
struct Args {
    path: String,
    old: String,
    new: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file within the working directory. The file must be read first \
         with read_file. By default `old` must match exactly once; set replace_all to replace every \
         occurrence. Returns a unified diff of the change."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative to working directory or absolute)" },
                "old": { "type": "string", "description": "Exact text to find" },
                "new": { "type": "string", "description": "Replacement text" },
                "replace_all": { "type": "boolean", "description": "Replace all occurrences (default false)" }
            },
            "required": ["path", "old", "new"]
        })
    }

    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::Risky
    }

    async fn execute(&self, arguments: &str, ctx: &ToolContext<'_>) -> ToolResult {
        let args: Args = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::error(
                    "edit_file",
                    json!({"error": format!("bad arguments: {e}")}).to_string(),
                );
            }
        };
        if args.old.is_empty() {
            return ToolResult::error(
                "edit_file",
                json!({"error": "`old` must not be empty"}).to_string(),
            );
        }

        let path = match resolve_existing_file(ctx.working_dir, &args.path) {
            Ok(p) => p,
            Err(message) => {
                return ToolResult::error(
                    "edit_file",
                    json!({"path": args.path, "error": message}).to_string(),
                );
            }
        };
        if !self.tracker.is_fresh(&path) {
            return ToolResult::error(
                "edit_file",
                json!({
                    "path": args.path,
                    "error": "file changed on disk or was not read; read it with read_file before editing",
                })
                .to_string(),
            );
        }

        let count = count_matches(std::fs::read_to_string(&path), &args.old);
        if count == 0 {
            return ToolResult::error(
                "edit_file",
                json!({"path": args.path, "error": "`old` not found in file"}).to_string(),
            );
        }
        if count > 1 && !args.replace_all {
            return ToolResult::error(
                "edit_file",
                json!({
                    "path": args.path,
                    "error": format!("`old` is ambiguous: found {count} matches; add more context or set replace_all"),
                    "matches": count,
                })
                .to_string(),
            );
        }

        let result = tokio::task::spawn_blocking({
            let path = path.clone();
            let old = args.old.clone();
            let new = args.new.clone();
            let all = args.replace_all;
            move || apply_edit(&path, &old, &new, all)
        })
        .await;

        match result {
            Ok(Ok((before, after))) => {
                self.tracker.mark_written(&path);
                let diff = unified_diff(&before, &after, &args.path, &args.path)
                    .unwrap_or_else(|| "(no changes)".to_string());
                let replaced = if args.replace_all { count } else { 1 };
                ToolResult::ok(
                    "edit_file",
                    format!("Edited {} ({replaced} replacement(s))\n{diff}", args.path),
                )
            }
            Ok(Err(message)) => ToolResult::error(
                "edit_file",
                json!({"path": args.path, "error": message}).to_string(),
            ),
            Err(e) => ToolResult::error(
                "edit_file",
                json!({"error": format!("join error: {e}")}).to_string(),
            ),
        }
    }
}

fn count_matches(text: Result<String, std::io::Error>, old: &str) -> usize {
    text.map(|t| t.matches(old).count()).unwrap_or(0)
}

fn apply_edit(
    path: &std::path::Path,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<(String, String), String> {
    let before = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let after = if replace_all {
        before.replace(old, new)
    } else {
        before.replacen(old, new, 1)
    };
    std::fs::write(path, &after).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok((before, after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::request::RequestBroker;

    async fn test_requester() -> mrorcode_kernel::Requester {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        RequestBroker::new(tx).handle()
    }

    fn setup(tag: &str, content: &str) -> (std::path::PathBuf, Arc<ReadTracker>) {
        let dir = std::env::temp_dir().join(format!("mrorcode-edit-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), content).unwrap();
        let tracker = ReadTracker::new();
        tracker.mark(&dir.join("a.txt").canonicalize().unwrap());
        (dir, tracker)
    }

    #[tokio::test]
    async fn unique_replacement_produces_diff() {
        let (dir, tracker) = setup("unique", "hello world\n");
        let tool = EditFileTool::new(tracker);
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
        };
        let r = tool
            .execute(r#"{"path":"a.txt","old":"world","new":"rust"}"#, &ctx)
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("+hello rust"));
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "hello rust\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn ambiguous_match_is_rejected() {
        let (dir, tracker) = setup("ambig", "x\nx\n");
        let tool = EditFileTool::new(tracker);
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
        };
        let r = tool
            .execute(r#"{"path":"a.txt","old":"x","new":"y"}"#, &ctx)
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("ambiguous"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn replace_all_succeeds() {
        let (dir, tracker) = setup("all", "x\nx\n");
        let tool = EditFileTool::new(tracker);
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
        };
        let r = tool
            .execute(
                r#"{"path":"a.txt","old":"x","new":"y","replace_all":true}"#,
                &ctx,
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "y\ny\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn not_found_is_structured_error() {
        let (dir, tracker) = setup("notfound", "abc\n");
        let tool = EditFileTool::new(tracker);
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
        };
        let r = tool
            .execute(r#"{"path":"a.txt","old":"zzz","new":"q"}"#, &ctx)
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("not found"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
