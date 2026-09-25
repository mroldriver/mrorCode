# mrorCode 项目计划

> Rust 编写的终端 AI 编码 Agent，TUI 视觉/交互对标 OpenCode，后端架构借鉴 AtomCode。
>
> **进度：M1（骨架）、M2（TUI 核心体验）、M3（Agent 能力增强）、M4（TUI 精致化）均已完成**，
> 当前版本 `0.5.1`，工作区 230 个测试全绿。实施细节、实测基线与偏差见
> [roadmap.md](roadmap.md)；后续方向见其末节「M5 后续里程碑方向」。

---

## 一、项目定位与目标

**核心目标**：用 Rust 打造一个终端 AI 编码 Agent，TUI 视觉/交互对标 OpenCode，后端架构借鉴 AtomCode 的分层设计。立项动机是 opencode 太慢——**速度是第一性约束**，而非后期优化项（对应架构：单进程内类型化事件、无固定帧率、增量 markdown、窗口化渲染）。

| 维度 | 借鉴来源 | 说明 |
|---|---|---|
| 后端核心架构 | AtomCode (Rust) | kernel → capabilities → agent → driver 分层 |
| TUI 视觉与交互 | OpenCode (TS) | 侧边栏、多会话、命令面板、对话框系统、主题插件 |
| 语言 | Rust | 全栈 Rust，单二进制部署 |
| 终端渲染 | Rust 生态选型 | 见下方「TUI 技术选型」 |

---

## 二、TUI 技术选型（关键决策）

OpenCode 的 TUI 基于 `@opentui/solid`（类 Solid.js 响应式 + TSX 渲染到终端 ANSI），Rust 生态没有对等物。

### 方案对比

| 方案 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| **ratatui + 自研 Elm 式 store** | 生态成熟、组件多、上手快；状态单向可测 | 复杂 UI 需自管状态 | ✅ 已采用 |
| **ratatui + 自研 retained 抽象** | 兼顾生态和可维护性 | 需额外抽象层开发 | 备选 |
| **完全自研渲染器 (atomcode 路线)** | 极致性能、完全可控 | 工程量巨大（retained.rs 就 2.6 万行） | ❌ |
| **crossterm 直写** | 零依赖、灵活 | 所有组件从零写 | ❌ |

**最终方案：ratatui 0.30（crossterm 0.29 后端）+ 自研 Elm 式 store 状态层 + 脏标记驱动的合批渲染**。
理由：ratatui 是 Rust TUI 事实标准，生态最丰富；在其上加一层单向数据流状态管理，既能快速迭代，又保留未来替换渲染底层的能力。

TUI 遵循单一所有权铁律：**agent/会话权威状态只在 runtime；TUI store 只存投影与纯 UI 态**（滚动/输入/弹窗），所有动作经 `DriverCommand`，不存在第二份状态机。

---

## 三、整体架构（Workspace Crate 划分）

实际落地为 **6 个 crate**（初版设计里的 `providers/tools/mcp/coding` 已按 AtomCode 的「能力内聚」原则合并进 `capabilities` 与 `agent`）：

```
mrorCode/
├── Cargo.toml                    # workspace 根（version = 0.5.0，default-members = cli）
├── crates/
│   ├── mrorcode-kernel/          # L0 中立内核 — Agent 抽象、消息、工具、事件、会话数据
│   ├── mrorcode-capabilities/    # L1 能力 — provider / 工具 / MCP / 会话持久化 / 审批
│   ├── mrorcode-agent/           # L2 装配 + Runtime（唯一运行时所有者）
│   ├── mrorcode-config/          # leaf — 配置系统（TOML + 环境变量）
│   ├── mrorcode-tui/             # UI 层 — ratatui + 自研组件/状态
│   └── mrorcode-cli/             # 入口 — CLI 解析、启动 TUI/headless
```

依赖方向（编译期不可反向）：

```
cli ─┬─> tui ─> agent ─> capabilities ─> kernel
     └─> agent ──┘            └─> config（leaf，谁都可读）
```

`cli/headless` 同样经 agent runtime，**禁止第二份 agent 状态机**。

### 各 crate 职责与关键内容

#### `mrorcode-kernel` — 核心内核（L0）
- **核心类型**：`Message`/`Conversation`（含 tool_call/result 配对修复）、`Tool`/`ToolRegistry`、`LlmProvider`、`ToolMiddleware`、`Agent`/`AgentHandle`。
- **事件系统**：`AgentCommand` / `AgentEvent` 均可序列化，UI 与运行时解耦。
- **会话数据**：`session.rs`（`SessionRecord`/`TranscriptEntry`，纯数据零 IO）、`message.rs` 的 `compact`/`estimate_tokens`（上下文压缩）。
- 依赖仅 tokio/futures/async-trait/serde/thiserror，零 fs、零网络。

#### `mrorcode-capabilities` — 能力实现（L1）
- `provider/`：`sse.rs`（字节有状态 SSE 解码，手写不引 eventsource）、`openai_compat.rs`（OpenAI 兼容 + 工具调用 + 重试）、`retry.rs`。
- `tools/`：`read_file`、`bash`、`write_file`、`edit_file`、`glob`、`grep`，附 `diff.rs`（similar）与 `read_tracker.rs`（read-before-write）。
- `path_guard.rs`：所有文件工具强制 working_dir 内解析，拒绝 `..`/符号链接逃逸。
- `session/`：`SessionStore`（`~/.mrorcode/sessions/<id>.jsonl` transcript + `<id>.snapshot.json` 原子快照）。
- `mcp/`：stdio JSON-RPC 客户端 + `McpTool`（动态注册 `mcp__<server>__<tool>`）。
- `approval.rs`：审批中间件 + 内存授权 store。

#### `mrorcode-agent` — 编码 Agent 运行时（L2）
- `assemble.rs`：从 config 装配 provider、内置工具、MCP、审批中间件、系统 persona（`async`，MCP 需 await 拉起子进程）。
- `runtime.rs`：单 owner task 的 `Runtime`；`DriverCommand::{Submit{session_id,text}, Respond, SetModel, Resume, SetPlanMode, Cancel, Shutdown}`；`RuntimeEvent` 转发时包 `session_id`。
- `persona.rs`：从实际挂载的 registry 动态生成可用工具清单。

#### `mrorcode-config` — 配置系统（leaf）
- `Config { default_provider, default_model, providers, compaction, mcp }`，全部 `#[serde(default)]`。
- 加载顺序（高→低）：环境变量 > `$MRORCODE_CONFIG` 文件 > `~/.mrorcode/config.toml`；损坏文件产生 warning 而非 panic。
- `config.example.toml` 仅作仓库参考，不自动落盘。

#### `mrorcode-tui` — 终端界面（对标 OpenCode）
- 主界面：左侧栏（会话列表）+ 会话视图 + 底部输入框；`store.rs` 单一投影。
- 组件：增量 Markdown 渲染、syntect 高亮、diff 着色、工具折叠卡、命令面板、对话框栈、toast。
- 主题：`src/themes/*.json` 由 `build.rs` 编译期嵌入（vendored 33 个 opencode MIT 主题）。
- 输入层：`input.rs` 的 `@` 文件补全 + `/` 斜杠命令。

#### `mrorcode-cli` — CLI 入口
- 子命令：`mrorcode`（默认 TUI）、`mrorcode run [--json]`（一次性任务）。
- 全局参数：`-C/--cd`、`--model`、`--provider`、`--resume <id>`、`--dangerously-skip-permissions`。
- clap 解析 + 组装各模块并启动；`headless.rs` 输出流式文本或版本化 JSONL。

---

## 四、TUI 功能对标清单（实际状态）

### P0 — 可用版本 ✅
- [x] 主界面三栏布局：侧边栏（会话列表）+ 对话区 + 输入框
- [x] 消息流式渲染（增量 Markdown）
- [x] 工具调用状态展示（spinner + 折叠展开）
- [x] 输入框：多行输入、持久化历史、提交
- [x] 会话切换 / 新建 / 删除（多会话，含磁盘持久化与 resume）
- [x] 键盘快捷键系统（base/modal 两层 + leader 键 + 用户覆盖）

### P1 — 完整体验 ✅
- [x] 命令面板（`ctrl+p`），模糊搜索 + 分组
- [x] 对话框系统（主题选择、模型选择、确认/提示/帮助）
- [x] 侧边栏：会话列表 + 工作目录/模型徽标
- [x] 主题系统（33 个内置 JSON + dark/light）
- [x] 代码审查视图（write/edit 结果的 unified diff 着色）
- [x] 会话标签页（多会话并排）
- [x] 剪贴板支持（拖选复制，OSC 52 兜底）

### P2 — 高级特性
- [x] 语法高亮（syntect，闭合代码块一次性高亮并入缓存）
- [x] `/` 斜杠命令与 `@` 文件补全
- [x] plan 模式（Shift+Tab，只读 gate）
- [x] 思考过程折叠 / 展开
- [x] 子代理（`task` 只读子 agent；实时嵌套视图延后）
- [x] rewind 时间线
- [ ] 图片附件渲染——归 M5
- [ ] 插件系统 / 国际化——归 M5
- [x] 鼠标交互（滚动、点击、选择）

---

## 五、开发阶段与进度

| 阶段 | 目标 | 状态 |
|---|---|---|
| **M1 骨架** | 跑通最小闭环：发消息 → 调 LLM → 流式显示 → 工具调用 | ✅ 2026-09-23 |
| **M2 TUI 核心体验** | 主题/高亮/折叠卡/对话框/命令面板/键位/多会话 | ✅ 2026-09-24 |
| **M3 Agent 能力** | 编辑/搜索工具、持久化+resume、压缩、MCP、输入层、plan、路径 gate | ✅ 2026-09-24 |
| **M4 TUI 精致化** | 多会话标签、diff viewer、reasoning 折叠、子 agent、rewind、鼠标/剪贴板 | ✅ 2026-09-25 |
| **M5 硬化发布** | Anthropic provider、daemon、跨平台 CI、conformance、打包分发 | ⏳ 待启动 |

各里程碑的步骤、验收标准与完成报告见 [roadmap.md](roadmap.md)。

---

## 六、关键技术栈

| 类别 | 选型 | 说明 |
|---|---|---|
| 异步运行时 | tokio | 全项目统一 |
| 终端 I/O | crossterm 0.29 | 跨平台终端事件 |
| TUI 渲染 | ratatui 0.30 | 主流成熟方案 |
| 文本输入 | tui-textarea-2 0.13 | 多行编辑器（封装在 prompt.rs） |
| Markdown | pulldown-cmark 0.12 | 解析 + 自写终端渲染（增量） |
| 语法高亮 | syntect 5.3 | `default-fancy` 纯 Rust，无 onig/C 依赖 |
| 配置 | serde + toml | TOML 配置文件 |
| CLI 解析 | clap 4 | derive |
| 错误处理 | anyhow（应用）+ thiserror（库） | |
| HTTP | reqwest 0.12（**native-tls**） | 本机无 C 编译器，rustls 的 ring 会断构建 |
| 编辑/搜索 | similar / ignore / regex | 均纯 Rust 无 C 依赖 |
| SSE | 手写解码器 | 不引 eventsource（见 architecture 规则） |
| 基准 | criterion | 性能回归守护（release gate） |

**MSRV 1.88**；版本号唯一来源为根 `Cargo.toml` 的 `[workspace.package] version`。

---

## 七、与两个参考项目的对应关系

| 功能模块 | AtomCode 对应 | OpenCode 对应 | 我们的位置 |
|---|---|---|---|
| 核心内核 | `atomcode-kernel` | `services/agent/` | `mrorcode-kernel` |
| 编码 Agent 运行时 | `atomcode-coding` | `agent/` | `mrorcode-agent` |
| Provider / 工具 / MCP / 会话 | `atomcode-capabilities` | `providers/` `tools/` `mcp/` | `mrorcode-capabilities`（内聚） |
| TUI | `atomcode-tuix` | `packages/tui/` | `mrorcode-tui` |
| CLI | `atomcode-cli` | `cli/` | `mrorcode-cli` |
| 配置 | `atomcode-config` | `config/` | `mrorcode-config` |

---

## 八、已确认事项

- [x] TUI 渲染层选型：ratatui + 自研 Elm 式状态层
- [x] 项目命名：crate 前缀 `mrorcode-`，二进制 `mrorcode`，配置目录 `~/.mrorcode/`
- [x] 首个 Provider：OpenAI 兼容（覆盖 OpenAI/DeepSeek/GLM/本地网关）
- [x] M1–M3 范围与验收标准（见 roadmap.md）
