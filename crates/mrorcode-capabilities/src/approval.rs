//! 审批中间件：Safe 放行 → 已授权 grant 放行 → 否则弹请求，
//! Null/超时/cancel 一律 fail-closed 拒绝。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use mrorcode_kernel::message::ToolCall;
use mrorcode_kernel::middleware::{BeforeOutcome, ToolMiddleware};
use mrorcode_kernel::request::{Requester, DEFAULT_REQUEST_TIMEOUT};
use mrorcode_kernel::tool::{RiskLevel, Tool};

/// 持久/可替换的授权存储。
pub trait PermissionStore: Send + Sync {
    fn is_granted(&self, key: &str) -> bool;
    fn grant(&self, key: &str);
}

/// 进程内内存授权（M1）。
#[derive(Default)]
pub struct InMemoryPermissionStore {
    granted: Mutex<HashSet<String>>,
}

impl PermissionStore for InMemoryPermissionStore {
    fn is_granted(&self, key: &str) -> bool {
        self.granted.lock().unwrap().contains(key)
    }

    fn grant(&self, key: &str) {
        self.granted.lock().unwrap().insert(key.to_string());
    }
}

/// 审批中间件。
pub struct ApprovalMiddleware {
    store: Arc<dyn PermissionStore>,
}

impl ApprovalMiddleware {
    pub fn new(store: Arc<dyn PermissionStore>) -> Self {
        Self { store }
    }

    pub fn shared_memory() -> Arc<InMemoryPermissionStore> {
        Arc::new(InMemoryPermissionStore::default())
    }
}

/// 授权键：bash 按规范化命令，其余按工具名（工具级）。
pub fn grant_key(tool_name: &str, arguments: &str) -> String {
    if tool_name == "bash" {
        let command = serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|v| {
                v.get("command")
                    .and_then(|c| c.as_str().map(str::to_string))
            })
            .unwrap_or_default();
        format!("bash::{}", command.trim())
    } else {
        tool_name.to_string()
    }
}

#[async_trait]
impl ToolMiddleware for ApprovalMiddleware {
    async fn before(
        &self,
        call: &mut ToolCall,
        risk: RiskLevel,
        tool: &Arc<dyn Tool>,
        request: &Requester,
        cancel: &CancellationToken,
    ) -> BeforeOutcome {
        if risk == RiskLevel::Safe {
            return BeforeOutcome::Proceed;
        }

        let key = grant_key(&call.name, &call.arguments);
        if self.store.is_granted(&key) {
            return BeforeOutcome::Allow {
                reason: "remembered approval".to_string(),
            };
        }

        let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        let payload = json!({
            "tool": tool.name(),
            "risk": "risky",
            "arguments": arguments,
        });

        let response = request
            .request_cancellable("approval", payload, DEFAULT_REQUEST_TIMEOUT, cancel)
            .await;

        let decision = response
            .get("decision")
            .and_then(|v| v.as_str())
            .unwrap_or("deny");
        let reason = response
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("permission denied")
            .to_string();

        match decision {
            "allow_always" | "always" => {
                self.store.grant(&key);
                BeforeOutcome::Allow { reason }
            }
            "allow" | "once" => BeforeOutcome::Proceed,
            _ => BeforeOutcome::Deny { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mrorcode_kernel::event::AgentEvent;
    use mrorcode_kernel::request::RequestBroker;
    use std::sync::Arc;

    fn broker_with_auto_reply(reply: Value) -> Requester {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let broker = RequestBroker::new(tx);
        let requester = broker.handle();
        // 独立任务等待请求并应答；broker 被 move 进去，requester 经 Arc 共享后端
        tokio::spawn(async move {
            let mut rx = rx;
            while let Some(ev) = rx.recv().await {
                if let AgentEvent::Request { id, .. } = ev {
                    broker.resolve(id, reply.clone());
                }
            }
        });
        requester
    }

    fn bash_call(command: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            arguments: json!({"command": command}).to_string(),
        }
    }

    fn echo_tool() -> Arc<dyn Tool> {
        Arc::new(crate::tools::BashTool)
    }

    #[tokio::test]
    async fn safe_commands_pass_without_request() {
        // BashTool 对 ls 判 Safe，不应发出任何 Request
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let broker = RequestBroker::new(tx);
        let mw = ApprovalMiddleware::new(Arc::new(InMemoryPermissionStore::default()));
        let mut call = bash_call("ls");
        let outcome = mw
            .before(
                &mut call,
                RiskLevel::Safe,
                &echo_tool(),
                &broker.handle(),
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(outcome, BeforeOutcome::Proceed));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn risky_allow_once_does_not_grant() {
        let requester = broker_with_auto_reply(json!({"decision":"allow"}));
        let store = ApprovalMiddleware::shared_memory();
        let mw = ApprovalMiddleware::new(store.clone());
        let mut call = bash_call("rm x");
        let outcome = mw
            .before(
                &mut call,
                RiskLevel::Risky,
                &echo_tool(),
                &requester,
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(outcome, BeforeOutcome::Proceed));
        assert!(!store.is_granted("bash::rm x"));
    }

    #[tokio::test]
    async fn risky_allow_always_grants_and_second_call_is_silent() {
        let requester = broker_with_auto_reply(json!({"decision":"allow_always"}));
        let store = ApprovalMiddleware::shared_memory();
        let mw = ApprovalMiddleware::new(store.clone());
        let tool = echo_tool();
        let cancel = CancellationToken::new();

        let mut call = bash_call("rm x");
        let first = mw
            .before(&mut call, RiskLevel::Risky, &tool, &requester, &cancel)
            .await;
        assert!(matches!(first, BeforeOutcome::Allow { .. }));
        assert!(store.is_granted("bash::rm x"));

        // 第二次：直接命中授权，无需应答者也能通过
        let mut call2 = bash_call("rm x");
        let second = mw
            .before(&mut call2, RiskLevel::Risky, &tool, &requester, &cancel)
            .await;
        assert!(matches!(second, BeforeOutcome::Allow { .. }));
    }

    #[tokio::test]
    async fn deny_and_null_are_fail_closed() {
        let requester = broker_with_auto_reply(json!({"decision":"deny","reason":"nope"}));
        let mw = ApprovalMiddleware::new(Arc::new(InMemoryPermissionStore::default()));
        let mut call = bash_call("rm x");
        let outcome = mw
            .before(
                &mut call,
                RiskLevel::Risky,
                &echo_tool(),
                &requester,
                &CancellationToken::new(),
            )
            .await;
        match outcome {
            BeforeOutcome::Deny { reason } => assert_eq!(reason, "nope"),
            other => panic!("expected deny, got {other:?}"),
        }

        // cancel/null 同样拒绝
        let (tx, _rx2) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let broker = RequestBroker::new(tx);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut call = bash_call("rm y");
        let outcome = mw
            .before(
                &mut call,
                RiskLevel::Risky,
                &echo_tool(),
                &broker.handle(),
                &cancel,
            )
            .await;
        assert!(matches!(outcome, BeforeOutcome::Deny { .. }));
    }

    #[test]
    fn grant_keys_are_command_scoped_for_bash() {
        let k1 = grant_key("bash", r#"{"command":"rm x"}"#);
        assert_eq!(k1, "bash::rm x");
        assert_eq!(grant_key("read", "{}"), "read");
    }
}
