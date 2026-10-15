//! mrorcode-agent: 组装 kernel 与 capabilities 的编码运行时（L2）。

mod assemble;
mod persona;
mod runtime;

pub use assemble::{
    assemble, assemble_openai_compat, assemble_openai_compat_with_model, rebuild_assembly,
    AssembleOptions, Assembly, InitialModel,
};
pub use mrorcode_capabilities::session::{SessionStore, SESSIONS_DIR};
pub use mrorcode_kernel::PermissionMode;
pub use runtime::{start, DriverCommand, RuntimeEvent, RuntimeHandle, RuntimeStart};
