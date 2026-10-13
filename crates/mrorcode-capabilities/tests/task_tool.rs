//! TaskTool 集成测试（M4-10）。
//!
//! TaskTool 内部用 kernel AgentBuilder 起一个只读子 agent，共享同一个 provider 实例。
//! 这里用 ScriptedProvider 模拟子 agent 的 chat_stream，验证：
//! - 子 agent 最终文本被回灌为工具结果；
//! - 缺 prompt 直接报错，不发请求。

use std::sync::Arc;

use mrorcode_capabilities::tools::TaskTool;
use mrorcode_kernel::testkit::{text_script, ScriptedProvider};
use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolResult};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join("mrorcode-task-tool-tests")
        .join(tag)
}

/// 构造只读注册表（与 assemble.rs 一致）并实例化 TaskTool。
fn make_tool(provider: Arc<dyn mrorcode_kernel::LlmProvider>) -> TaskTool {
    let registry = mrorcode_capabilities::tools::read_only_registry();
    TaskTool::new(provider, registry, "test sub persona".to_string(), None)
}

/// 构造一个带有效 Requester 的 ToolContext（与 kernel 内部一致）。
fn ctx<'a>(
    working_dir: &'a std::path::Path,
    cancel: &'a CancellationToken,
    request: &'a mrorcode_kernel::Requester,
) -> ToolContext<'a> {
    ToolContext {
        working_dir,
        cancel,
        request,
        tool_id: "",
        event_sink: None,
    }
}

fn new_requester() -> mrorcode_kernel::Requester {
    let (tx, _rx) = mpsc::unbounded_channel();
    mrorcode_kernel::RequestBroker::new(tx).handle()
}

fn run_tool(tool: &TaskTool, args: &str) -> ToolResult {
    let dir = temp_dir("run");
    let cancel = CancellationToken::new();
    let requester = new_requester();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    rt.block_on(async {
        let ctx = ctx(&dir, &cancel, &requester);
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tool.execute(args, &ctx).await
        })
        .await
        .expect("sub-agent timed out")
    })
}

#[test]
fn task_returns_sub_agent_final_text() {
    let provider = Arc::new(ScriptedProvider::new(vec![text_script("sub-agent answer")]));
    let tool = make_tool(provider.clone());
    let result = run_tool(&tool, r#"{"prompt": "find the answer"}"#);
    assert!(!result.is_error, "expected ok, got: {}", result.content);
    assert_eq!(result.content.trim(), "sub-agent answer");
    assert_eq!(provider.call_count(), 1, "只应有一次子 agent 请求");
}

#[test]
fn task_requires_prompt() {
    let provider = Arc::new(ScriptedProvider::new(vec![]));
    let tool = make_tool(provider.clone());
    let result = run_tool(&tool, r#"{}"#);
    assert!(result.is_error, "缺 prompt 应报错");
    assert_eq!(provider.call_count(), 0, "未发起子 agent 请求");
}

#[test]
fn task_reports_empty_answer_as_error() {
    // 脚本给空白文本：子 agent 无有效回答 → 应视为错误（不是静默成功）
    let provider = Arc::new(ScriptedProvider::new(vec![text_script("   ")]));
    let tool = make_tool(provider);
    let result = run_tool(&tool, r#"{"prompt": "explain nothing"}"#);
    assert!(result.is_error, "空白回答应报错");
}

#[test]
fn task_risk_is_risky() {
    let tool = make_tool(Arc::new(ScriptedProvider::new(vec![])));
    assert_eq!(tool.risk(&serde_json::Value::Null), RiskLevel::Risky);
}
