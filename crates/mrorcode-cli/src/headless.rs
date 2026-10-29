//! headless 一次性任务：普通流式文本 或 版本化 JSONL。
//!
//! M5-d：JSONL schema 在此**冻结**。每行一个 JSON 对象，必含 `v`（schema 版本）与 `type`：
//!
//! | type | 其余字段 |
//! |---|---|
//! | `run.started` | — |
//! | `message.delta` | `delta: string` |
//! | `reasoning.delta` | `delta: string` |
//! | `tool.started` | `id: string`、`tool: string`、`arguments: string`（原始 JSON 文本） |
//! | `tool.completed` | `id: string`、`tool: string`、`is_error: bool`、`content: string` |
//! | `usage` | `input_tokens: u64`、`output_tokens: u64` |
//! | `permission.denied` | `id: u64`、`kind: string`、`payload: any` |
//! | `turn.completed` | `stop_reason: string`（`snake_case`） |
//! | `error` | `retryable: bool`、`message: string` |
//! | `cancelled` | — |
//! | `run.finished` | `success: bool`（恒为最后一行） |
//!
//! 改动规则：**只增字段、不改语义**；不兼容改动必须 `SCHEMA_VERSION += 1` 并同步
//! `crates/mrorcode-cli/tests/headless_json.rs` 的黄金文件与字段级用例。

use std::io::Write;

use serde_json::{json, Value};

use mrorcode_agent::{Assembly, RuntimeEvent};
use mrorcode_kernel::event::{AgentEvent, StopReason};

use crate::runlog::RunLog;

/// headless 使用固定会话 id（无交互，单任务单会话）。
pub const HEADLESS_SESSION: u64 = 1;

/// JSONL schema 版本（不兼容变更时 +1）。
pub const SCHEMA_VERSION: u64 = 1;

/// 输出模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 人类可读流式文本（工具结果截断 20 行）。
    Plain,
    /// 版本化 JSONL。
    Json,
}

impl Mode {
    /// 由 `--json` 开关构造。
    pub fn from_json_flag(json_mode: bool) -> Self {
        if json_mode {
            Mode::Json
        } else {
            Mode::Plain
        }
    }

    fn is_json(self) -> bool {
        self == Mode::Json
    }
}

/// 事件 → JSONL 行（`None` = 该事件在 JSON 模式下不产出任何行）。
/// 纯函数，不做 IO：schema 的唯一事实来源，测试直接对它做字段级断言。
pub fn event_line(ev: &AgentEvent) -> Option<Value> {
    let body = match ev {
        AgentEvent::TurnStarted => json!({ "type": "run.started" }),
        AgentEvent::TextDelta(t) => json!({ "type": "message.delta", "delta": t }),
        AgentEvent::Reasoning(t) => json!({ "type": "reasoning.delta", "delta": t }),
        AgentEvent::ToolStarted {
            id,
            name,
            arguments,
        } => json!({ "type": "tool.started", "id": id, "tool": name, "arguments": arguments }),
        AgentEvent::ToolResult {
            id,
            name,
            content,
            is_error,
        } => json!({
            "type": "tool.completed",
            "id": id,
            "tool": name,
            "is_error": is_error,
            "content": content
        }),
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => json!({
            "type": "usage",
            "input_tokens": input_tokens,
            "output_tokens": output_tokens
        }),
        AgentEvent::Request { id, kind, payload } => json!({
            "type": "permission.denied",
            "id": id,
            "kind": kind,
            "payload": payload
        }),
        AgentEvent::TurnComplete { stop_reason } => json!({
            "type": "turn.completed",
            "stop_reason": stop_reason_str(stop_reason)
        }),
        AgentEvent::Error { message, retryable } => json!({
            "type": "error",
            "retryable": retryable,
            "message": message
        }),
        AgentEvent::Cancelled => json!({ "type": "cancelled" }),
        AgentEvent::SubAgent {
            parent_tool_id,
            event,
        } => {
            // 子 agent 事件递归转换，嵌套在 sub_agent 字段中
            let inner = event_line(event)?;
            json!({
                "type": "sub_agent.event",
                "parent_tool_id": parent_tool_id,
                "event": inner
            })
        }
    };
    Some(with_version(body))
}

/// `stop_reason` 的线缆取值：复用 `StopReason` 的 `snake_case` serde 约定
/// （`max_rounds`，而非 Debug 打印的 `MaxRounds`）。
fn stop_reason_str(sr: &StopReason) -> String {
    serde_json::to_value(sr)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{sr:?}").to_lowercase())
}

/// 收尾行（`run.finished`），恒为最后一行。
pub fn run_finished_line(success: bool) -> Value {
    with_version(json!({ "type": "run.finished", "success": success }))
}

fn with_version(body: Value) -> Value {
    // 放最前，使每行读起来是 `{"v":1,"type":...}`（`v` 之后无空格，diff 友好）
    let mut out = json!({ "v": SCHEMA_VERSION });
    if let Some(map) = out.as_object_mut() {
        if let Some(body_map) = body.as_object() {
            for (k, v) in body_map {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// 运行一个 prompt 直到终态，输出到 stdout，返回进程退出码（0 成功）。
pub async fn run_headless(
    assembly: &mut Assembly,
    prompt: &str,
    json_mode: bool,
    log: Option<&RunLog>,
) -> i32 {
    let mut out = std::io::stdout();
    run_headless_to_logged(
        assembly,
        prompt,
        Mode::from_json_flag(json_mode),
        &mut out,
        log,
    )
    .await
}

/// [`run_headless`] 的可注入 sink 版本（测试用 `&mut Vec<u8>` 收 JSONL）。
pub async fn run_headless_to<W: Write>(
    assembly: &mut Assembly,
    prompt: &str,
    mode: Mode,
    out: &mut W,
) -> i32 {
    run_headless_to_logged(assembly, prompt, mode, out, None).await
}

/// 同 [`run_headless_to`]，额外把事件转录进运行日志。
pub async fn run_headless_to_logged<W: Write>(
    assembly: &mut Assembly,
    prompt: &str,
    mode: Mode,
    out: &mut W,
    log: Option<&RunLog>,
) -> i32 {
    if let Some(log) = log {
        log.user(prompt);
    }
    assembly.handle.submit(HEADLESS_SESSION, prompt.to_string());

    let mut stop_ok = false;
    while let Some(ev) = assembly.events.recv().await {
        match ev {
            RuntimeEvent::Agent { event: ev, .. } => {
                if let Some(success) = handle_agent_event(assembly, ev, mode, out, log) {
                    stop_ok = success;
                    break;
                }
            }
            RuntimeEvent::Exited => break,
        }
    }
    if let Some(log) = log {
        log.flush_assistant();
    }
    if mode.is_json() {
        let _ = writeln!(out, "{}", run_finished_line(stop_ok));
    }
    i32::from(!stop_ok)
}

/// 处理单个事件：None=非终态，Some(success)=终态。
fn handle_agent_event<W: Write>(
    assembly: &Assembly,
    ev: AgentEvent,
    mode: Mode,
    out: &mut W,
    log: Option<&RunLog>,
) -> Option<bool> {
    if let Some(log) = log {
        log.event(&ev);
    }
    if mode.is_json() {
        if let Some(line) = event_line(&ev) {
            let _ = writeln!(out, "{line}");
        }
    } else {
        render_plain(&ev, out);
    }

    // 与输出模式无关的副作用：headless 无人应答，fail-closed 拒绝审批
    // （skip-permissions 的 runtime 根本不会发审批请求）。
    if let AgentEvent::Request { id, .. } = &ev {
        assembly.handle.respond(
            *id,
            json!({"decision":"deny","reason":"headless mode: no interactive approval"}),
        );
    }

    terminal_of(&ev)
}

/// 终态判定：返回 Some(success) 表示整轮结束。
///
/// 只有 `TurnComplete` / `Cancelled` 是终态。`Error` 只是诊断——kernel 在流错误时
/// 先发 `Error` 再发 `TurnComplete{ProviderError}`，因此 `Error` 必须放行，否则 JSONL
/// 会丢掉 `turn.completed` 行。
///
/// 成功仅限 `StopReason::Stopped`（模型正常收尾）；`MaxRounds` / `ProviderError` /
/// `Cancelled` 一律算失败（退出码非 0）。
fn terminal_of(ev: &AgentEvent) -> Option<bool> {
    match ev {
        AgentEvent::TurnComplete { stop_reason } => {
            Some(matches!(stop_reason, StopReason::Stopped))
        }
        AgentEvent::Cancelled => Some(false),
        _ => None,
    }
}

fn render_plain<W: Write>(ev: &AgentEvent, out: &mut W) {
    let w = &mut *out;
    match ev {
        AgentEvent::TurnStarted => {}
        AgentEvent::TextDelta(t) => {
            let _ = write!(w, "{t}");
            let _ = w.flush();
        }
        AgentEvent::Reasoning(_) => {}
        AgentEvent::ToolStarted {
            id: _,
            name,
            arguments,
        } => {
            let preview = one_line_preview(arguments);
            let _ = writeln!(w, "$ {name} {preview}");
        }
        AgentEvent::ToolResult {
            id: _,
            name,
            content,
            is_error,
        } => {
            let mark = if *is_error { "✗" } else { "✓" };
            let _ = writeln!(w, "{mark} {name}");
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                for line in trimmed.lines().take(20) {
                    let _ = writeln!(w, "  {line}");
                }
                if trimmed.lines().count() > 20 {
                    let _ = writeln!(w, "  ...[output truncated]");
                }
            }
        }
        AgentEvent::Usage { .. } => {}
        AgentEvent::Request { .. } => {
            eprintln!(
                "[headless] approval request denied (rerun with --dangerously-skip-permissions to allow)"
            );
        }
        AgentEvent::TurnComplete { .. } => {
            let _ = writeln!(w);
        }
        AgentEvent::Error { message, .. } => {
            eprintln!("error: {message}");
        }
        AgentEvent::Cancelled => {
            eprintln!("cancelled");
        }
        AgentEvent::SubAgent { event, .. } => {
            // 子 agent 事件递归渲染（纯文本模式下缩进显示）
            render_plain(event, out);
        }
    }
}

fn one_line_preview(arguments: &str) -> String {
    let parsed: Value = match serde_json::from_str(arguments) {
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

    /// schema 冻结用例：字段名/类型改动会让本用例失败。
    #[test]
    fn every_event_maps_to_frozen_shape() {
        let cases: Vec<(AgentEvent, Value)> = vec![
            (AgentEvent::TurnStarted, json!({"v":1,"type":"run.started"})),
            (
                AgentEvent::TextDelta("hi".into()),
                json!({"v":1,"type":"message.delta","delta":"hi"}),
            ),
            (
                AgentEvent::Reasoning("why".into()),
                json!({"v":1,"type":"reasoning.delta","delta":"why"}),
            ),
            (
                AgentEvent::ToolStarted {
                    id: "t1".into(),
                    name: "bash".into(),
                    arguments: r#"{"command":"ls"}"#.into(),
                },
                json!({
                    "v":1,"type":"tool.started","id":"t1","tool":"bash",
                    "arguments":r#"{"command":"ls"}"#
                }),
            ),
            (
                AgentEvent::ToolResult {
                    id: "t1".into(),
                    name: "bash".into(),
                    content: "a\nb".into(),
                    is_error: false,
                },
                json!({
                    "v":1,"type":"tool.completed","id":"t1","tool":"bash",
                    "is_error":false,"content":"a\nb"
                }),
            ),
            (
                AgentEvent::Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                json!({"v":1,"type":"usage","input_tokens":10,"output_tokens":5}),
            ),
            (
                AgentEvent::Request {
                    id: 7,
                    kind: "approval".into(),
                    payload: json!({"tool":"bash"}),
                },
                json!({
                    "v":1,"type":"permission.denied","id":7,"kind":"approval",
                    "payload":{"tool":"bash"}
                }),
            ),
            (
                AgentEvent::TurnComplete {
                    stop_reason: StopReason::Stopped,
                },
                json!({"v":1,"type":"turn.completed","stop_reason":"stopped"}),
            ),
            (
                AgentEvent::TurnComplete {
                    stop_reason: StopReason::MaxRounds,
                },
                json!({"v":1,"type":"turn.completed","stop_reason":"max_rounds"}),
            ),
            (
                AgentEvent::Error {
                    message: "boom".into(),
                    retryable: false,
                },
                json!({"v":1,"type":"error","retryable":false,"message":"boom"}),
            ),
            (AgentEvent::Cancelled, json!({"v":1,"type":"cancelled"})),
        ];
        for (ev, expected) in cases {
            assert_eq!(event_line(&ev).as_ref(), Some(&expected), "event: {ev:?}");
        }
    }

    /// 所有事件都必须能落一行，且都带 `v` 与 `type`。
    #[test]
    fn all_lines_carry_version_and_type() {
        let events = [
            AgentEvent::TurnStarted,
            AgentEvent::TextDelta("a".into()),
            AgentEvent::Reasoning("b".into()),
            AgentEvent::ToolStarted {
                id: "i".into(),
                name: "n".into(),
                arguments: "{}".into(),
            },
            AgentEvent::ToolResult {
                id: "i".into(),
                name: "n".into(),
                content: "c".into(),
                is_error: true,
            },
            AgentEvent::Usage {
                input_tokens: 1,
                output_tokens: 2,
            },
            AgentEvent::Request {
                id: 1,
                kind: "k".into(),
                payload: Value::Null,
            },
            AgentEvent::TurnComplete {
                stop_reason: StopReason::Stopped,
            },
            AgentEvent::Error {
                message: "m".into(),
                retryable: true,
            },
            AgentEvent::Cancelled,
        ];
        let mut types = Vec::new();
        for ev in events {
            let line = event_line(&ev).expect("event must map to a line");
            let map = line.as_object().expect("line must be an object");
            assert_eq!(map["v"], json!(SCHEMA_VERSION), "missing version: {ev:?}");
            let ty = map["type"].as_str().expect("type must be a string");
            types.push(ty.to_string());
        }
        // 每个事件类型唯一（防止复制粘贴漏改某个分支）
        let mut sorted = types.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            types.len(),
            "duplicate event types: {types:?}"
        );
        let fin = run_finished_line(true);
        assert_eq!(fin["v"], json!(SCHEMA_VERSION));
        assert_eq!(fin["type"], "run.finished");
    }

    #[test]
    fn run_finished_reflects_success() {
        assert_eq!(
            run_finished_line(true),
            json!({"v":1,"type":"run.finished","success":true})
        );
        assert_eq!(
            run_finished_line(false),
            json!({"v":1,"type":"run.finished","success":false})
        );
    }

    #[test]
    fn terminal_classification() {
        assert_eq!(terminal_of(&AgentEvent::TurnStarted), None);
        assert_eq!(terminal_of(&AgentEvent::TextDelta("x".into())), None);
        // 诊断事件必须放行：kernel 随后还会发 TurnComplete{ProviderError}
        assert_eq!(
            terminal_of(&AgentEvent::Error {
                message: "m".into(),
                retryable: false
            }),
            None
        );
        assert_eq!(
            terminal_of(&AgentEvent::Error {
                message: "m".into(),
                retryable: true
            }),
            None
        );
        // 仅正常收尾算成功
        assert_eq!(
            terminal_of(&AgentEvent::TurnComplete {
                stop_reason: StopReason::Stopped
            }),
            Some(true)
        );
        assert_eq!(
            terminal_of(&AgentEvent::TurnComplete {
                stop_reason: StopReason::ProviderError
            }),
            Some(false)
        );
        assert_eq!(
            terminal_of(&AgentEvent::TurnComplete {
                stop_reason: StopReason::MaxRounds
            }),
            Some(false)
        );
        assert_eq!(terminal_of(&AgentEvent::Cancelled), Some(false));
    }

    #[test]
    fn stop_reason_uses_serde_snake_case() {
        assert_eq!(stop_reason_str(&StopReason::Stopped), "stopped");
        assert_eq!(stop_reason_str(&StopReason::MaxRounds), "max_rounds");
        assert_eq!(
            stop_reason_str(&StopReason::ProviderError),
            "provider_error"
        );
        assert_eq!(stop_reason_str(&StopReason::Cancelled), "cancelled");
    }

    #[test]
    fn plain_mode_renders_tools_and_truncates() {
        let evs = [
            AgentEvent::TurnStarted,
            AgentEvent::TextDelta("hello".into()),
            AgentEvent::ToolStarted {
                id: "t1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"ls -la"}"#.into(),
            },
            AgentEvent::ToolResult {
                id: "t1".into(),
                name: "bash".into(),
                content: (0..30).map(|i| format!("l{i}\n")).collect(),
                is_error: false,
            },
            AgentEvent::TurnComplete {
                stop_reason: StopReason::Stopped,
            },
        ];
        let mut buf: Vec<u8> = Vec::new();
        for ev in &evs {
            render_plain(ev, &mut buf);
        }
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("hello"));
        assert!(s.contains("$ bash ls -la"));
        assert!(s.contains("✓ bash"));
        assert!(s.contains("  l0"));
        assert!(s.contains("...[output truncated]"));
        // 20 行上限
        assert!(!s.contains("  l20\n"));
    }

    #[test]
    fn mode_from_flag() {
        assert_eq!(Mode::from_json_flag(true), Mode::Json);
        assert_eq!(Mode::from_json_flag(false), Mode::Plain);
        assert!(Mode::Json.is_json());
        assert!(!Mode::Plain.is_json());
    }
}
