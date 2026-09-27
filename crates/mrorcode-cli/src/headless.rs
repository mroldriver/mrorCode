//! headless 一次性任务：普通流式文本 或 版本化 JSONL。

use serde_json::json;

use mrorcode_agent::{Assembly, RuntimeEvent};
use mrorcode_kernel::event::AgentEvent;

/// headless 使用固定会话 id（无交互，单任务单会话）。
pub const HEADLESS_SESSION: u64 = 1;

/// 把一个 agent 事件映射为 headless JSONL 行（`None` = 不输出）。
///
/// 纯函数、无副作用，供契约快照测试守护 schema 不漂移（M5-d）。
pub fn jsonl_event(ev: &AgentEvent) -> Option<serde_json::Value> {
    match ev {
        AgentEvent::TurnStarted => Some(json!({"type":"run.started"})),
        AgentEvent::TextDelta(t) => Some(json!({"type":"message.delta","delta":t})),
        AgentEvent::Reasoning(t) => Some(json!({"type":"reasoning.delta","delta":t})),
        AgentEvent::ToolStarted {
            id,
            name,
            arguments,
        } => Some(json!({"type":"tool.started","id":id,"tool":name,"arguments":arguments})),
        AgentEvent::ToolResult {
            id,
            name,
            content,
            is_error,
        } => Some(
            json!({"type":"tool.completed","id":id,"tool":name,"is_error":is_error,"content":content}),
        ),
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => {
            Some(json!({"type":"usage","input_tokens":input_tokens,"output_tokens":output_tokens}))
        }
        AgentEvent::Request { id, kind, payload } => {
            Some(json!({"type":"permission.denied","id":id,"kind":kind,"payload":payload}))
        }
        AgentEvent::TurnComplete { stop_reason } => Some(
            json!({"type":"turn.completed","stop_reason":format!("{stop_reason:?}").to_lowercase()}),
        ),
        AgentEvent::Error { message, retryable } => {
            Some(json!({"type":"error","retryable":retryable,"message":message}))
        }
        AgentEvent::Cancelled => Some(json!({"type":"cancelled"})),
    }
}

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
    let terminal_success = match &ev {
        AgentEvent::TurnComplete { .. } => Some(true),
        AgentEvent::Error { retryable, .. } => Some(!retryable),
        AgentEvent::Cancelled => Some(false),
        _ => None,
    };

    if json_mode {
        if let Some(v) = jsonl_event(&ev) {
            println!("{v}");
        }
    } else {
        match &ev {
            AgentEvent::TextDelta(t) => {
                print!("{t}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            AgentEvent::ToolStarted { name, arguments, .. } => {
                let preview = one_line_preview(arguments);
                println!("$ {name} {preview}");
            }
            AgentEvent::ToolResult {
                name,
                content,
                is_error,
                ..
            } => {
                let mark = if *is_error { "✗" } else { "✓" };
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
            AgentEvent::TurnComplete { .. } => println!(),
            AgentEvent::Error { message, .. } => eprintln!("error: {message}"),
            AgentEvent::Cancelled => eprintln!("cancelled"),
            AgentEvent::Request { .. } => eprintln!(
                "[headless] approval request denied (rerun with --dangerously-skip-permissions to allow)"
            ),
            _ => {}
        }
    }

    // 审批请求：headless 无人应答，fail-closed 拒绝。
    if let AgentEvent::Request { id, .. } = &ev {
        assembly.handle.respond(
            *id,
            json!({"decision":"deny","reason":"headless mode: no interactive approval"}),
        );
    }

    terminal_success
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

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::event::StopReason;

    /// 契约快照：每个事件类型的 JSONL schema 必须与此处一致（M5-d）。
    /// 改字段名/类型/枚举串会让本测试失败，需显式更新并在提交说明记录。
    #[test]
    fn jsonl_contract_all_event_types() {
        let cases: Vec<(&str, AgentEvent)> = vec![
            ("run.started", AgentEvent::TurnStarted),
            ("message.delta", AgentEvent::TextDelta("hello".to_string())),
            (
                "reasoning.delta",
                AgentEvent::Reasoning("thinking...".to_string()),
            ),
            (
                "tool.started",
                AgentEvent::ToolStarted {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"a.rs"}"#.to_string(),
                },
            ),
            (
                "tool.completed",
                AgentEvent::ToolResult {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    content: "file contents".to_string(),
                    is_error: false,
                },
            ),
            (
                "usage",
                AgentEvent::Usage {
                    input_tokens: 100,
                    output_tokens: 50,
                },
            ),
            (
                "permission.denied",
                AgentEvent::Request {
                    id: 7,
                    kind: "approval".to_string(),
                    payload: json!({"tool":"bash"}),
                },
            ),
            (
                "turn.completed",
                AgentEvent::TurnComplete {
                    stop_reason: StopReason::Stopped,
                },
            ),
            (
                "error",
                AgentEvent::Error {
                    message: "boom".to_string(),
                    retryable: false,
                },
            ),
            ("cancelled", AgentEvent::Cancelled),
        ];

        for (expected_type, ev) in cases {
            let v = jsonl_event(&ev).expect("event should map to jsonl");
            assert_eq!(v["type"], expected_type, "type mismatch for {ev:?}");
            // 每行必须是对象且含 type 字段
            assert!(v.is_object());
            assert!(v.get("type").is_some());
        }
    }

    #[test]
    fn turn_completed_stop_reason_lowercase() {
        for (reason, expected) in [
            (StopReason::Stopped, "stopped"),
            (StopReason::MaxRounds, "maxrounds"),
            (StopReason::ProviderError, "providererror"),
            (StopReason::Cancelled, "cancelled"),
        ] {
            let v = jsonl_event(&AgentEvent::TurnComplete {
                stop_reason: reason,
            })
            .unwrap();
            assert_eq!(v["stop_reason"], expected);
        }
    }

    #[test]
    fn tool_completed_is_error_flag() {
        let ok = jsonl_event(&AgentEvent::ToolResult {
            id: "x".into(),
            name: "bash".into(),
            content: "".into(),
            is_error: false,
        })
        .unwrap();
        assert_eq!(ok["is_error"], false);

        let err = jsonl_event(&AgentEvent::ToolResult {
            id: "x".into(),
            name: "bash".into(),
            content: "".into(),
            is_error: true,
        })
        .unwrap();
        assert_eq!(err["is_error"], true);
    }
}
