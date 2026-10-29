//! bash 工具真实子进程冒烟（跨平台 shell 分支）。
//!
//! 「真跑一次 shell」这一步依赖运行环境放行：MSYS2/Cygwin 启动时要在 `\BaseNamedObjects`
//! 里创建命名共享内存对象，受限 token、企业沙箱、部分 CI 容器会以 `0xC0000022`
//! （`STATUS_ACCESS_DENIED`）拒掉，bash 在跑命令之前就 abort。那种情况下按环境不可用跳过，
//! 见 [`MSYS_UNAVAILABLE`]。

use mrorcode_capabilities::tools::BashTool;
use mrorcode_kernel::request::RequestBroker;
use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext};
use mrorcode_kernel::Requester;
use tokio_util::sync::CancellationToken;

/// 环境拒绝启动 MSYS 程序的签名。
///
/// 命中它说明**子进程还没跑到命令**就死了，与工具逻辑无关：`build_shell_command` 只是
/// `Command::new(shell).args(["-c", cmd])`，换任何 Rust 程序来起都一样。实测同一份
/// `bash.exe` 由已有 MSYS shell 或 powershell 拉起就正常，因为那条链路上共享内存的
/// section 句柄是**继承**过去的（继承句柄不重做 ACL 检查）。
const MSYS_UNAVAILABLE: &str = "fatal error - NtCreateDirectoryObject";

/// 用工具在临时目录里跑一条命令。
async fn execute(command: &str) -> mrorcode_kernel::tool::ToolResult {
    let dir = std::env::temp_dir();
    let cancel = CancellationToken::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let requester: Requester = RequestBroker::new(tx).handle();
    let ctx = ToolContext {
        working_dir: &dir,
        cancel: &cancel,
        request: &requester,
        tool_id: "",
        event_sink: None,
    };
    BashTool
        .execute(&serde_json::json!({ "command": command }).to_string(), &ctx)
        .await
}

#[tokio::test]
async fn executes_platform_shell_echo() {
    let command = if cfg!(windows) {
        "echo hello-mror"
    } else {
        "printf '%s' hello-mror"
    };

    // 与环境无关的硬断言：echo 属于只读安全命令，必须判为 Safe。
    let risk = BashTool.risk(&serde_json::json!({ "command": command }));
    assert_eq!(risk, RiskLevel::Safe);

    let result = execute(command).await;

    // 唯一的跳过口：**只有**命中 MSYS 初始化签名才认为环境不放行。其它任何错误照常失败，
    // 免得把真 bug 一起吞掉。
    if result.content.contains(MSYS_UNAVAILABLE) {
        eprintln!(
            "skip executes_platform_shell_echo：本环境不允许从原生进程启动 MSYS bash\
             （NtCreateDirectoryObject 被拒 0xC0000022）；工具逻辑由 tools::bash 的注入式单测覆盖"
        );
        return;
    }

    assert!(!result.is_error, "output: {}", result.content);
    assert!(result.content.contains("hello-mror"));
}

#[tokio::test]
async fn failing_command_returns_error_result() {
    let result = execute("exit 3").await;

    // 失败结果必须回显实际使用的 shell（issue #5），这条与环境是否放行 MSYS 无关：
    // shell 起不来时同样是 is_error，且首行照样是 `[shell: ...]`。
    assert!(result.is_error);
    let first_line = result.content.lines().next().unwrap_or_default();
    assert!(
        first_line.starts_with("[shell: ") && first_line.ends_with(']'),
        "content: {:?}",
        result.content
    );
}
