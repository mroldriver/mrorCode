//! 编码运行时：driver 与 kernel agent 之间的薄桥。
//!
//! 唯一 agent 状态所有者是 kernel；这里只做命令/事件桥接，不建第二份状态机。
//! 事件按当前 turn 所属 session 标记，供 TUI 投影到对应会话。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use mrorcode_kernel::event::{AgentCommand, AgentEvent};
use mrorcode_kernel::tool::ToolRegistry;
use mrorcode_kernel::AgentBuilder;
use mrorcode_kernel::ToolMiddleware;

/// driver → runtime 的命令（M3：多会话 + 模型切换 + resume + plan）。
#[derive(Clone, Debug)]
pub enum DriverCommand {
    Submit {
        session_id: u64,
        text: String,
    },
    /// 从磁盘恢复的会话体注入 kernel（再 submit 即延续上下文）。
    Resume {
        session_id: u64,
        conversation: mrorcode_kernel::message::Conversation,
    },
    SetModel(String),
    /// 切换 plan 模式（只读 gate）。
    SetPlanMode(bool),
    Respond(u64, Value),
    /// 回退会话到保留最早 `keep_turns` 个 user turn（M4-9）。
    Rewind {
        session_id: u64,
        keep_turns: usize,
    },
    Cancel,
    Shutdown,
}

/// runtime → driver 的事件。
#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    Agent {
        session_id: u64,
        event: AgentEvent,
    },
    /// runtime 已退出（终态）。
    Exited,
}

/// 廉价 Clone 的控制句柄。
#[derive(Clone)]
pub struct RuntimeHandle {
    commands: UnboundedSender<DriverCommand>,
}

impl RuntimeHandle {
    pub fn submit(&self, session_id: u64, text: impl Into<String>) {
        let _ = self.commands.send(DriverCommand::Submit {
            session_id,
            text: text.into(),
        });
    }

    pub fn resume(&self, session_id: u64, conversation: mrorcode_kernel::message::Conversation) {
        let _ = self.commands.send(DriverCommand::Resume {
            session_id,
            conversation,
        });
    }

    pub fn set_model(&self, model: impl Into<String>) {
        let _ = self.commands.send(DriverCommand::SetModel(model.into()));
    }

    pub fn set_plan_mode(&self, on: bool) {
        let _ = self.commands.send(DriverCommand::SetPlanMode(on));
    }

    pub fn respond(&self, id: u64, value: Value) {
        let _ = self.commands.send(DriverCommand::Respond(id, value));
    }

    pub fn cancel(&self) {
        let _ = self.commands.send(DriverCommand::Cancel);
    }

    /// 回退会话到保留最早 `keep_turns` 个 user turn；kernel 空闲时生效（M4-9）。
    pub fn rewind(&self, session_id: u64, keep_turns: usize) {
        let _ = self.commands.send(DriverCommand::Rewind {
            session_id,
            keep_turns,
        });
    }

    pub fn shutdown(&self) {
        let _ = self.commands.send(DriverCommand::Shutdown);
    }
}

/// 运行时启动所需的全部装配。
pub struct RuntimeStart {
    pub provider: Arc<dyn mrorcode_kernel::LlmProvider>,
    pub tools: ToolRegistry,
    pub middleware: Vec<Arc<dyn ToolMiddleware>>,
    pub working_dir: PathBuf,
    pub system_prompt: String,
    /// 启动即生效的模型覆盖（如 KV 中记住的选择）。
    pub model: Option<String>,
    /// 上下文压缩策略。
    pub compaction: mrorcode_kernel::agent::CompactionPolicy,
}

/// 启动 runtime，返回控制句柄与事件接收端。
pub fn start(start: RuntimeStart) -> (RuntimeHandle, UnboundedReceiver<RuntimeEvent>) {
    let mut builder = AgentBuilder::new()
        .provider(start.provider)
        .tools(start.tools)
        .system_prompt(start.system_prompt)
        .compaction(start.compaction)
        .working_dir(start.working_dir);
    // 必须挂上中间件：审批（ApprovalMiddleware）走的就是这条链。
    // 漏挂会让 Risky 工具跳过审批直接执行（TUI 与 headless 同时失效）。
    for mw in start.middleware {
        builder = builder.add_middleware(mw);
    }
    if let Some(model) = start.model {
        let options = mrorcode_kernel::ChatOptions {
            model: Some(model),
            ..Default::default()
        };
        builder = builder.chat_options(options);
    }
    let mut agent_handle = builder.spawn();

    let (cmd_tx, mut cmd_rx) = unbounded_channel::<DriverCommand>();
    let (evt_tx, evt_rx) = unbounded_channel::<RuntimeEvent>();

    tokio::spawn(async move {
        // 当前 turn 所属会话；无 turn 时事件不会产生
        let mut active_session: u64 = 0;
        loop {
            tokio::select! {
                biased;

                cmd = cmd_rx.recv() => {
                    match cmd {
                        None | Some(DriverCommand::Shutdown) => {
                            let _ = agent_handle.commands.send(AgentCommand::Shutdown);
                            while let Some(ev) = agent_handle.events.recv().await {
                                let _ = evt_tx.send(RuntimeEvent::Agent {
                                    session_id: active_session,
                                    event: ev,
                                });
                            }
                            let _ = evt_tx.send(RuntimeEvent::Exited);
                            break;
                        }
                        Some(DriverCommand::Submit { session_id, text }) => {
                            active_session = session_id;
                            let _ = agent_handle
                                .commands
                                .send(AgentCommand::Submit { session_id, text });
                        }
                        Some(DriverCommand::Resume { session_id, conversation }) => {
                            active_session = session_id;
                            let _ = agent_handle
                                .commands
                                .send(AgentCommand::LoadSession { session_id, conversation });
                        }
                        Some(DriverCommand::SetModel(model)) => {
                            let _ = agent_handle
                                .commands
                                .send(AgentCommand::SetModel(model));
                        }
                        Some(DriverCommand::SetPlanMode(on)) => {
                            let _ = agent_handle
                                .commands
                                .send(AgentCommand::SetPlanMode(on));
                        }
                        Some(DriverCommand::Respond(id, value)) => {
                            let _ = agent_handle.commands.send(AgentCommand::Respond(id, value));
                        }
                        Some(DriverCommand::Cancel) => {
                            let _ = agent_handle.commands.send(AgentCommand::Cancel);
                        }
                        Some(DriverCommand::Rewind {
                            session_id,
                            keep_turns,
                        }) => {
                            let _ = agent_handle.commands.send(AgentCommand::Rewind {
                                session_id,
                                keep_turns,
                            });
                        }
                    }
                }

                ev = agent_handle.events.recv() => {
                    match ev {
                        Some(event) => {
                            if evt_tx
                                .send(RuntimeEvent::Agent {
                                    session_id: active_session,
                                    event,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        None => {
                            let _ = evt_tx.send(RuntimeEvent::Exited);
                            break;
                        }
                    }
                }
            }
        }
    });

    (RuntimeHandle { commands: cmd_tx }, evt_rx)
}
