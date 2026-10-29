//! grep：正则内容搜索（Safe，尊重 .gitignore，跳过二进制）。

use std::io::BufRead;

use async_trait::async_trait;
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::json;

use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};

/// 输出匹配行上限。
pub const MAX_MATCH_LINES: usize = 200;
/// 单文件扫描上限（超过视为不适合行搜索）。
pub const MAX_SCAN_BYTES: u64 = 2 * 1024 * 1024;

pub struct GrepTool;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    /// 只搜索匹配该 glob 的文件，如 `*.rs`。
    #[serde(default)]
    include: Option<String>,
    /// 大小写不敏感。
    #[serde(default)]
    ignore_case: bool,
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with a regular expression within the working directory. \
         Respects .gitignore, skips binaries, returns `path:line: text` (max 200 lines)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Rust regex pattern" },
                "path": { "type": "string", "description": "Subdirectory to search (default working directory)" },
                "include": { "type": "string", "description": "Only search files matching this glob, e.g. *.rs" },
                "ignore_case": { "type": "boolean", "description": "Case-insensitive match (default false)" }
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
                return ToolResult::error("grep", format!("bad arguments: {e}"));
            }
        };

        let root = match &args.path {
            Some(p) => match crate::path_guard::resolve_safe(ctx.working_dir, p) {
                Ok(p) => p,
                Err(message) => {
                    return ToolResult::error("grep", message);
                }
            },
            None => ctx.working_dir.to_path_buf(),
        };

        let workdir = ctx.working_dir.to_path_buf();
        let pattern = args.pattern.clone();
        let include = args.include.clone();
        let ignore_case = args.ignore_case;
        let result = tokio::task::spawn_blocking(move || {
            run_grep(&workdir, &root, &pattern, include.as_deref(), ignore_case)
        })
        .await;

        match result {
            Ok(Ok((lines, truncated))) => {
                if lines.is_empty() {
                    return ToolResult::ok("grep", "no matches");
                }
                let mut out = lines.join("\n");
                if truncated {
                    out.push_str(&format!("\n...[stopped after {MAX_MATCH_LINES} matches]"));
                }
                ToolResult::ok("grep", out)
            }
            Ok(Err(message)) => ToolResult::error("grep", message),
            Err(e) => ToolResult::error("grep", format!("join error: {e}")),
        }
    }
}

fn run_grep(
    workdir: &std::path::Path,
    root: &std::path::Path,
    pattern: &str,
    include: Option<&str>,
    ignore_case: bool,
) -> Result<(Vec<String>, bool), String> {
    let re = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| format!("invalid regex: {e}"))?;

    let include_matcher = match include {
        Some(glob) => {
            let overrides = ignore::overrides::OverrideBuilder::new(root)
                .add(glob)
                .map_err(|e| format!("invalid include glob: {e}"))?
                .build()
                .map_err(|e| format!("invalid include glob: {e}"))?;
            Some(overrides)
        }
        None => None,
    };

    let mut lines: Vec<String> = Vec::new();
    let mut truncated = false;

    let walker = ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .git_global(true)
        .hidden(true)
        .build();

    'outer: for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some(m) = &include_matcher {
            if !m.matched(path, false).is_whitelist() {
                continue;
            }
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > MAX_SCAN_BYTES {
            continue;
        }
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let rel = path
            .strip_prefix(workdir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        let reader = std::io::BufReader::new(file);
        for (i, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break, // 非 UTF-8/二进制行，放弃该文件剩余内容
            };
            if re.is_match(&line) {
                lines.push(format!("{}:{}: {}", rel, i + 1, line.trim_end()));
                if lines.len() >= MAX_MATCH_LINES {
                    truncated = true;
                    break 'outer;
                }
            }
        }
    }

    Ok((lines, truncated))
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
    async fn finds_lines_with_path_and_number() {
        let dir = std::env::temp_dir().join(format!("mrorcode-grep-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "let x = 1;\nfn foo() {}\n").unwrap();
        std::fs::write(dir.join("b.txt"), "no match here\n").unwrap();

        let tool = GrepTool;
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
            tool_id: "",
            event_sink: None,
        };
        let r = tool
            .execute(r#"{"pattern":"fn \\w+","include":"*.rs"}"#, &ctx)
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("src/a.rs:2:"), "{}", r.content);
        assert!(r.content.contains("fn foo"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn invalid_regex_is_error() {
        let dir = std::env::temp_dir().join(format!("mrorcode-grep-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = GrepTool;
        let cancel = tokio_util::sync::CancellationToken::new();
        let req = test_requester().await;
        let ctx = ToolContext {
            working_dir: &dir,
            cancel: &cancel,
            request: &req,
            tool_id: "",
            event_sink: None,
        };
        let r = tool.execute(r#"{"pattern":"(unclosed"}"#, &ctx).await;
        assert!(r.is_error);
        assert!(r.content.contains("invalid regex"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
