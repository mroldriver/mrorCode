//! 工具测试的共享脚手架。
//!
//! 每个工具的 `mod tests` 都需要两样东西：一个「不会真发请求」的 `Requester`，和一个最小的
//! `ToolContext`。这两样以前在 read / write / edit / grep / glob 里各抄一份（issue #13），
//! 改一处要改五处，参数列表也开始各自漂移。统一放这里，由 `mod.rs` 用 `#[cfg(test)]` 挂载，
//! 不进产物。

use mrorcode_kernel::request::RequestBroker;
use mrorcode_kernel::tool::ToolContext;
use mrorcode_kernel::Requester;

/// 指向已关闭通道的请求句柄。
///
/// 单测里没有审批中间件，工具就算真的发请求也没人应答——这正是想要的：让「不该发请求」的
/// 分支直接拿到通道已关闭的错误，而不是悄悄通过。
pub async fn test_requester() -> Requester {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    RequestBroker::new(tx).handle()
}

/// 测试用的最小 `ToolContext`：无工具 id、无子 agent 事件通道。
pub fn ctx_for<'a>(
    dir: &'a std::path::Path,
    cancel: &'a tokio_util::sync::CancellationToken,
    req: &'a Requester,
) -> ToolContext<'a> {
    ToolContext {
        working_dir: dir,
        cancel,
        request: req,
        tool_id: "",
        event_sink: None,
    }
}
