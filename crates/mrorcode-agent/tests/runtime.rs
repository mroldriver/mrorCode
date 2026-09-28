//! Runtime 桥冒烟：submit → 事件序列 →终态。

use std::sync::Arc;
use std::time::Duration;

use mrorcode_agent::{start, DriverCommand, RuntimeEvent, RuntimeStart};
use mrorcode_capabilities::approval::ApprovalMiddleware;
use mrorcode_kernel::event::AgentEvent;
use mrorcode_kernel::middleware::ToolMiddleware;
use mrorcode_kernel::testkit::{text_script, ScriptedProvider};
use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolRegistry, ToolResult};

/// 恒为 Risky 的工具（用于验证审批链是否接通）。
struct RiskyEchoTool;

#[async_trait::async_trait]
impl Tool for RiskyEchoTool {
    fn name(&self) -> &str {
        "risky"
    }

    fn description(&self) -> &str {
        "always risky (test tool)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        RiskLevel::Risky
    }

    async fn execute(&self, _arguments: &str, _ctx: &ToolContext<'_>) -> ToolResult {
        ToolResult::ok("risky", "ran")
    }
}

async fn drain_until_complete(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
) -> Vec<RuntimeEvent> {
    let mut out = Vec::new();
    while let Some(ev) = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("runtime event in time")
    {
        let terminal = matches!(
            ev,
            RuntimeEvent::Agent {
                event: mrorcode_kernel::AgentEvent::TurnComplete { .. },
                ..
            } | RuntimeEvent::Exited
        );
        out.push(ev);
        if terminal {
            break;
        }
    }
    out
}

#[tokio::test]
async fn submit_streams_and_completes() {
    let provider = ScriptedProvider::new(vec![text_script("runtime ok")]);
    let (handle, mut events) = start(RuntimeStart {
        provider: Arc::new(provider),
        tools: ToolRegistry::new(),
        middleware: Vec::new(),
        working_dir: std::env::temp_dir(),
        system_prompt: "system".to_string(),
        model: None,
        compaction: Default::default(),
    });

    handle.submit(7, "hello");
    let out = drain_until_complete(&mut events).await;
    assert!(out.iter().any(|e| matches!(
        e,
        RuntimeEvent::Agent { session_id, event: mrorcode_kernel::AgentEvent::TextDelta(t) }
            if *session_id == 7 && t == "runtime ok"
    )));
    assert!(out.iter().any(|e| matches!(
        e,
        RuntimeEvent::Agent {
            event: mrorcode_kernel::AgentEvent::TurnComplete { .. },
            ..
        }
    )));

    handle.shutdown();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(RuntimeEvent::Exited) = events.recv().await {
                break;
            }
        }
    })
    .await
    .expect("runtime exits after shutdown");
}

#[tokio::test]
async fn cancel_command_is_forwarded() {
    // 用空脚本：provider 立即结束流，验证命令通道无死锁即可
    let provider = ScriptedProvider::new(vec![text_script("x")]);
    let (handle, mut events) = start(RuntimeStart {
        provider: Arc::new(provider),
        tools: ToolRegistry::new(),
        middleware: Vec::new(),
        working_dir: std::env::temp_dir(),
        system_prompt: String::new(),
        model: None,
        compaction: Default::default(),
    });
    handle.submit(1, "hi");
    let _ = drain_until_complete(&mut events).await;
    // 空闲时发 cancel 也不能阻塞/报错
    handle.cancel();
    let _: DriverCommand = DriverCommand::Cancel;
}

/// 回归：`start()` 曾漏挂中间件，导致 Risky 工具完全不弹审批直接执行。
/// 审批链断了，TUI 的审批对话框与 headless 的 `permission.denied` 都是死代码。
#[tokio::test]
async fn middleware_is_wired_into_the_agent() {
    let scripts = vec![
        mrorcode_kernel::testkit::single_tool_call_script("", "c1", "risky", serde_json::json!({})),
        text_script("after"),
    ];
    let mut tools = ToolRegistry::new();
    tools.insert(Arc::new(RiskyEchoTool));
    let middleware: Vec<Arc<dyn ToolMiddleware>> = vec![Arc::new(ApprovalMiddleware::new(
        ApprovalMiddleware::shared_memory(),
    ))];

    let (handle, mut events) = start(RuntimeStart {
        provider: Arc::new(ScriptedProvider::new(scripts)),
        tools,
        middleware,
        working_dir: std::env::temp_dir(),
        system_prompt: "system".to_string(),
        model: None,
        compaction: Default::default(),
    });
    handle.submit(1, "go");
    let mut out = Vec::new();
    let mut approved_id: Option<u64> = None;
    // 审批请求会阻塞 agent，测试里当作 driver 回应一次 deny
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("runtime event in time")
            .expect("channel open");
        let terminal = matches!(
            ev,
            RuntimeEvent::Agent {
                event: AgentEvent::TurnComplete { .. },
                ..
            } | RuntimeEvent::Exited
        );
        if let RuntimeEvent::Agent {
            event: AgentEvent::Request { id, kind, .. },
            ..
        } = &ev
        {
            assert_eq!(kind, "approval");
            approved_id = Some(*id);
            handle.respond(
                *id,
                serde_json::json!({"decision": "deny", "reason": "test"}),
            );
        }
        out.push(ev);
        if terminal {
            break;
        }
    }

    assert!(
        approved_id.is_some(),
        "中间件必须被挂上：Risky 工具应触发审批请求"
    );
    // 拒绝后工具结果为错误，且命令未执行
    let denied_result = out.iter().find_map(|e| match e {
        RuntimeEvent::Agent {
            event: AgentEvent::ToolResult {
                content, is_error, ..
            },
            ..
        } => Some((content.clone(), *is_error)),
        _ => None,
    });
    assert_eq!(
        denied_result,
        Some((r#"{"reason":"test","status":"denied"}"#.to_string(), true)),
        "拒绝后应回灌 denied 结果"
    );
    handle.shutdown();
}
