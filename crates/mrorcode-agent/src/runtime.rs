//! 编码运行时：driver 与 kernel agent 之间的薄桥。
//!
//! 唯一 agent 状态所有者是 kernel；这里只做命令/事件桥接，不建第二份状态机。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use mrorcode_kernel::event::{AgentCommand, AgentEvent};
use mrorcode_kernel::tool::ToolRegistry;
use mrorcode_kernel::AgentBuilder;
use mrorcode_kernel::ToolMiddleware;

/// driver → runtime 的命令（M1 精简集）。
#[derive(Clone, Debug)]
pub enum DriverCommand {
    Submit(String),
    Respond(u64, Value),
    Cancel,
    Shutdown,
}

/// runtime → driver 的事件。
#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    Agent(AgentEvent),
    /// runtime 已退出（终态）。
    Exited,
}

/// 廉价 Clone 的控制句柄。
#[derive(Clone)]
pub struct RuntimeHandle {
    commands: UnboundedSender<DriverCommand>,
}

impl RuntimeHandle {
    pub fn submit(&self, text: impl Into<String>) {
        let _ = self.commands.send(DriverCommand::Submit(text.into()));
    }

    pub fn respond(&self, id: u64, value: Value) {
        let _ = self.commands.send(DriverCommand::Respond(id, value));
    }

    pub fn cancel(&self) {
        let _ = self.commands.send(DriverCommand::Cancel);
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
}

/// 启动 runtime，返回控制句柄与事件接收端。
pub fn start(start: RuntimeStart) -> (RuntimeHandle, UnboundedReceiver<RuntimeEvent>) {
    let agent = AgentBuilder::new()
        .provider(start.provider)
        .tools(start.tools)
        .system_prompt(start.system_prompt)
        .working_dir(start.working_dir);
    let agent = start
        .middleware
        .into_iter()
        .fold(agent, |a, mw| a.add_middleware(mw));
    let mut agent_handle = agent.spawn();

    let (cmd_tx, mut cmd_rx) = unbounded_channel::<DriverCommand>();
    let (evt_tx, evt_rx) = unbounded_channel::<RuntimeEvent>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;

                cmd = cmd_rx.recv() => {
                    match cmd {
                        None | Some(DriverCommand::Shutdown) => {
                            let _ = agent_handle.commands.send(AgentCommand::Shutdown);
                            // 排空事件直到 agent 关闭
                            while let Some(ev) = agent_handle.events.recv().await {
                                let _ = evt_tx.send(RuntimeEvent::Agent(ev));
                            }
                            let _ = evt_tx.send(RuntimeEvent::Exited);
                            break;
                        }
                        Some(DriverCommand::Submit(text)) => {
                            let _ = agent_handle.commands.send(AgentCommand::SendMessage(text));
                        }
                        Some(DriverCommand::Respond(id, value)) => {
                            let _ = agent_handle.commands.send(AgentCommand::Respond(id, value));
                        }
                        Some(DriverCommand::Cancel) => {
                            let _ = agent_handle.commands.send(AgentCommand::Cancel);
                        }
                    }
                }

                ev = agent_handle.events.recv() => {
                    match ev {
                        Some(ev) => {
                            if evt_tx.send(RuntimeEvent::Agent(ev)).is_err() {
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
