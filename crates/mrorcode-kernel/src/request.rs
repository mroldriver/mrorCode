//! 运行时请求/应答机制（审批等）。
//!
//! 请求通过 [`crate::event::AgentEvent::Request`] 事件发给 driver，
//! driver 用 [`crate::event::AgentCommand::Respond`] 回应。
//! oneshot 通道不进事件体，保持事件可序列化；超时/dropped 一律返回 `Null`（fail-closed）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::event::AgentEvent;

pub type RequestId = u64;

/// 等待审批的默认超时（交互场景 5 分钟）。
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

struct BrokerInner {
    next_id: AtomicU64,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Value>>>,
    events: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
}

/// 驱动侧应答入口 + 工具/中间件发请求句柄的共享后端。
pub struct RequestBroker {
    inner: Arc<BrokerInner>,
}

/// 给工具/中间件用的瘦句柄（廉价 Clone）。
#[derive(Clone)]
pub struct Requester {
    inner: Arc<BrokerInner>,
}

impl RequestBroker {
    pub fn new(events: tokio::sync::mpsc::UnboundedSender<AgentEvent>) -> Self {
        Self {
            inner: Arc::new(BrokerInner {
                next_id: AtomicU64::new(1),
                pending: Mutex::new(HashMap::new()),
                events,
            }),
        }
    }

    pub fn handle(&self) -> Requester {
        Requester {
            inner: Arc::clone(&self.inner),
        }
    }

    /// 登记一个待应答请求并发出事件。返回接收端（由 owner task 等待）。
    pub fn emit(&self, kind: &str, payload: Value) -> oneshot::Receiver<Value> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        // 事件通道若已关闭（agent 退出），登记立即作废，等待侧拿到 Null
        let _ = self
            .inner
            .events
            .send(AgentEvent::Request { id, kind: kind.to_string(), payload });
        rx
    }

    /// 解析一个应答。未知/重复 id 返回 false。
    pub fn resolve(&self, id: RequestId, value: Value) -> bool {
        match self.inner.pending.lock().unwrap().remove(&id) {
            Some(tx) => tx.send(value).is_ok(),
            None => false,
        }
    }

    /// 取消/关闭：所有未决请求以 Null 解套（fail-closed）。
    pub fn fail_all(&self) {
        let mut pending = self.inner.pending.lock().unwrap();
        for (_, tx) in pending.drain() {
            let _ = tx.send(Value::Null);
        }
    }
}

impl Requester {
    /// 发出请求并等待应答。超时或通道失效一律返回 `Value::Null`。
    pub async fn request(&self, kind: &str, payload: Value) -> Value {
        self.request_with_timeout(kind, payload, DEFAULT_REQUEST_TIMEOUT)
            .await
    }

    pub async fn request_with_timeout(
        &self,
        kind: &str,
        payload: Value,
        timeout: Duration,
    ) -> Value {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        if self
            .inner
            .events
            .send(AgentEvent::Request { id, kind: kind.to_string(), payload })
            .is_err()
        {
            self.inner.pending.lock().unwrap().remove(&id);
            return Value::Null;
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => Value::Null,
            Err(_) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Value::Null
            }
        }
    }

    /// 可取消的请求（cancel 触发时同样返回 Null）。
    pub async fn request_cancellable(
        &self,
        kind: &str,
        payload: Value,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Value {
        tokio::select! {
            v = self.request_with_timeout(kind, payload, timeout) => v,
            _ = cancel.cancelled() => Value::Null,
        }
    }
}
