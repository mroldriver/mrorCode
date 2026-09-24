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
    /// 同 provider 下可切换模型（含当前 model）。
    pub available_models: Vec<String>,
}

/// 装配输入：额外携带启动模型覆盖（KV 记住的选择）。
#[derive(Clone, Debug, Default)]
pub struct InitialModel(pub Option<String>);

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
    assemble_openai_compat_with_model(resolved, working_dir, options, None)
}

pub fn assemble_openai_compat_with_model(
    resolved: &ResolvedProvider,
    working_dir: PathBuf,
    options: &AssembleOptions,
    initial_model: Option<String>,
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

    // KV 记住的模型若在可用列表内则作为启动覆盖
    let model = match &initial_model {
        Some(m) if resolved.models.iter().any(|x| x == m) => m.clone(),
        _ => resolved.model.clone(),
    };

    let (handle, events) = start(RuntimeStart {
        provider: Arc::new(provider),
        tools: registry,
        middleware,
        working_dir,
        system_prompt,
        model: Some(model.clone()).filter(|m| m != &resolved.model),
    });

    Ok(Assembly {
        handle,
        events,
        model,
        available_models: resolved.models.clone(),
    })
}
