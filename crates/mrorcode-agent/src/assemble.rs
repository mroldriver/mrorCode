//! 能力装配：把 config 解析结果 + 内置工具 + 审批中间件 + persona 组装为 runtime。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mrorcode_capabilities::approval::ApprovalMiddleware;
use mrorcode_capabilities::provider::{OpenAiCompatConfig, OpenAiCompatProvider};
use mrorcode_capabilities::tools::builtin_registry;
use mrorcode_config::ResolvedProvider;
use mrorcode_kernel::ToolMiddleware;

use crate::persona::coding_persona;
use crate::runtime::{start, RuntimeEvent, RuntimeHandle, RuntimeStart};

pub struct Assembly {
    pub handle: RuntimeHandle,
    pub events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    pub model: String,
}

/// 装配选项。
#[derive(Clone, Debug, Default)]
pub struct AssembleOptions {
    /// 跳过全部审批（headless `--dangerously-skip-permissions`）。
    pub skip_permissions: bool,
}

/// 用 OpenAI 兼容 provider 装配一个完整 coding runtime。
pub fn assemble_openai_compat(
    resolved: &ResolvedProvider,
    working_dir: PathBuf,
    options: &AssembleOptions,
) -> anyhow::Result<Assembly> {
    let provider = OpenAiCompatProvider::new(OpenAiCompatConfig {
        base_url: resolved.base_url.clone(),
        api_key: resolved.api_key.clone(),
        model: resolved.model.clone(),
        context_window: resolved.context_window,
        timeout: Duration::from_secs(300),
    })?;

    let registry = builtin_registry();

    let mut middleware: Vec<Arc<dyn ToolMiddleware>> = Vec::new();
    if !options.skip_permissions {
        middleware.push(Arc::new(ApprovalMiddleware::new(
            ApprovalMiddleware::shared_memory() as Arc<_>,
        )));
    }

    let system_prompt = coding_persona(&working_dir);

    let (handle, events) = start(RuntimeStart {
        provider: Arc::new(provider),
        tools: registry,
        middleware,
        working_dir,
        system_prompt,
    });

    Ok(Assembly {
        handle,
        events,
        model: resolved.model.clone(),
    })
}
