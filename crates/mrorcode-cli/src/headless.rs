//! headless 一次性任务：普通流式文本 或 版本化 JSONL。

use serde_json::json;

use mrorcode_agent::{Assembly, RuntimeEvent};
use mrorcode_kernel::event::AgentEvent;

/// headless 使用固定会话 id（无交互，单任务单会话）。
pub const HEADLESS_SESSION: u64 = 1;

/// 运行一个 prompt 直到终态，返回进程退出码（0 成功）。
pub async fn run_headless(assembly: &mut Assembly, prompt: &str, json_mode: bool) -> i32 {
    assembly.handle.submit(HEADLESS_SESSION, prompt.to_string());

    let mut stop_ok = false;
    while let Some(ev) = assembly.events.recv().await {
        match ev {
            RuntimeEvent::Agent { event: ev, .. } => {
                match handle_agent_event(assembly, ev, json_mode) {
                    None => continue,
                    Some(success) => {
                        stop_ok = success;
                        break;
                    }
                }
            }
            RuntimeEvent::Exited => break,
        }
    }
    if json_mode {
        println!("{}", json!({"type":"run.finished","success":stop_ok}));
    }
    if stop_ok {
        0
    } else {
        1
    }
}

/// 处理单个事件：None=非终态，Some(success)=终态。
fn handle_agent_event(assembly: &Assembly, ev: AgentEvent, json_mode: bool) -> Option<bool> {
    match ev {
        AgentEvent::TurnStarted => {
            if json_mode {
                println!("{}", json!({"type":"run.started"}));
            }
            None
        }
        AgentEvent::TextDelta(t) => {
            if json_mode {
                println!("{}", json!({"type":"message.delta","delta":t}));
            } else {
                print!("{t}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            None
        }
        AgentEvent::Reasoning(t) => {
            if json_mode {
                println!("{}", json!({"type":"reasoning.delta","delta":t}));
            }
            None
        }
        AgentEvent::ToolStarted {
            id,
            name,
            arguments,
        } => {
            if json_mode {
                println!(
                    "{}",
                    json!({"type":"tool.started","id":id,"tool":name,"arguments":arguments})
                );
            } else {
                let preview = one_line_preview(&arguments);
                println!("$ {name} {preview}");
            }
            None
        }
        AgentEvent::ToolResult {
            id,
            name,
            content,
            is_error,
        } => {
            if json_mode {
                println!(
                    "{}",
                    json!({"type":"tool.completed","id":id,"tool":name,"is_error":is_error,"content":content})
                );
            } else {
                let mark = if is_error { "✗" } else { "✓" };
                println!("{mark} {name}");
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    for line in trimmed.lines().take(20) {
                        println!("  {line}");
                    }
                    if trimmed.lines().count() > 20 {
                        println!("  ...[output truncated]");
                    }
                }
            }
            None
        }
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => {
            if json_mode {
                println!(
                    "{}",
                    json!({"type":"usage","input_tokens":input_tokens,"output_tokens":output_tokens})
                );
            }
            None
        }
        AgentEvent::Request { id, kind, payload } => {
            // headless 无人应答：fail-closed 拒绝；skip-permissions 的 runtime
            // 根本不会发审批请求。
            if json_mode {
                println!(
                    "{}",
                    json!({"type":"permission.denied","id":id,"kind":kind,"payload":payload})
                );
            } else {
                eprintln!(
                    "[headless] approval request denied (rerun with --dangerously-skip-permissions to allow)"
                );
            }
            assembly.handle.respond(
                id,
                json!({"decision":"deny","reason":"headless mode: no interactive approval"}),
            );
            None
        }
        AgentEvent::TurnComplete { stop_reason } => {
            if json_mode {
                println!(
                    "{}",
                    json!({"type":"turn.completed","stop_reason":format!("{stop_reason:?}").to_lowercase()})
                );
            } else {
                println!();
            }
            Some(true)
        }
        AgentEvent::Error { message, retryable } => {
            if json_mode {
                println!(
                    "{}",
                    json!({"type":"error","retryable":retryable,"message":message})
                );
            } else {
                eprintln!("error: {message}");
            }
            Some(!retryable)
        }
        AgentEvent::Cancelled => {
            if json_mode {
                println!("{}", json!({"type":"cancelled"}));
            } else {
                eprintln!("cancelled");
            }
            Some(false)
        }
    }
}

fn one_line_preview(arguments: &str) -> String {
    let parsed: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let cmd = parsed
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| parsed.get("path").and_then(|v| v.as_str()))
        .unwrap_or("");
    cmd.chars().take(120).collect()
}
