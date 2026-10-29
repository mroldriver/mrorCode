//! task：委派一个只读子 agent 完成子任务（M4-10）。
//!
//! 只持只读工具（read/glob/grep），不挂写/改/bash；结果以最终文本回灌，
//! 作为普通工具卡呈现（实时嵌套视图为可选加分项，暂不做）。

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use mrorcode_kernel::agent::AgentBuilder;
use mrorcode_kernel::event::{AgentCommand, AgentEvent};
use mrorcode_kernel::provider::LlmProvider;
use mrorcode_kernel::tool::{RiskLevel, Tool, ToolContext, ToolRegistry, ToolResult};

/// 子 agent 的最大 round 数（有保险丝，避免死循环烧 token）。
pub const SUB_MAX_ROUNDS: usize = 15;

/// `task` 工具。`provider` 与主 agent 共享，子 agent 只挂只读工具。
pub struct TaskTool {
    provider: Arc<dyn LlmProvider>,
    read_only: ToolRegistry,
    sub_system_prompt: String,
    model: Option<String>,
}

#[derive(Deserialize)]
struct Args {
    /// 给子 agent 的任务描述（自然语言指令）。
    prompt: String,
}

impl TaskTool {
    /// 构造 task 工具。
    ///
    /// `read_only` 必须是纯只读工具注册表（read/glob/grep 等），
    /// `sub_system_prompt` 为子 agent 的 persona；`model` 可选模型覆盖（None 用 provider 默认）。
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        read_only: ToolRegistry,
        sub_system_prompt: String,
        model: Option<String>,
    ) -> Self {
        Self {
            provider,
            read_only,
            sub_system_prompt,
            model,
        }
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Delegate a read-only research subtask to a sub-agent. The sub-agent may only use \
         read-only tools (read/glob/grep); it never edits or runs commands. Returns the \
         sub-agent's final text answer."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Optional short label shown in the tool card (not sent to the sub-agent)."
                },
                "prompt": {
                    "type": "string",
                    "description": "Task instructions for the read-only sub-agent, in natural language."
                }
            },
            "required": ["prompt"]
        })
    }

    fn risk(&self, _arguments: &serde_json::Value) -> RiskLevel {
        // 派生只读子 agent 会消耗 token/时间，故设为 Risky（需批准）
        RiskLevel::Risky
    }

    async fn execute(&self, arguments: &str, ctx: &ToolContext<'_>) -> ToolResult {
        let args: Args = match serde_json::from_str(arguments) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::error("task", format!("bad arguments: {e}"));
            }
        };
        if args.prompt.trim().is_empty() {
            return ToolResult::error("task", "prompt is required");
        }

        let mut builder = AgentBuilder::new()
            .provider(Arc::clone(&self.provider))
            .tools(self.read_only.clone()) // ToolRegistry: Clone 由本 crate 提供
            .system_prompt(self.sub_system_prompt.clone())
            .working_dir(ctx.working_dir.to_path_buf())
            .max_rounds(SUB_MAX_ROUNDS);
        if let Some(model) = &self.model {
            let options = mrorcode_kernel::ChatOptions {
                model: Some(model.clone()),
                ..Default::default()
            };
            builder = builder.chat_options(options);
        }
        let mut handle = builder.spawn();

        let send_err = handle.commands.send(AgentCommand::Submit {
            session_id: 0,
            text: args.prompt.clone(),
        });
        if send_err.is_err() {
            return ToolResult::error("task", "sub-agent channel closed before submit");
        }

        // 子 agent 工具调用 id（用于事件投影定位，由主 agent 在 ToolContext 中传入）
        let parent_tool_id = ctx.tool_id.to_string();
        let sink = ctx.event_sink;

        let mut answer = String::new();
        let stop = loop {
            tokio::select! {
                biased;
                _ = ctx.cancel.cancelled() => {
                    let _ = handle.commands.send(AgentCommand::Cancel);
                    let _ = handle.commands.send(AgentCommand::Shutdown);
                    return ToolResult::error("task", "cancelled");
                }
                ev = handle.events.recv() => match ev {
                    None => break mrorcode_kernel::StopReason::Cancelled,
                    Some(AgentEvent::TextDelta(t)) => {
                        answer.push_str(&t);
                        // 转发文本增量到主会话实时投影
                        if let Some(sink) = sink {
                            let _ = sink.send(AgentEvent::SubAgent {
                                parent_tool_id: parent_tool_id.clone(),
                                event: Box::new(AgentEvent::TextDelta(t)),
                            });
                        }
                    }
                    Some(AgentEvent::TurnComplete { stop_reason }) => break stop_reason,
                    Some(AgentEvent::ToolStarted { id, name, arguments }) => {
                        if let Some(sink) = sink {
                            let _ = sink.send(AgentEvent::SubAgent {
                                parent_tool_id: parent_tool_id.clone(),
                                event: Box::new(AgentEvent::ToolStarted { id, name, arguments }),
                            });
                        }
                    }
                    Some(AgentEvent::ToolResult { id, name, content, is_error }) => {
                        if let Some(sink) = sink {
                            let _ = sink.send(AgentEvent::SubAgent {
                                parent_tool_id: parent_tool_id.clone(),
                                event: Box::new(AgentEvent::ToolResult { id, name, content, is_error }),
                            });
                        }
                    }
                    Some(_) => {}
                },
            }
        };
        let _ = handle.commands.send(AgentCommand::Shutdown);

        if answer.trim().is_empty() {
            return ToolResult::error("task", format!("sub-agent produced no answer ({stop:?})"));
        }
        ToolResult::ok("task", answer.trim_end().to_string())
    }
}
