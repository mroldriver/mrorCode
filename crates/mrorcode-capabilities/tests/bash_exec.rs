//! bash 工具真实子进程冒烟（跨平台 shell 分支）。

use mrorcode_capabilities::tools::BashTool;
use mrorcode_kernel::request::RequestBroker;
use mrorcode_kernel::tool::{RiskLevel, Tool};
use mrorcode_kernel::Requester;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn executes_platform_shell_echo() {
    let dir = std::env::temp_dir();
    let command = if cfg!(windows) {
        "echo hello-mror"
    } else {
        "printf '%s' hello-mror"
    };

    // 风险分类：echo 为 Safe
    let risk = BashTool.risk(&serde_json::json!({"command": command}));
    assert_eq!(risk, RiskLevel::Safe);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let requester: Requester = RequestBroker::new(tx).handle();
    let cancel = CancellationToken::new();
    let ctx = mrorcode_kernel::tool::ToolContext {
        working_dir: &dir,
        cancel: &cancel,
        request: &requester,
        tool_id: "",
        event_sink: None,
    };

    let result = BashTool
        .execute(&serde_json::json!({"command": command}).to_string(), &ctx)
        .await;
    assert!(!result.is_error, "output: {}", result.content);
    assert!(result.content.contains("hello-mror"));
}

#[tokio::test]
async fn failing_command_returns_error_result() {
    let dir = std::env::temp_dir();
    let command = "exit 3";
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let requester = RequestBroker::new(tx).handle();
    let cancel = CancellationToken::new();
    let ctx = mrorcode_kernel::tool::ToolContext {
        working_dir: &dir,
        cancel: &cancel,
        request: &requester,
        tool_id: "",
        event_sink: None,
    };
    let result = BashTool
        .execute(&serde_json::json!({"command": command}).to_string(), &ctx)
        .await;
    assert!(result.is_error);
    // 失败时 content 只保留纯输出（此处为空），退出码不再写入文本。
    assert!(result.content.trim().is_empty());
}
