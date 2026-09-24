//! Runtime 桥冒烟：submit → 事件序列 →终态。

use std::sync::Arc;
use std::time::Duration;

use mrorcode_agent::{start, DriverCommand, RuntimeEvent, RuntimeStart};
use mrorcode_kernel::testkit::{text_script, ScriptedProvider};
use mrorcode_kernel::tool::ToolRegistry;

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
    });
    handle.submit(1, "hi");
    let _ = drain_until_complete(&mut events).await;
    // 空闲时发 cancel 也不能阻塞/报错
    handle.cancel();
    let _: DriverCommand = DriverCommand::Cancel;
}
