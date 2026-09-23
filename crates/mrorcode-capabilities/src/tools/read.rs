//! read_file：只读文件读取工具（Safe）。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};

/// 单文件大小硬上限（超出必须用 offset/limit 分页）。
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// 默认/最大行数页。
pub const DEFAULT_LIMIT: usize = 2000;

pub struct ReadFileTool;

#[derive(Deserialize)]
struct Args {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file within the working directory. Supports 1-based line pagination via offset/limit."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path (relative to working directory or absolute)" },
                "offset": { "type": "integer", "description": "1-based starting line", "minimum": 1 },
                "limit": { "type": "integer", "description": "Max lines to read", "minimum": 1, "maximum": 2000 }
            },
            "required": ["path"]
        })
    }

    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::Safe
    }

    async fn execute(&self, arguments: &str, ctx: &ToolContext<'_>) -> ToolResult {
        let args: Args = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::error(
                    "read_file",
                    json!({"error": format!("bad arguments: {e}")}).to_string(),
                );
            }
        };

        let path = resolve_inside(ctx.working_dir, &args.path);
        let result =
            tokio::task::spawn_blocking(move || read_blocking(&path, args.offset, args.limit))
                .await;
        match result {
            Ok(Ok(content)) => ToolResult::ok("read_file", content),
            Ok(Err(message)) => ToolResult::error(
                "read_file",
                json!({"path": args.path, "error": message}).to_string(),
            ),
            Err(e) => ToolResult::error(
                "read_file",
                json!({"error": format!("join error: {e}")}).to_string(),
            ),
        }
    }
}

fn resolve_inside(working_dir: &Path, p: &str) -> PathBuf {
    let candidate = PathBuf::from(p);
    if candidate.is_absolute() {
        candidate
    } else {
        working_dir.join(candidate)
    }
}

fn read_blocking(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let meta =
        std::fs::metadata(path).map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
    if meta.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    if meta.len() > MAX_FILE_BYTES && offset.is_none() {
        return Err(format!(
            "file is {} bytes (limit {MAX_FILE_BYTES}); read it in pages with offset/limit",
            meta.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);

    let start = offset.unwrap_or(1).saturating_sub(1);
    let take = limit.unwrap_or(DEFAULT_LIMIT).min(DEFAULT_LIMIT);

    let mut out = String::new();
    for (i, line) in text.lines().skip(start).take(take).enumerate() {
        out.push_str(&format!("{:>6}\t{line}\n", start + i + 1));
    }
    if out.is_empty() {
        out.push_str("[empty file or page]\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_with_line_numbers_and_pagination() {
        let dir = std::env::temp_dir().join(format!("mrorcode-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "l1\nl2\nl3\n").unwrap();

        let tool = ReadFileTool;
        let cancel = tokio_util::sync::CancellationToken::new();
        let requester = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &requester,
        };

        let r = tool.execute(r#"{"path":"a.txt"}"#, &ctx).await;
        assert!(r.content.contains("l2"));

        let r2 = tool
            .execute(r#"{"path":"a.txt","offset":2,"limit":1}"#, &ctx)
            .await;
        assert!(r2.content.contains("l2"));
        assert!(!r2.content.contains("l3"));

        std::fs::remove_dir_all(&dir).ok();
    }

    async fn test_requester() -> mrorcode_kernel::Requester {
        // 只读工具不会发请求；构造一个指向已关闭通道的句柄即可
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        mrorcode_kernel::request::RequestBroker::new(tx).handle()
    }
}
