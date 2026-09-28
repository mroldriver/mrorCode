//! M5-d：headless `--json` 事件流的端到端冻结。
//!
//! - `jsonl_matches_golden`：真实 runtime（ScriptedProvider + EchoTool）跑完整一轮，
//!   逐行对比 `tests/golden/headless-json.txt`。改了 schema 或事件顺序就会红。
//!   重新生成：`UPDATE_GOLDEN=1 cargo test -p mrorcode-cli --test headless_json`。
//! - 其余用例覆盖黄金文件覆盖不到的分支（审批拒绝、流错误、取消）。
//!
//! 只用「无副作用」的工具（EchoTool），避免平台相关的输出进入黄金文件。

use std::sync::Arc;

use mrorcode_agent::{start, Assembly, RuntimeStart};
use mrorcode_capabilities::approval::ApprovalMiddleware;
use mrorcode_capabilities::tools::BashTool;
use mrorcode_cli::headless::{run_headless_to, Mode, SCHEMA_VERSION};
use mrorcode_kernel::agent::CompactionPolicy;
use mrorcode_kernel::middleware::ToolMiddleware;
use mrorcode_kernel::provider::StreamEvent;
use mrorcode_kernel::testkit::{EchoTool, ScriptedProvider};
use mrorcode_kernel::tool::ToolRegistry;
use serde_json::{json, Value};

const GOLDEN: &str = include_str!("golden/headless-json.txt");

/// 组一个用脚本 provider 驱动的 runtime；`tools` 由调用方决定。
fn assembly_with(
    scripts: Vec<Vec<StreamEvent>>,
    tools: ToolRegistry,
    middleware: Vec<Arc<dyn ToolMiddleware>>,
) -> Assembly {
    let provider = Arc::new(ScriptedProvider::new(scripts).with_model("scripted-model"));
    let (handle, events) = start(RuntimeStart {
        provider,
        tools,
        middleware,
        working_dir: std::env::temp_dir(),
        system_prompt: "You are a test fixture. Always call the echo tool once.".to_string(),
        model: None,
        compaction: CompactionPolicy::default(),
    });
    Assembly {
        handle,
        events,
        model: "scripted-model".to_string(),
        available_models: vec!["scripted-model".to_string()],
        mcp: None,
        warnings: Vec::new(),
    }
}

fn echo_registry() -> ToolRegistry {
    let mut r = ToolRegistry::default();
    r.insert(Arc::new(EchoTool));
    r
}

fn bash_registry() -> ToolRegistry {
    let mut r = ToolRegistry::default();
    r.insert(Arc::new(BashTool));
    r
}

/// 解析 JSONL 为行数组，顺带验证每行都是带 `v` 的对象。
fn parse_lines(raw: &str) -> Vec<Value> {
    let lines: Vec<Value> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap_or_else(|e| panic!("bad JSONL line {l:?}: {e}"))
        })
        .collect();
    for v in &lines {
        assert_eq!(v["v"], json!(SCHEMA_VERSION), "line missing version: {v}");
        assert!(v["type"].is_string(), "line missing type: {v}");
    }
    lines
}

fn types(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .map(|v| v["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn jsonl_matches_golden() {
    // 第 1 轮：文本 + 推理 + 一次工具调用；第 2 轮：收尾文本
    let scripts = vec![
        vec![
            StreamEvent::TextDelta("Hel".into()),
            StreamEvent::Reasoning("先想".into()),
            StreamEvent::TextDelta("lo".into()),
            StreamEvent::Usage {
                input_tokens: 12,
                output_tokens: 7,
            },
            StreamEvent::ToolCallStarted {
                id: "call_1".into(),
                name: "echo".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".into(),
                arguments: r#"{"text":"ping"}"#.into(),
            },
        ],
        vec![
            StreamEvent::TextDelta("done".into()),
            StreamEvent::Usage {
                input_tokens: 20,
                output_tokens: 1,
            },
        ],
    ];
    let mut asm = assembly_with(scripts, echo_registry(), Vec::new());
    let mut buf: Vec<u8> = Vec::new();
    let code = run_headless_to(&mut asm, "say hi", Mode::Json, &mut buf).await;
    asm.handle.shutdown();

    let raw = String::from_utf8(buf).expect("JSONL must be utf-8");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/headless-json.txt");
        std::fs::write(&path, &raw).expect("write golden");
        eprintln!("golden updated: {}", path.display());
    }

    let lines = parse_lines(&raw);
    // `run.started` 是「每轮 turn 一行」，工具后的追加 round 不再发一次
    assert_eq!(
        types(&lines),
        vec![
            "run.started",
            "message.delta",
            "reasoning.delta",
            "message.delta",
            "usage",
            "tool.started",
            "tool.completed",
            "message.delta",
            "usage",
            "turn.completed",
            "run.finished",
        ],
        "event sequence drifted: {raw}"
    );
    assert_eq!(code, 0, "正常收尾应退出码 0");
    // 黄金文件逐字节比对（顺序 + 字段 + 取值全冻结）
    assert_eq!(
        raw, GOLDEN,
        "JSONL 与黄金文件不一致（改 schema 请更新 golden 并写理由）"
    );

    // 关键取值抽样（比黄金文件更可读的断言）
    let tool_done = lines
        .iter()
        .find(|l| l["type"] == "tool.completed")
        .unwrap();
    assert_eq!(tool_done["id"], "call_1");
    assert_eq!(tool_done["tool"], "echo");
    assert_eq!(tool_done["is_error"], json!(false));
    assert_eq!(tool_done["content"], r#"{"text":"ping"}"#);
    let finished = lines.last().unwrap();
    assert_eq!(finished["type"], "run.finished");
    assert_eq!(finished["success"], json!(true));
}

#[tokio::test]
async fn risky_tool_is_denied_fail_closed_without_tool_started() {
    // 审批中间件在位：headless 无人应答 → permission.denied + 拒绝结果，且不发 tool.started
    let scripts = vec![
        vec![
            StreamEvent::ToolCallStarted {
                id: "c1".into(),
                name: "bash".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "c1".into(),
                arguments: r#"{"command":"rm -rf /tmp/nope"}"#.into(),
            },
        ],
        vec![StreamEvent::TextDelta("cannot".into())],
    ];
    let middleware: Vec<Arc<dyn ToolMiddleware>> = vec![Arc::new(ApprovalMiddleware::new(
        ApprovalMiddleware::shared_memory(),
    ))];
    let mut asm = assembly_with(scripts, bash_registry(), middleware);
    let mut buf: Vec<u8> = Vec::new();
    let _code = run_headless_to(&mut asm, "delete it", Mode::Json, &mut buf).await;
    asm.handle.shutdown();

    let raw = String::from_utf8(buf).unwrap();
    let lines = parse_lines(&raw);
    // 拒绝后仍发 tool.started（前端要画出这张工具卡），但结果 is_error 且命令未执行
    assert_eq!(
        types(&lines),
        vec![
            "run.started",
            "permission.denied",
            "tool.started",
            "tool.completed",
            "message.delta",
            "turn.completed",
            "run.finished",
        ],
        "deny 分支事件序列漂移：{raw}"
    );
    let denied = lines
        .iter()
        .find(|l| l["type"] == "permission.denied")
        .unwrap();
    assert_eq!(denied["kind"], "approval");
    assert_eq!(denied["payload"]["tool"], "bash");
    let done = lines
        .iter()
        .find(|l| l["type"] == "tool.completed")
        .unwrap();
    assert_eq!(done["is_error"], json!(true));
    assert_eq!(
        done["content"],
        r#"{"reason":"headless mode: no interactive approval","status":"denied"}"#
    );
}

#[tokio::test]
async fn provider_error_yields_error_then_turn_completed_and_nonzero_exit() {
    // 回归：Error 只是诊断，kernel 随后发 TurnComplete{ProviderError}。
    // 早期实现把 Error 当终态，导致缺 turn.completed 行且（retryable=false 时）错误地退出 0。
    let scripts = vec![vec![StreamEvent::Error(
        mrorcode_kernel::provider::ProviderError {
            message: "boom".into(),
            retryable: false,
            http_status: None,
            code: None,
        },
    )]];
    let mut asm = assembly_with(scripts, echo_registry(), Vec::new());
    let mut buf: Vec<u8> = Vec::new();
    let code = run_headless_to(&mut asm, "fail", Mode::Json, &mut buf).await;
    asm.handle.shutdown();

    let raw = String::from_utf8(buf).unwrap();
    let lines = parse_lines(&raw);
    assert_eq!(
        types(&lines),
        vec!["run.started", "error", "turn.completed", "run.finished",],
        "provider 错误分支事件序列漂移：{raw}"
    );
    assert_eq!(lines[1]["retryable"], json!(false));
    assert_eq!(lines[1]["message"], "boom");
    assert_eq!(lines[2]["stop_reason"], "provider_error");
    assert_eq!(lines[3]["success"], json!(false));
    assert_eq!(code, 1, "provider 致命错误必须非 0 退出");
}

#[tokio::test]
async fn plain_mode_has_no_json_lines() {
    let scripts = vec![vec![StreamEvent::TextDelta("plain text".into())]];
    let mut asm = assembly_with(scripts, echo_registry(), Vec::new());
    let mut buf: Vec<u8> = Vec::new();
    let code = run_headless_to(&mut asm, "hi", Mode::Plain, &mut buf).await;
    asm.handle.shutdown();
    let raw = String::from_utf8(buf).unwrap();
    assert!(!raw.contains("\"type\""), "plain 模式不得输出 JSONL：{raw}");
    assert!(raw.contains("plain text"));
    assert_eq!(code, 0);
}
