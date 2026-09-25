//! 能力装配：把 config 解析结果 + 内置工具 + 审批中间件 + persona 组装为 runtime。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mrorcode_capabilities::approval::ApprovalMiddleware;
use mrorcode_capabilities::mcp::{bootstrap, McpBootstrap};
use mrorcode_capabilities::provider::{
    AnthropicConfig, AnthropicProvider, OpenAiCompatConfig, OpenAiCompatProvider,
};
use mrorcode_capabilities::tools::{builtin_registry, read_only_registry, TaskTool};
use mrorcode_config::{McpServer, ResolvedProvider};
use mrorcode_kernel::{LlmProvider, ToolMiddleware};

use crate::persona::{coding_persona, read_only_sub_persona};
use crate::runtime::{start, RuntimeEvent, RuntimeHandle, RuntimeStart};

pub struct Assembly {
    pub handle: RuntimeHandle,
    pub events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    pub model: String,
    /// 同 provider 下可切换模型（含当前 model）。
    pub available_models: Vec<String>,
    /// 已连接的 MCP server（保持存活；Drop 即 teardown 子进程）。
    pub mcp: Option<McpBootstrap>,
    /// MCP 连接诊断（非致命）。
    pub warnings: Vec<String>,
}

/// 装配输入：额外携带启动模型覆盖（KV 记住的选择）。
#[derive(Clone, Debug, Default)]
pub struct InitialModel(pub Option<String>);

/// 装配选项。
#[derive(Clone, Debug, Default)]
pub struct AssembleOptions {
    /// 跳过全部审批（headless `--dangerously-skip-permissions`）。
    pub skip_permissions: bool,
    /// 上下文压缩策略（来自 config）。
    pub compaction: mrorcode_kernel::agent::CompactionPolicy,
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
    assemble_inner(resolved, working_dir, options, initial_model, None)
}

/// 完整装配（含 MCP）。MCP 需异步握手，故装配为 async。
pub async fn assemble(
    resolved: &ResolvedProvider,
    working_dir: PathBuf,
    options: &AssembleOptions,
    initial_model: Option<String>,
    mcp_servers: &[McpServer],
) -> anyhow::Result<Assembly> {
    let mcp = if mcp_servers.is_empty() {
        None
    } else {
        Some(bootstrap(mcp_servers).await)
    };
    let mut assembly = assemble_inner(resolved, working_dir, options, initial_model, mcp)?;
    if let Some(boot) = &assembly.mcp {
        assembly.warnings = boot.warnings.clone();
    }
    Ok(assembly)
}

fn assemble_inner(
    resolved: &ResolvedProvider,
    working_dir: PathBuf,
    options: &AssembleOptions,
    initial_model: Option<String>,
    mcp: Option<McpBootstrap>,
) -> anyhow::Result<Assembly> {
    let provider: Arc<dyn LlmProvider> = match resolved.provider_type.as_str() {
        "anthropic" => Arc::new(AnthropicProvider::new(AnthropicConfig {
            base_url: resolved.base_url.clone(),
            api_key: resolved.api_key.clone(),
            model: resolved.model.clone(),
            context_window: resolved.context_window,
            timeout: Duration::from_secs(300),
            max_tokens: 4096,
        })?),
        _ => Arc::new(OpenAiCompatProvider::new(OpenAiCompatConfig {
            base_url: resolved.base_url.clone(),
            api_key: resolved.api_key.clone(),
            model: resolved.model.clone(),
            context_window: resolved.context_window,
            timeout: Duration::from_secs(300),
        })?),
    };

    let mut registry = builtin_registry();
    if let Some(boot) = &mcp {
        boot.register_into(&mut registry);
    }
    // M4-10：task 只读子 agent 工具，与主 agent 共享 provider。
    registry.insert(Arc::new(TaskTool::new(
        Arc::clone(&provider),
        read_only_registry(),
        read_only_sub_persona(&working_dir),
        Some(resolved.model.clone()),
    )));

    let mut middleware: Vec<Arc<dyn ToolMiddleware>> = Vec::new();
    if !options.skip_permissions {
        middleware.push(Arc::new(ApprovalMiddleware::new(
            ApprovalMiddleware::shared_memory() as Arc<_>,
        )));
    }

    let tool_names: Vec<String> = registry.names().map(str::to_string).collect();
    let system_prompt = coding_persona(&working_dir, &tool_names);

    // KV 记住的模型若在可用列表内则作为启动覆盖
    let model = match &initial_model {
        Some(m) if resolved.models.iter().any(|x| x == m) => m.clone(),
        _ => resolved.model.clone(),
    };

    let (handle, events) = start(RuntimeStart {
        provider,
        tools: registry,
        middleware,
        working_dir,
        system_prompt,
        model: Some(model.clone()).filter(|m| m != &resolved.model),
        compaction: options.compaction,
    });

    Ok(Assembly {
        handle,
        events,
        model,
        available_models: resolved.models.clone(),
        mcp,
        warnings: Vec::new(),
    })
}
