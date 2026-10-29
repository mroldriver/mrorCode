//! glob：按模式列出工作目录内文件（Safe，尊重 .gitignore）。

use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};

/// 结果条数上限。
pub const MAX_RESULTS: usize = 2000;

pub struct GlobTool;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by glob pattern (e.g. `**/*.rs`, `src/*.toml`) within the working directory. \
         Respects .gitignore and returns paths newest-first (max 2000)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern, e.g. **/*.rs" },
                "path": { "type": "string", "description": "Subdirectory to search (default working directory)" }
            },
            "required": ["pattern"]
        })
    }

    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::Safe
    }

    async fn execute(&self, arguments: &str, ctx: &ToolContext<'_>) -> ToolResult {
        let args: Args = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::error("glob", format!("bad arguments: {e}"));
            }
        };

        let root = match &args.path {
            Some(p) => match crate::path_guard::resolve_safe(ctx.working_dir, p) {
                Ok(p) => p,
                Err(message) => {
                    return ToolResult::error("glob", message);
                }
            },
            None => ctx.working_dir.to_path_buf(),
        };

        let workdir = ctx.working_dir.to_path_buf();
        let pattern = args.pattern.clone();
        let result = tokio::task::spawn_blocking(move || run_glob(&workdir, &root, &pattern)).await;

        match result {
            Ok(Ok(mut files)) => {
                if files.is_empty() {
                    return ToolResult::ok("glob", "[no files matched]");
                }
                let total = files.len();
                files.truncate(MAX_RESULTS);
                let mut out = files.join("\n");
                if total > MAX_RESULTS {
                    out.push_str(&format!("\n...[{} more matches]", total - MAX_RESULTS));
                }
                ToolResult::ok("glob", out)
            }
            Ok(Err(message)) => ToolResult::error("glob", message),
            Err(e) => ToolResult::error("glob", format!("join error: {e}")),
        }
    }
}

fn run_glob(workdir: &Path, root: &Path, pattern: &str) -> Result<Vec<String>, String> {
    let matcher = ignore::overrides::OverrideBuilder::new(root)
        .add(pattern)
        .map_err(|e| format!("invalid pattern: {e}"))?
        .build()
        .map_err(|e| format!("invalid pattern: {e}"))?;

    let mut entries: Vec<(std::time::SystemTime, String)> = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .git_global(true)
        .hidden(true)
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if !matcher.matched(path, false).is_whitelist() {
            continue;
        }
        let rel = path
            .strip_prefix(workdir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        entries.push((mtime, rel));
    }

    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(entries.into_iter().map(|(_, p)| p).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::request::RequestBroker;

    async fn test_requester() -> mrorcode_kernel::Requester {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        RequestBroker::new(tx).handle()
    }

    #[tokio::test]
    async fn matches_and_respects_gitignore() {
        let dir = std::env::temp_dir().join(format!("mrorcode-glob-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.join("ignored.rs"), "").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.rs\n").unwrap();
        // 让 ignore crate 识别 git 上下文
        std::fs::create_dir_all(dir.join(".git")).unwrap();

        let tool = GlobTool;
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
            tool_id: "",
            event_sink: None,
        };
        let r = tool.execute(r#"{"pattern":"**/*.rs"}"#, &ctx).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("main.rs"));
        assert!(!r.content.contains("ignored.rs"), "{}", r.content);
        std::fs::remove_dir_all(&dir).ok();
    }
}
