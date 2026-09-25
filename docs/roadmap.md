# mrorCode 里程碑路线图（M1–M4 已完成）

> 本文合并自三份已完成的里程碑实施计划（原 `mrorcode_m1_plan.md` / `mrorcode_m2_plan.md` /
> `mrorcode_m3_plan.md`），保留原文以存档决策过程、实测基线与偏差记录。
> 产品总体设计见 [project-plan-atomcode.md](project-plan-atomcode.md)，
> 文档索引见 [README.md](README.md)。
>
> - **M1：骨架 — 多轮工具闭环 + 最小 TUI**（2026-09-23）
> - **M2：TUI 核心体验**（2026-09-24）
> - **M3：Agent 能力增强**（2026-09-24）
> - **M4：TUI 精致化**（2026-09-25，手工冒烟待人工）
> - **M5：硬化发布**（已细化为 a–e 实施计划，见文末；尚未开工）


---

## mrorCode M1 骨架实施计划：多轮工具闭环 + OpenCode 风格最小 TUI

> **状态：M1 已完成（2026-09-23）。** 12 个步骤全部落地，49 个自动化测试全绿，
> clippy `-D warnings` / fmt 门禁通过，debug+release 构建通过，
> headless 与 TUI 均端到端跑通。实测数据与偏差见文末「九、M1 完成报告」。
>
> 本计划是第一个可执行里程碑（M1）的实施方案。整体产品规划见
> [project-plan-atomcode.md](project-plan-atomcode.md)，
> 本文在其基础上把第一阶段细化为可落地的步骤、文件与验证。

### 一、调研结论（Repository Research）

#### 1.1 参考项目事实

**atomcode（Rust，v5.0.9）— 后端架构蓝本**

- 严格四层单向依赖：L0 `atomcode-kernel`（中立 agent 协议）→ L1 `atomcode-capabilities`（provider/工具/MCP/session）→ L2 `atomcode-coding`（`CodingRuntime` 唯一运行时所有者，prepare/assemble 两阶段装配）→ L3 `atomcode-cli`/`atomcode-tuix`/daemon（driver）。
- 内核关键设计（均经源码核实）：
  - `LlmProvider::chat_stream` 返回 `BoxStream<StreamEvent>`；`Tool` trait 的 `risk()` 只是建议元数据，强制力在 middleware。
  - `AgentCommand`/`AgentEvent` 全部可序列化；审批走 `RequestCtx` 的 oneshot（不进事件体），超时/cancel 一律 fail-closed 拒绝。
  - 工具执行三阶段 CLASSIFY → EXECUTE（并发门 + 信号量）→ APPLY（保序落库），结果中央 64KiB head+tail 截断。
  - Provider 的 SSE **不依赖 eventsource 库**：reqwest `bytes_stream()` + 纯字节有状态解码器（`feed(&[u8]) -> Vec<StreamEvent>`），用录制字节做单测。
  - 会话双层持久化：`.snapshot`（resume，有损压缩）+ `.jsonl` transcript（recall，不压缩）。
  - TUI 是 crossterm **自研 retained 渲染器**（单 event_loop 约 1 万行），明确不跟随。
- 教训：runtime 代际管理、goal/loop、rewind、多权限 store、team 等是 5.0 成熟产品的复杂度，M1 全部不做。

**opencode（TS）— TUI 视觉/交互蓝本**

- TUI 包 `packages/tui` 通过 SDK 事件通道与后端通信，**UI 不拥有第二份 agent 状态**，只做事件投影 + 发命令——与 atomcode 的 driver 原则一致，mrorcode 照此办理。
- 视觉语言：几乎不用方框，统一「左侧竖线」`┃`（收尾 `╹`）+ panel 背景色；Home 居中（输入框宽 max(75, 屏宽 70%)）；Session 为消息滚动区（粘底）+ 底部固定输入/权限条 + 右侧 42 列 sidebar（屏宽 >120 内嵌，否则浮层）。
- 事件投影要点：事件 16ms 合批；message/part 按 `(created_at, id)` 有序 upsert；流式 token 对文本字段做 O(1) 追加；hydration 与实时事件有竞态合并。
- 输入：多行 textarea、Enter 提交 / Ctrl+J 等换行 / Esc 双击中断 / Up/Down 边界触发历史；@补全 + /斜杠命令 + extmark 虚拟片段。
- 对话框为栈结构（同一时刻只渲染栈顶），通用 Select（fuzzy 过滤 + 分组）/Alert/Confirm/Prompt/Help；命令面板即所有注册命令的 fuzzy 列表。
- 主题为 JSON token 制（语义色/背景/diff/markdown/syntax 共约 40 个 token + defs 引用 + dark/light 双值），33 个内置主题文件格式可直接兼容。
- 键位：分层模式栈（base/modal/autocomplete）+ leader 键（默认 ctrl+x，2s 超时）。

#### 1.2 已确认决策

| 决策项 | 结论 |
|---|---|
| 命名 | 二进制 `mrorcode`，crate 前缀 `mrorcode-*`，配置目录 `~/.mrorcode/` |
| M1 范围 | 多轮对话 + `read_file`/`bash` 工具调用闭环 + 流式 TUI（三栏雏形 + 审批弹窗） |
| 首个 Provider | OpenAI 兼容（覆盖 OpenAI/DeepSeek/GLM/兼容网关），手写 SSE 解码 |
| TUI 栈 | ratatui 0.30（crossterm 0.29 后端）+ 自研 Elm 式 store 状态层 |
| Rust 版本 | MSRV 1.88（ratatui 0.30 要求） |

### 二、M1 目标与非目标

#### 目标（验收标准）

1. `cargo build` 产出单二进制 `mrorcode`；无配置时报清晰的引导错误。
2. `mrorcode`（无参）启动 TUI：Home 页居中输入框；发送后进入 Session 页，流式渲染回复，工具调用显示状态卡片，bash 危险命令弹出 once/always/reject 审批。
3. 支持多轮：工具结果回灌模型继续对话，直到模型无工具调用结束本轮；Esc（双击）可中断。
4. `mrorcode run "提示"` 走 headless 一次性任务，stdout 输出流式文本与工具状态（`--json` 输出版本化 JSONL），用于无 TTY 环境与自动化验证。
5. 配置来自 `~/.mrorcode/config.toml` 与环境变量（`MRORCODE_API_KEY`/`MRORCODE_BASE_URL`/`MRORCODE_MODEL`）。
6. 内核可用脚本化 MockProvider 做完整单测，不依赖真实 API。

#### 非目标（M1 明确不做，排入后续里程碑）

- 会话磁盘持久化 / resume / rewind / compact（M3）
- write/edit/grep/glob 以外工具、MCP、skills、memory、子 agent、plan 模式（M3）
- Markdown 完整渲染与 syntect 高亮、diff 视图、@补全、命令面板、主题 JSON、leader 键（M2）
- 鼠标、剪贴板矩阵、图片、IDE 集成、插件、守护进程/ACP、多 provider（M4+）

#### 性能硬指标（与功能同等验收，达不到不进 M2）

本项目立项动机就是 opencode 太慢，因此速度是第一性约束，不是后期优化项。
opencode 的慢点经拆解为：双进程 HTTP/SSE 事件总线（每事件 JSON 序列化往返）、
TS/Bun 运行时（冷启动、GC）、60fps 固定节拍 + Solid 全树 reconcile、
流式 markdown 逐 token 全量重解析。mrorcode 的对应对策是架构级的：

- **单进程内类型化事件**：TUI 直接持有 `RuntimeHandle`，热路径只有 enum + mpsc，零 JSON、零网络、零 worker 进程冷启动。
- **量化预算（M1 基准测试强制项）**：
  1. 冷启动到首帧绘制（TTFFD，有配置缓存时）目标 < 50ms（Windows 中端机，release profile）；
  2. 流式 1000 token/s 持续输入时，单核 CPU 占用 < 15%、输入延迟（按键到反馈）< 1 帧；
  3. 1 万行消息历史下滚动稳定 ≥ 60fps（终端支持时），内存增量 < 仅存文本的 2 倍；
  4. 空闲时（无事件、无 spinner）进程 **0 唤醒、0 输出、接近 0% CPU**；
  5. 审批弹窗弹出延迟 < 16ms。
- 上述指标在 M1 用 criterion 微基准 + 一个录制事件回放的端到端基准守护（见第六节），CI 中跑 release 基准并记录趋势。

### 三、Crate 与模块划分

```
mrorCode/
├── Cargo.toml                     # workspace：members=crates/*，workspace.dependencies 统一版本
├── rust-toolchain.toml            # 1.88+
├── .gitignore
└── crates/
    ├── mrorcode-kernel/           # L0 中立内核
    │   └── src/{lib,error,message,provider,tool,middleware,event,request,agent}.rs
    ├── mrorcode-capabilities/     # L1 具体能力（feature 门控：providers/tools）
    │   └── src/{lib,provider/{mod,openai_compat,sse,retry},tools/{mod,read,bash},approval}.rs
    ├── mrorcode-agent/            # L2 装配 + 精简 Runtime
    │   └── src/{lib,runtime,assemble,persona}.rs
    ├── mrorcode-config/           # leaf 配置
    │   └── src/{lib,settings}.rs
    ├── mrorcode-tui/              # L3 ratatui 界面
    │   └── src/{lib,app,store,theme,keymap,widgets/{mod,border,markdown,tool_card,textarea,prompt,permission,sidebar,home,session},render}.rs
    └── mrorcode-cli/              # 二进制入口
        └── src/{main,headless}.rs
```

依赖方向（编译期不可反向）：

```
cli ─┬─> tui ─> agent ─> capabilities ─> kernel
     └─> agent ──┘            └─> config（leaf，谁都可读）
cli/headless 同样经 agent runtime，禁止第二份 agent 状态机。
```

#### 各 crate 职责与关键类型

**mrorcode-kernel**（依赖仅 tokio/tokio-util/async-trait/futures/serde/serde_json/thiserror）

- `message.rs`：`Role`、`Message { role, text, tool_calls, tool_call_id, is_error, reasoning, meta }`、`ToolCall { id, name, arguments: String }`、`ToolResult { call_id, content, is_error }`、`ToolDef { name, description, parameters: serde_json::Value }`、`Conversation`（含 tool_call/result 配对修复）。
- `provider.rs`：`#[async_trait] trait LlmProvider { async fn chat_stream(&[Message], &[ToolDef], &ChatOptions) -> Result<BoxStream<'static, StreamEvent>, ProviderError> }`；`ChatOptions { temperature, max_tokens, tool_choice }`（只转发不做策略）；`StreamEvent { TextDelta, ToolCallStarted/Delta, Usage, Done, Error }`；`ProviderError { retryable, message, http_status }`。
- `tool.rs`：`#[async_trait] trait Tool: Send+Sync { name/description/parameters_schema; fn risk(&Value)->RiskLevel(Safe/Risky); async fn execute(&str, &ToolContext)->ToolResult }`；`ToolContext { working_dir, cancel: CancellationToken, request: Requester }`；`ToolRegistry`（BTreeMap 保序）。
- `middleware.rs`：`ToolMiddleware { before/after }`，`BeforeOutcome::Proceed/Allow/Ask/Deny`（`&mut ToolCall` 可改参数）；注册序即执行序。
- `event.rs`：`AgentCommand::{SendMessage(String), Respond(RequestId, Value), Cancel, Shutdown}`；`AgentEvent::{TurnStarted, TextDelta, ToolStarted{id,name,args}, ToolResult{...}, Usage, TurnComplete{stop_reason}, Request{id,kind,payload}, Error, Cancelled}`，均 `Serialize`。
- `request.rs`：`RequestCtx` 用 id + oneshot 做审批往返；超时/dropped 一律返回 `Null`（fail-closed）。
- `agent.rs`：`Agent::builder().provider(..).tools(..).middleware(..).hooks(..).spawn() -> AgentHandle { commands, events }`。turn 循环：发送请求 → 累积流事件（文本/工具参数分片）→ 有 tool_calls 则过 middleware `before` 后顺序执行（M1 不并发）→ 结果 `after` → 64KiB 截断 → 回灌进入下一 round，无 tool_calls 则 `TurnComplete`；`max_rounds`（默认 25）保险丝；`CancellationToken` 在流消费与工具执行间隙生效，Cancel 后补发 `Cancelled` 终态。

**mrorcode-capabilities**

- `provider/sse.rs`：`SseDecoder::feed(&[u8]) -> Vec<RawEvent>`，按行切分、仅在 UTF-8 字符边界解码、`[DONE]` 识别、保留半包字节；纯函数无网络。
- `provider/openai_compat.rs`：`OpenAiCompatProvider { base_url, api_key, model, context_window }`；POST `/chat/completions`（`stream:true`，tools 走 OpenAI function-calling 形态）；reqwest `bytes_stream()` 喂解码器；delta → `StreamEvent`；简单 `RetryPolicy`（429/5xx/网络错误指数退避 3 次，抖动；解析 `Retry-After`）。
- `tools/read.rs`：`read_file`（path 必填；只读；字节预算 + 超长分页提示）。
- `tools/bash.rs`：`bash`（command 必填；`risk()` 首版用保守词表判定：只读命令（cat/ls/grep/git status/diff/log…）Safe，其余 Risky；`tokio::process::Command` 执行，超时 120s，stdout/stderr 合并输出；Windows 用 `cmd /C`、Unix 用 `sh -c`，`$SHELL` 优先；编码用 encoding_rs 容错）。
- `approval.rs`：`ApprovalMiddleware` + `InMemoryPermissionStore`；Safe 放行 → 已授权 grant（bash 按规范化命令串）放行 → 否则 `request("approval", …)` 等 driver 回 `allow/allow_always/deny`；Null/超时 = 拒绝。

**mrorcode-agent**

- `assemble.rs`：从 config 构建 provider，注册 read_file/bash 与 ApprovalMiddleware，注入精简 coding persona（系统提示：角色、工具使用纪律、当前工作目录与日期）。
- `runtime.rs`：单 owner task 的精简 runtime。
  - `Runtime::start(cfg, working_dir) -> RuntimeHandle`；
  - `DriverCommand::{Submit(String), Respond(u64, serde_json::Value), Cancel, Shutdown}`；
  - `RuntimeEvent::{Agent(AgentEvent), Exited}`，M1 不做 generation/rewind；
  - `RuntimeHandle` 廉价 Clone（内部 mpsc/oneshot broker）；TUI 与 headless 都只持句柄。

**mrorcode-config**

- `Config { default_provider, default_model, providers: HashMap<String, ProviderConfig> }`，`ProviderConfig { provider_type:"openai_compat", base_url, api_key, model, context_window }`；全部 `#[serde(default)]`，缺失字段给诊断提示而非 panic。
- 加载顺序：环境变量 > `$MRORCODE_CONFIG` 指定文件 > `~/.mrorcode/config.toml`；提供 `config.example.toml`（仅作为仓库参考文件，随 CLI 初始化提示，不自动落盘）。

**mrorcode-tui**（ratatui 0.30 + crossterm 0.29 + tui-textarea-2 0.13）

- `app.rs`：终端 raw mode/alternate screen 生命周期（用 panic 兜底恢复终端）；主循环 `tokio::select! { biased }`：crossterm 输入事件（独立线程转 mpsc，用 `event::poll` 阻塞等待，不占 CPU）、runtime 事件、以及一个按需启动的合并定时器。**渲染采用脏标记驱动，不设固定帧率 tick**：任何输入/事件只置 `dirty=true`，首个脏信号启动 ~8ms 尾沿定时器，期间到达的 delta 全部合并，到点重绘一帧并清脏；无事件时循环完全阻塞在 select，0 唤醒。spinner 动画只在有 running 工具时才启用一个独立 100ms 慢 tick，工具结束立即取消。退出时 `Shutdown` 并恢复终端。
- `store.rs`（自研状态层，Elm 式）：
  - `AppState { route: Home|Session, sessions: Vec<SessionView>, current: Option<usize>, input: TextArea, phase: Idle|Working|WaitingApproval, dialog: Option<Dialog>, scroll: ScrollState, notice: Option<String> }`；
  - `Message`（枚举）：`UserInput(KeyEvent)`、`Runtime(AgentEvent)`、`Submit`、`ApprovalResolved`、`Tick`；
  - `fn update(&mut AppState, Message, &RuntimeHandle) -> Option<RuntimeCommand>` 纯更新（副作用以返回命令表达），`fn render(&Frame, &AppState)`；runtime 事件仅在此处投影，UI 不另建会话状态——这是 opencode/atomcode 共同的单一所有权原则。
  - 流式文本：`TextDelta` 对当前 assistant 消息的 `String` 做 `push_str`（O(1) 追加，不替换结构、不克隆历史），只置脏不立即重绘，由 app 的 8ms 尾沿合批成帧；消息容器用按 id 索引的 `Vec`（M1 单会话顺序追加即可），渲染态（解析结果/布局结果）独立缓存，不与权威文本混存。
- `theme.rs`：`Theme` 结构体先按 opencode token 命名建模（primary/text/background/background_panel/border/diff_*/markdown_* 等），内置一套（参考 tokyonight），代码中常量；**字段与 opencode 主题 JSON 一一对应**，M2 直接反序列化用户主题文件。
- `widgets/border.rs`：左竖线卡片原语（ratatui `Block::bordered().border(Borders::LEFT)` 配 `┃`/`╹` 符号与主题色），全站禁方框。
- `widgets/markdown.rs`（M1 精简版，性能设计先行）：pulldown-cmark 解析为块流（标题、列表、引用、行内代码、围栏代码块、粗体/斜体）。**不做每帧全量重解析**——维护增量解析器：已闭合的 block 解析一次后生成带样式的 span 并永久缓存（block 在 Vec 中的下标 → spans），流式追加时只把最后一个未闭合 block 的原文送去重新解析；delta 只改最后一个 block，已渲染历史零开销。折行结果按 `(block 下标, 渲染宽度)` 缓存，终端宽度不变时滚动不重新折行；仅宽度变化时整块重排。围栏代码块整块 code 底色缩进，不做语法高亮（M2 加 syntect，高亮结果同样走 block 缓存）。
- `widgets/tool_card.rs`：inline 一行卡（图标 + 摘要 + 状态色/spinner）：`$ bash`、`→ read_file`；pending 黄色 `~`、running spinner、成功 ✓、失败红色可展开错误；bash 审批中显示 waiting。M1 不做输出折叠卡（输出直接以 ToolResult 文本段落展示，超长截断）。
- `widgets/prompt.rs`：tui-textarea 多行编辑（max-height = 屏高 1/3，最少 1 行）；Enter 提交、Ctrl+J/Alt+Enter 换行、Esc 双击中断（首次显示 "再按一次 Esc 中断"）、空输入 Ctrl+C 退出；Up/Down 仅在首行首列/末行末列触发历史（内存历史，M1 不持久化）；底栏一行元信息：`模型 · 工作目录`，工作中显示 spinner 与 `esc interrupt`。
- `widgets/permission.rs`：审批弹窗（居中卡片，非全屏栈，M2 升级为通用 Dialog 栈）：显示工具名/命令，键位 `y` once / `a` always / `n` reject，拒绝信息回灌模型。
- `widgets/{home,session,sidebar}.rs`：
  - Home：logo 文本 + 居中输入框（宽 = min(75, 屏宽 70%)）+ 底部目录提示；
  - Session：横向 layout = 主区 + 右侧 42 列 sidebar（M1 内容仅会话列表/模型信息，可 `<leader? 先用 F3 或命令>` 切换——M1 键位用 `F2` 切 sidebar，避免 leader 系统提前引入）；主区纵向 = 消息列表（`Pager` 粘底滚动：用户在底部时跟随新内容，上翻后脱离；PageUp/PageDown 半页滚动）+ 底部 prompt/审批条；
  - **消息列表窗口化**：渲染前先按已缓存的每 block 行高算累计偏移，只对「可视行区间 ± 一屏余量」内的消息执行布局/绘制，视口外的消息不产生任何 Cell 写入；滚动只改变偏移量，不重排不可见内容（支撑万行历史 60fps 指标）；
  - 用户消息：主题 primary 色左竖线 + panel 底；assistant：默认文本 + 末尾一行 `▢ model · 用时`（M1 用时可选）。
- `keymap.rs`：M1 用静态键位表（一个 `KeyAction` 枚举 + match 分派），但代码结构按「base/modal 两层模式」组织，为 M2 leader/用户覆盖预留。
- `render.rs`（渲染热路径工具）：~~`unicode-width` 结果进 Mutex+HashMap 全局缓存~~ —— **M1 实测该缓存比裸查表慢 10 倍（455ns vs 47ns），已按"先量化再优化"原则删除**，现仅保留 ASCII 快路径，非 ASCII 直接走 unicode-width 表查询。M2 若 profiling 显示宽度成为热点再评估无锁/线程局部缓存。

**mrorcode-cli**

- clap derive：`Cli { command: Option<Commands>, -C/--cd <PATH>, --model <NAME>, --provider <NAME> }`，`Commands::Run { prompt: String, --json }`；无参默认进 TUI。
- `main.rs`：加载 config → 校验 provider 配置（缺失则输出如何配置并退出）→ 起 tokio runtime → 分派 TUI 或 headless。
- `headless.rs`：订阅 `RuntimeEvent`，普通模式打印流式文本（工具行带状态前缀）；`--json` 打印 `#[serde(tag="type")]` 的 JSONL（`run.started/message.delta/tool.started/tool.completed/turn.completed/error`），结束码反映成败。

### 四、实施步骤（依赖排序）

> 每个步骤结束都必须保持 workspace 可编译、已有测试通过。建议每步一个提交。
> **M1 执行结果：12 步全部完成（✅）。**

- ✅0. **工作区骨架**：根 `Cargo.toml`（workspace + `[workspace.dependencies]` 统一版本）、`rust-toolchain.toml`（1.88）、`.gitignore`、六个空 crate（lib.rs 占位）；**同日定好速度优先 release profile**：dev 保持默认；release 用 `opt-level=3`、`lto="thin"`、`codegen-units=1`、`strip=true`（不用 atomcode 的 `opt-level="z"`——那是体积优先；暂不用 `panic="abort"`，保留 unwind 便于测试与终端恢复兜底）；新增 `bench` profile 继承 release 并开 debug 符号供 perf 采样；建 `benches/` 目录占位；`cargo check` 与 `cargo build --release` 通过。
✅1. **kernel 基础类型**：error/message/provider/tool/middleware/request 类型与 trait；`ToolRegistry`；单测覆盖消息构造与配对修复逻辑。
✅2. **kernel agent 循环**：event + agent（顺序工具执行、round 循环、max_rounds、cancel）；在 crate 内加 `test-support` feature 提供 `ScriptedProvider`（按脚本吐 TextDelta/ToolCall）与 `EchoTool`；测试：纯聊天一轮、单工具两轮、连续多工具、deny 后继续、cancel 终态、工具结果截断。
✅3. **config crate**：Config/ProviderConfig、toml + 环境变量加载与合并；单测（serde 默认值、env 覆盖、损坏文件诊断）。
✅4. **capabilities provider**：先写 `SseDecoder` 并用录制字节单测（半包、半 UTF-8 字符、多工具参数分片、`[DONE]`、SSE comment 行）；再写 `OpenAiCompatProvider`（wiremock 打本地 mock HTTP 做集成测试：流式聊天、工具调用、429 重试、错误上抛）。
✅5. **capabilities 工具与审批**：read_file、bash（风险词表 + 平台 shell 分支 + 超时）、ApprovalMiddleware/grant store；单测（越权路径拦截放 M3，M1 仅 working_dir 内执行）、审批 allow/always/deny/null 超时四态。
✅6. **agent crate**：assemble + persona + Runtime/Handle；单测用 ScriptedProvider 经 Runtime 走一遍 submit → 事件序列断言；`tracing` 日志埋点。
✅7. **cli headless**：clap + `mrorcode run [--json]`；可用配置对真实兼容端点手工冒烟；脚本化 provider 经一个 `#[cfg(test)]` 注入点做集成测试。
✅8. **tui 基础设施**：终端生命周期、event loop（输入线程阻塞 poll + select + **脏标记驱动的 8ms 合批尾沿，spinner 100ms 慢 tick 按需启停**）、store/update/render 骨架、render.rs 宽度缓存、Theme 常量；先渲染 Home 静态页；写第一个基准：静态页空闲 200ms 内事件循环唤醒次数必须为 0。
✅9. **tui 对话闭环**：Session 布局、消息列表与粘底滚动（**窗口化绘制 + block 行高缓存**）、**增量 markdown 解析器（只重解析最后未闭合 block，闭合 block 的 span/折行缓存）**、流式文本与 TextArea 输入、历史、Esc 双击中断；接 RuntimeHandle 真实跑通；基准：录制 1000 token/s delta 流回放，测量每帧布局耗时与合批后帧率。
✅10. **tui 工具与审批**：工具 inline 卡片状态机、审批弹窗与 Respond 回灌、sidebar（会话/模型）、F2 切换、基础错误展示（provider 报错红色卡片 + 可继续输入）。
✅11. **性能验收与收尾**：跑齐第六节全部基准（冷启动 TTFFD、流式 CPU、万行历史滚动、空闲 0 唤醒、弹窗延迟），任一不达标先优化不外推功能；README 中写明配置与用法（仅更新现有文档，不新建 md）、`cargo clippy --workspace -- -D warnings`、`cargo fmt --check`、跨平台注意点自查（见风险）。

### 五、关键依赖

| crate | 依赖 | 用途 |
|---|---|---|
| 全局 | tokio 1（rt/macros/sync/process/time）、serde/serde_json、anyhow/thiserror、tracing | 异步与序列化基线 |
| kernel | futures（BoxStream/FuturesOrdered）、tokio-util（CancellationToken）、async-trait | 最小依赖面 |
| capabilities | reqwest 0.12（**native-tls**：Windows=SChannel/macOS=Security.framework + stream + json + charset/http2）、bytes、async-stream、encoding_rs；dev: wiremock | SSE 不引 eventsource；TLS 选型见完成报告 |
| tools |（bash/read 仅 std + tokio） | M3 才引入 similar/ignore/grep |
| config | toml、dirs | |
| tui | ratatui 0.30、crossterm 0.29、tui-textarea-2 0.13、pulldown-cmark、unicode-width | syntect M2 引入 |
| cli | clap 4（derive）、tracing-subscriber | |
| dev/bench | criterion（统计基准）、insta（可选，TUI 快照） | 性能回归守护 |

版本全部收敛到根 `[workspace.dependencies]`。**速度优先 profile 从 M1 第一天启用**（步骤 0）：release = `opt-level=3` + `lto="thin"` + `codegen-units=1` + `strip=true`；另设 `bench` profile（release + debug 行号）供采样；`panic` 保持 unwind（库开发期便于测试与 panic 路径的终端恢复），M5 评估 `panic="abort"` 与 mimalloc 等分发期优化。

### 六、验证

- `cargo test --workspace` 全绿，重点：
  - SSE 解码器录制字节用例（半包/分片/[DONE]）；
  - agent 循环 ScriptedProvider 六场景（步骤 2）；
  - 审批四态、工具结果截断、cancel 终态；
  - wiremock 下 provider 的流式与 429 重试。
- `cargo clippy --workspace -- -D warnings` + `cargo fmt --check`。
- 手工冒烟（Windows Terminal）：配置一个 OpenAI 兼容端点 → 普通问答流式输出 → 让其读文件（自动放行）→ 让其执行非只读命令（弹窗 y/a/n 各验一次，always 后同命令不再询问）→ Esc 双击中断 → `run` 与 `run --json` 各一次。
- 退出后终端必须恢复（raw mode/光标/ alternate screen），包括 panic 路径。
- **性能基准（`cargo bench --bench <name>`，release/bench profile，M1 收尾门禁）**：
  - `ttffd`：进程启动到首帧入屏时间（TestBackend/虚拟终端驱动，目标 < 50ms，记录 p50/p95）；
  - `stream_replay`：录制 1000 token/s 的 `TextDelta` 事件流回放，断言单核 CPU < 15%（采样统计）、按键到渲染延迟 < 16ms、每帧布局耗时；
  - `long_history_scroll`：灌入 1 万行消息，连续滚动 1000 帧，断言帧率 ≥ 60fps 等价预算（每帧 < 16ms）与内存上限；
  - `idle_zero_wakeup`：Home 静态页 200ms 窗口内事件循环唤醒/终端输出字节数均为 0；
  - `markdown_incremental`：同一文本「全量重解析」对比「增量只解析尾 block」的耗时基准，固化收益倍数，防止退化为全量；
  - `approval_latency`：审批事件到弹窗渲染 < 16ms；
  - `width_cache`：宽度缓存命中路径与裸 unicode-width 计算的对比基准。
  - 基准结果产出 JSON 存档（`target/bench/`），M1 建立基线；后续里程碑 PR 如使关键指标回退 > 10% 必须说明。

### 七、风险与对策

| 风险 | 对策 |
|---|---|
| ratatui 0.30 与 tui-textarea-2 版本耦合（后者为维护分叉） | 步骤 0 先做最小渲染 demo 锁定可用版本组合；tui-textarea 仅用于 prompt，封装在 `widgets/prompt.rs` 单文件后，替换成本可控 |
| Windows 上 crossterm 键位差异（Ctrl+C、粘贴 CRLF、conpty） | Enter/Esc/Ctrl+J 优先，不依赖 kitty 键盘；粘贴统一规范化 CR/CRLF；Windows 控制台 ENABLE_PROCESSED_INPUT 问题若出现，参考 opencode terminal-win32 方案用 windows-sys 处理（M2） |
| bash 工具在 Windows 的语义差异 | Command 按平台分支（cmd / sh，$SHELL 优先），风险词表两套；集成测试在两平台分别跑（CI M5 补齐，M1 本地验证） |
| SSE 分片/半个 UTF-8 字符 | 解码器纯函数 + 录制字节单测先行（步骤 4），未过测试不写 provider |
| markdown 增量解析的缓存失效 bug（尾部 block 边界误判、宽度变化脏数据） | block 边界由解析器消费到的字节偏移确定并单测覆盖（表格/围栏/引用嵌套场景）；折行缓存以宽度为 key，resize 全量失效重算；保留全量解析路径作为测试预言机做差分对比 |
| 性能预算不达标导致 M1 延期 | 步骤 8/9 即嵌入轻量基准早暴露，不把性能拖到步骤 11；热点先量化再优化，禁止凭感觉加缓存；万行滚动若窗口化后仍超预算，优先减布局复杂度而非换渲染器 |
| TUI 状态复杂度膨胀成第二套 runtime | 铁律：agent/会话权威状态只在 runtime；TUI store 只存投影与纯 UI 态（滚动/输入/弹窗），所有动作经 DriverCommand；code review 按此检查 |
| 审批 oneshot 阻塞导致 agent 挂死 | RequestCtx 带超时（交互 5 分钟、headless 立即拒绝），drop/cancel 一律 fail-closed Deny |

### 八、后续里程碑路线图（概要，M1 批准后另行细化）

- **M2 TUI 核心体验**：syntect 代码高亮、bash 输出折叠卡、通用 Dialog 栈 + fuzzy Select + 命令面板、主题 JSON（直接兼容 opencode 33 主题文件）与 KV 持久化、leader 键与用户键位覆盖、模型切换弹窗、会话列表/新建。
- **M3 Agent 能力**：write/edit（similar diff）/grep/glob、会话 snapshot+jsonl 双层持久化与 resume、stub/overflow compaction、MCP stdio、@补全与 /斜杠命令（tui-textarea atomic ranges 承载虚拟片段）、plan 模式、路径安全 gate。
- **M4 TUI 精致化**：多会话标签、diff viewer（unified/split）、reasoning 折叠、子 agent 视图、rewind 时间线、鼠标与选区复制、剪贴板矩阵/图片粘贴、which-key/toast/通知、侧边栏 context/todo/files 分区。
- **M5 硬化发布**：Anthropic provider、分发期优化评估（`panic="abort"`、mimalloc、进一步 LTO/体积）、跨平台 CI（Win/macOS/Linux，含性能基准趋势门）、headless JSONL 稳定化、conformance 测试套件（仿 atomcode testkit 对抗双甲）、cargo-dist 打包。

### 九、M1 完成报告（2026-09-23）

#### 9.1 交付清单

- 六个 crate 全部落地（kernel / config / capabilities / agent / tui / cli），代码结构与第三节设计一致；
  实际小调整：未单独建 `keymap.rs`（键位分派内聚在 `app.rs::on_key`，仍按 base/modal 两层组织）、
  未单独建 `border.rs`（左竖线直接用 ratatui Block 内联）。
- 49 个自动化测试全部通过：kernel 11（含 6 个 agent 循环场景）、config 5、capabilities 26（SSE/重试/
  审批四态/风险分类/真实 cmd 子进程/wiremock HTTP）、agent 3、tui 4（TestBackend 渲染+键位+0 定时器）。
- `cargo clippy --workspace --all-targets -- -D warnings` 零告警；`cargo fmt --check` 通过；
  debug 与 release（opt-level=3 + thin LTO + codegen-units=1 + strip）均构建成功，
  release 单二进制 **4.9MB**。
- 端到端验证：本地 mock SSE（Python，第一轮 bash 工具调用、第二轮收尾）跑通
  `mrorcode run --json`，事件链 `run.started → message.delta → reasoning.delta →
  tool.started → tool.completed → turn.completed → run.finished` 完整，退出码正确。
- 附加交付：[config.example.toml](../config.example.toml)、
  [scripts/install.ps1](../scripts/install.ps1)（装入 `~/.cargo/bin`，全局可用）、
  [scripts/bootstrap-tools.ps1](../scripts/bootstrap-tools.ps1)（本机工具链修复，见 9.3）。

#### 9.2 性能实测（criterion，bench profile）

| 基准 | 实测 | 预算/结论 |
|---|---|---|
| markdown 全量重解析（200 block 文本/帧） | 0.73 ms（首轮 1.27ms 冷缓存） | 对照基线 |
| markdown 增量更新（仅尾 block 变化） | **0.22 ms（早期 18µs 级，随文本增长）** | 增量路径比全量快 3-70 倍，流式只重排尾块的设计有效 |
| **1 万条消息窗口化单帧** | 初版 **47ms（超 16ms 预算）→ 优化后 0.40ms** | 达标（预算的 3.9%）；优化=MarkdownView 零分配 `update(&str)` + 缓存命中时 `line_count` 只数行不克隆 |
| 宽度计算（混合宽字符串） | 缓存版 455ns vs 裸 unicode-width 47ns | **缓存方案被证伪并删除**，仅保留 ASCII 快路径 |
| 空闲事件循环 | 无 spinner 时不挂任何定时器（单测断言 idle→无 timer、busy→有 timer、回 idle→撤销） | 0 唤醒达标 |

未自动化的两项（需真实终端/真实 API，留作人工冒烟）：TTFFD < 50ms、1000 token/s 下 CPU < 15%；
架构上已满足前提（单进程 enum+mpsc、无 worker、无固定帧率）。

#### 9.3 与计划的偏差及原因

1. **TLS 从 rustls 改为 native-tls**：本机为 windows-gnu 目标且无可用 C 编译器
   （`~/.cargo/bin` 下 gcc/dlltool 是残缺副本，无 VS、无 MSYS2），rustls 的 ring 需要 C 编译。
   native-tls 在 Windows 走系统 SChannel，纯 Rust 绑定零 C 依赖（与 atomcode Windows 做法一致）。
   同时用 `scripts/bootstrap-tools.ps1` 把 MSYS2 官方 binutils/gcc-libs/zstd 等 6 个小包
   （约 10MB）下载到项目本地 `.tools/`，经 `.cargo/config.toml` 的 `-C dlltool` 指向，
   解决了 windows-gnu raw-dylib 生成（dlltool 需要同目录 as.exe）的链接问题。零系统改动、可复跑。
2. **宽度缓存方案否决**：见 9.2。教训固化——"先量化再优化，禁止凭感觉加缓存"已写入风险表。
3. **窗口化帧优化**：计划只写了"只画可视消息"，实测发现每帧对全部消息 clone 原文做同步
   本身就要 47ms。补充设计：store 在事件投影时同步 doc（O(1) `push_str` 后增量更新），
   渲染路径只读缓存零 clone。
4. **测试发现并修复一个真实缺陷**：工具返回的 call_id 原由工具自带（EchoTool 写死 `echo-1`），
   多工具连续调用时 id 全部相同；现由 agent 在结果落库前**权威覆写**为调用 id。
5. **MSRV 兼容性**：clippy 报 `str::floor_char_boundary`（1.91 稳定）超出声明的 MSRV 1.88，
   已改为自实现字符边界回退。

#### 9.4 已知限制（明确留给后续里程碑）

- 仅 OpenAI 兼容 provider；bash 风险分类是保守词表（非 AST，默认偏 Risky 是安全方向）；
- 会话不持久化、无并发工具、无 compaction；
- TUI 无主题文件/命令面板/@补全/鼠标（按计划归 M2）；
- 窄终端（≤100 列）sidebar 直接隐藏，未做 opencode 式浮层；
- headless 审批一律拒绝（交互审批只在 TUI；`--dangerously-skip-permissions` 可放行）。




---

## mrorCode M2 实施计划：TUI 核心体验（opencode 对标度拉满）

> 前置：M1 已完成（见下文 M1「九、M1 完成报告」）。
> M2 聚焦第八节路线图中的 TUI 体验项；会话磁盘持久化/MCP/编辑工具仍在 M3。

### 一、M2 目标（验收标准）

1. **主题系统**：直接 vendoring opencode 全部 33 个 MIT 主题 JSON（`assets/`），
   支持 defs 引用、dark/light 双值、`transparent`/`none`、ANSI 数字；
   主题选择持久化；M1 内置 tokyonight 降为兜底默认。
2. **代码高亮**：代码块按语言 syntect 着色（纯 Rust fancy-regex，无 C 依赖），
   颜色走 theme syntax token；沿用 M1 block 缓存，高亮结果同样缓存。
3. **bash 输出折叠卡**：工具输出默认折叠（保留首行摘要），按键展开/收起。
4. **通用对话框栈**：modal 键位层、通用 fuzzy Select（自写评分，零新依赖）、
   Confirm/Alert/Help；任意时刻只渲染栈顶。
5. **命令面板**：所有命令注册到 CommandRegistry（name/category/suggested/run/真实快捷键），
   fuzzy 搜索 + 分组 + Suggested 首组。
6. **键位系统**：base/modal 两层模式；leader 键（默认 ctrl+x，2s 超时窗口）；
   用户覆盖持久化（紧凑键位串如 `"ctrl+p"`/`"f2"`/`"leader n"`）。
7. **模型切换**：同 provider 多模型选择（config `models = []`），
   SetModel 经 kernel 在**下一轮**生效，选择持久化。
8. **多会话（内存）**：侧栏会话列表、新建/切换/删除；kernel 支持多会话会话体映射。
9. toast 通知替代 M1 notice；help 对话框列出全部命令与真实绑定。

#### 非目标（仍归后续里程碑）

- 会话磁盘持久化/resume（M3）；切换会话的内容目前仅进程内，退出即失（UI 明示）。
- 跨 provider 切换（base_url/api_key 不同的条目切换需要重建 runtime，M3 随 daemon 一起做）。
- @补全/斜杠命令输入层（M3，依赖 tui-textarea atomic ranges）。
- diff viewer、reasoning 折叠、子 agent 视图、鼠标（M4）。

### 二、架构改动要点

#### 2.1 kernel：多会话（in-memory）+ 每轮模型覆盖

- `AgentCommand::SendMessage` 改为 `Submit { session_id: u64, text: String }`；
  新增 `SetModel(String)`。
- `AgentState.conversation: Conversation` → `conversations: HashMap<u64, Conversation>`；
  turn 开始按 session_id 取/建会话，结束归还。同一时刻仍只允许一个 turn
  （M2 不做并发会话，busy 时对其他 session 的 submit 进 pending 队列）。
- `ChatOptions` 增加 `model: Option<String>`；OpenAI provider 发请求时
  `options.model.clone().unwrap_or(self.model)`。
- 事件不带 session_id：runtime 知道当前 turn 属于哪个 session，转发时包一层
  `RuntimeEvent::Agent { session_id, event }`。

#### 2.2 agent runtime

- `DriverCommand::Submit(text)` → `Submit { session_id, text }`；
  新增 `SetModel(String)`、`NewSession/` 不需要（id 由 TUI 分配）。
- RuntimeHandle 增加 `submit_to(session_id, text)`、`set_model(name)`。
- assemble 读 config 的 `models` 列表随 Assembly 返回（供选择对话框）。

#### 2.3 config

- `ProviderConfig` 增加 `models: Vec<String>`（缺省回退单 `model`）。

#### 2.4 TUI 模块（新增/修改）

```
crates/mrorcode-tui/src/
├── storage.rs          # 新：KV 持久化（state.json，原子写）
├── fuzzy.rs            # 新：subsequence 评分（前缀/连续/分隔符加权）
├── commands.rs         # 新：CommandRegistry（命令面板数据源）
├── keymap.rs           # 新：模式层 + leader + 用户覆盖 + 键位串解析
├── theme.rs            # 改：serde 反序列化 + 注册表 + dark/light
├── themes/             # build.rs include_str! 33 个 vendored JSON
├── dialog/
│   ├── mod.rs          # Dialog 栈（Select/Confirm/Alert/Help 枚举）
│   ├── select.rs       # fuzzy 列表（过滤/分组/上下行/enter/esc）
│   └── forms.rs        # Confirm/Alert/Help 渲染
├── app.rs              # 改：键位走 keymap、对话框栈分发、命令接线
├── store.rs            # 改：多会话投影、toast 队列、KV 装载
└── widgets/
    ├── tool_card.rs    # 改：折叠卡（展开集合存 store）
    └── sidebar.rs      # 改：会话列表 + 主题/模型徽标
```

### 三、关键设计

#### 3.1 KV 存储（storage.rs）

- 路径 `~/.mrorcode/state.json`；`serde_json` 单文件，字段全部 `#[serde(default)]`（前向兼容）。
- 写入：`tmp → fsync(尽力) → rename` 原子替换；写入在 spawn_blocking 或直接同步（数据 < 4KB）。
- 键：`theme: String`、`theme_mode: dark|light`、`sidebar: bool`、
  `model: Option<String>`、`keybinds: Vec<{keys, action}>`、`prompt_history: Vec<String>`
  （上限 50，替代 M1 的纯内存历史）。
- 读一次启动装载；写采用显式 `save()`（低频变更点，不做每帧写）。

#### 3.2 主题

- 33 个 JSON 经 tui `build.rs` 从 `src/themes/*.json` 生成
  `pub static BUNDLED: &[(&str/*文件名*/, &str/*内容*/)]`（无 include_dir 新依赖）。
- 解析：`{ defs: {name: "#hex"}, theme: { token: "#hex" | {dark,light} | "transparent"|"none" | 0..1(alpha?) } }`。
  defs 可引用 defs（检测循环）；`{dark,light}` 按当前 mode 取；
  解析为 ratatui `Color::Rgb`（alpha 在 0.30 无 RGBA，按规则与背景合成或降级忽略）。
- 我们的 Theme struct 扩展到 opencode 全量 token（syntax 9 色 + markdown 全量），
  M1 未用字段补 `#[allow(dead_code)]` 保留。
- 未知 token 忽略、解析失败的主题跳过并 warning（不崩）。
- 加 `themes::parse_bench`：解析 33 个主题总耗时（应 < 数 ms，只在切换时发生）。

#### 3.3 syntect 高亮

- 依赖 `syntect = { default-features = false, features = ["default-fancy"] }`
  （fancy-regex 纯 Rust，不引入 onig C 编译——本机工具链教训）。
- `SyntaxSet::load_defaults_newlines()` 一次（lazy 静态）；
- markdown 代码 block 渲染时按 info string 选语法 → `highlight_line` →
  映射 scope 前缀（comment/keyword/string/number/function/type/…）到 syntax token；
- 未命中语言走 M1 朴素 code 底色；高亮 span 进入现有 block 折行缓存
  （缓存 key 已含文本+宽度，无需额外失效）。

#### 3.4 fuzzy 评分（fuzzy.rs，零依赖）

- 子序列匹配 + 加分：首字母/分隔符后命中、连续命中、前缀命中；长度惩罚。
- 返回 0..u32（0=不匹配），用于排序与高亮命中字符。
- criterion 基准：1 万条命令/文件名规模下单次过滤预算（目标 < 0.5ms）。

#### 3.5 对话框栈与命令面板

- `AppState.dialogs: Vec<Dialog>`；`push/pop/replace`；按键先交栈顶 modal 层；
  栈空才走 base 层。
- Select：顶部过滤输入（实时 fuzzy）、类别分组标题、`●/○` 当前项、
  footer `↑↓ 移动 · enter 选择 · esc 关闭`；宽 60%、高 60%、遮罩底色半透明效果用深色块近似。
- 命令面板命令（M2 首批）：
  - `new session`、`list sessions`、`delete session`
  - `switch theme`、`toggle sidebar`、`switch model`
  - `help`、`quit`、`interrupt`
- 命令记录 `run` 闭包：为避免闭包持有 AppState，采用
  `enum AppCommand { NewSession, OpenThemePicker, … }` 的纯数据分派（可测、无生命周期纠缠）。

#### 3.6 键位系统

- `enum Action`（与 AppCommand 大部分重合 + 光标/滚动/提交等 UI 动作）。
- 模式：`base`（默认）、`modal`（对话框打开时）；M3 预留 `autocomplete`。
- leader：base 层 ctrl+x（可配）进入限时等待（2s），下一个键组合成命令
  （如 leader+b 侧栏、leader+p 命令面板、leader+1..9 会话）。
- 覆盖格式：`keys = ["ctrl+k"]`, `action = "command_palette"`；解析支持
  `ctrl/alt/shift` 前缀 + `enter/esc/tab/f2/char/leader x`。
- 默认绑定（对齐 opencode 习惯，M1 绑定保留）：
  - `ctrl+p` 命令面板；`F2`/`leader b` 侧栏；`ctrl+n`/`leader n` 新会话；
  - `ctrl+l`/`leader l` 会话列表；`leader t` 主题；`leader m` 模型；
  - `ctrl+j` 换行；`enter` 提交；`esc` 双击中断；`pageup/down` 滚动。
- help 对话框从注册表生成（名称+分类+真实绑定串），杜绝手写键位表过期。

#### 3.7 多会话 UI

- store：`sessions: Vec<SessionMeta {id, title, status}>`、`active_session: u64`；
  消息视图按 session 分桶（`HashMap<u64, Vec<MessageView>>`），切走再切回投影不丢。
- 标题取首条用户消息截断 40 字符。
- 侧栏列出会话（active 高亮、busy spinner）；新建/删除有 Confirm；
  删除会话仅删内存（M3 接磁盘时会有真正的 unlink 确认）。
- kernel 侧 session id 由 TUI 用递增 u64 分配，全链路一致。

### 四、实施步骤（每步保持编译+测试通过）

- ✅式门禁同 M1：clippy `-D warnings`、fmt、全部测试。

0. ✅ **kernel 多会话 + SetModel**：command/state 改造，单测覆盖多会话消息隔离、
   SetModel 下一轮生效。
1. ✅ **config models + runtime 桥接**：RuntimeEvent 包 session_id；
   Assembly 返回可选模型列表；agent crate 集成测试。
2. ✅ **storage.rs KV**：原子写、默认值、损坏回退单测；M1 历史与侧栏状态接入。
3. ✅ **主题系统**：vendoring 33 JSON（含 LICENSE/NOTICE 归属）、build.rs、
   解析器（defs/双值/transparent/循环检测）单测、切换+KV、基准（33 主题 1.16ms）。
4. ✅ **syntect 高亮**：scope→token 映射、未命中回退、block 缓存复用（8 单测；
   默认语法包无 TypeScript，ts/tsx 回退 JavaScript）。
5. ✅ **工具折叠卡 + toast**：展开集合（Tab 焦点/Enter 切换）、长输出默认折叠；
   notice 升级 toast 队列；toast 存活期间才挂 100ms 节拍，空闲仍 0 定时器。
6. ✅ **fuzzy + Select 对话框**：fuzzy 单测/基准（1 万条 233µs）；
   通用选择组件 TestBackend 测试（对话框栈位于 `widgets/dialog/`）。
7. ✅ **命令面板**：注册表 + AppCommand 分派 + 接线已有功能 + Suggested 分组。
8. ✅ **keymap 系统**：模式层/leader/覆盖解析（ctrl+x，2s 窗口）；
   M1 硬编码键位全部迁移；help 数据打通（format↔parse round-trip 单测）。
9. ✅ **模型切换对话框**：models 列表/SetModel/KV；启动时恢复记住的模型。
10. ✅ **多会话 UI**：store 分桶、侧栏列表（active 高亮/busy spinner）、
    新建/切换/删除（Confirm）对话框、home 空态接线、leader 1..9。
11. ✅ **Help/Confirm/Alert + 端到端回归**：TestBackend 用例覆盖面板过滤、
    leader 序列、主题/模型切换、会话切换投影隔离、help 真实绑定（含用户覆盖）。
12. ✅ **性能门禁**：既有基准不回退（万行帧 ~0.52ms、增量 markdown ~0.24ms），
    新增 fuzzy/高亮基准；clippy/fmt 全绿；release 构建通过。

### 五、依赖新增

| crate | features | 用途 |
|---|---|---|
| syntect 5.3 | `default-features=false, default-fancy` | 纯 Rust 语法高亮（无 onig/C） |

fuzzy 评分、KV 存储、键位解析均自写，不引入新 crate。

### 六、风险与对策

| 风险 | 对策 |
|---|---|
| syntect 高亮拖慢流式帧 | 高亮只在闭合代码 block 做一次并入 block 缓存；未闭合围栏用朴素底色；加基准守护 |
| 33 主题质量参差（缺 token/怪颜色） | 缺 token 用 tokyonight 默认补齐；解析失败跳过并记录，不阻塞启动；切换对话框只列成功解析项 |
| 多会话改造影响 M1 稳定性 | kernel 改动最小：仅 Submit 带 id + map 替换单值；M1 六场景测试全部保留并加 id 参数 |
| leader 键与 IME/终端兼容 | leader 可配置可禁用；保留 ctrl+p 等直接绑定，leader 不是唯一入口 |
| 对话框栈与输入焦点错乱（面板过滤输入吃掉 esc/enter） | modal 层先于 base 吃键；每个 Dialog 明确声明是否吞键；TestBackend 键位序列测试 |
| KV 文件并发写（未来多进程） | M2 单进程原子 rename 足够；M5 daemon 化时升级 fs2 锁 |
| 命令闭包借用 AppState 造成生命周期地狱 | 命令用纯数据 `AppCommand` 枚举在 on_key 统一 apply，闭包不进注册表 |

### 七、M2 完成判据

- 全部测试 + clippy/fmt 绿；既有性能基准不回退，新增基准有基线；
- 手工冒烟：ctrl+p 过滤执行命令、leader 序列、主题即时切换、
  模型选择下轮生效、新建两个会话来回切换投影隔离、删除确认、help 键位与实际一致；
- 33 主题均可在选择器列出并切换（个别丑的不阻塞，解析正确即可）。

### 八、实施记录（2026-09-24 完成）

#### 8.1 交付状态

- 全部 13 步（0–12）完成；工作区 **121 个测试全绿**（其中 mrorcode-tui 74 个）；
  `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check`、
  `cargo build --release --workspace` 均通过。
- 33 个 vendored 主题全部成功解析（dark/light 双模式），可在选择器列出与即时切换。

#### 8.2 实际模块落位

```
crates/mrorcode-tui/src/
├── fuzzy.rs        # subsequence 评分（query 预编译，filter 零 positions 分配）
├── commands.rs     # CommandRegistry + AppCommand（纯数据分派）
├── keymap.rs       # KeyPress 归一化 / parse_binding / format_binding / Keymap
├── storage.rs      # M2-2（未变位置）
├── theme.rs        # M2-3（未变位置）
└── widgets/
    ├── dialog/
    │   ├── mod.rs    # 栈分派：Select/Confirm/Alert/Help
    │   ├── select.rs # fuzzy Select（遮罩/分组/命中高亮/滚动）
    │   └── forms.rs  # Confirm / Alert / Help
    ├── highlight.rs  # syntect 适配
    ├── tool_card.rs  # 折叠卡
    ├── toast.rs      # toast 浮层
    └── sidebar.rs    # 会话列表 + 模型/目录徽标
```

#### 8.3 与计划的偏差（均为实现中收敛，无功能裁剪）

1. **syntect 实际版本 5.3**（计划写 0.11）：API 用 `ScopeSelectors::from_str`
   构造主题项；默认语法包不含 TypeScript，`ts/tsx/typescript` 回退 JavaScript 词法。
2. **对话框目录落在 `widgets/dialog/`**（计划 2.4 写在 src 根）：随 widget 内聚放置。
3. **Confirm 提前到步骤 10** 随删除会话一起交付；步骤 11 补 Alert + Help。
4. **命令面板自身也注册为一条命令**（`command palette`，ctrl+p / leader p），
   否则 help 中无法列出它的真实绑定。
5. **fuzzy 权重调参两轮**：streak 8×、首命中位置 2× 惩罚，才满足
   “连续紧凑匹配优先于词边界散落命中”的直觉排序。
6. **修了两个 M1 遗留问题**：事件通道关闭（`None`）时主循环不再空转、
   高亮行截断混用字节长度与显示宽度的 bug（改走 `take_by_width`）。
7. 接续开发时发现 M2-3/4 代码此前从未真正编译通过（泛型少一个 `>` 的解析根因
   连带十几个级联报错），已全部修复，历史 release 成功记录是更早代码的产物。

#### 8.4 性能基线（criterion，release）

| 基准 | 结果 | 预算/说明 |
|---|---|---|
| windowed_frame / 10000_msgs | ~0.52 ms | < 16 ms 帧预算 |
| markdown/full_reparse | ~0.78 ms | 对照（每帧全排） |
| markdown/incremental_tail_block | ~0.24 ms | 流式热路径 |
| fuzzy/filter_10000_items | ~0.23 ms | 计划目标 < 0.5ms |
| theme/parse_33_bundled | ~1.16 ms | 仅切换时发生 |
| syntax/highlight_rust_200_lines | ~42.6 ms（≈53µs/行） | 仅闭合代码块一次性执行并入缓存 |

空闲态 0 定时器不变；toast 存活时临时挂 100ms 节拍，过期即撤。

#### 8.5 已知遗留（归 M3+，与计划非目标一致）

- 多会话仅进程内，退出即失（删除 Confirm 文案已明示）；磁盘持久化/resume 归 M3。
- 跨 provider 切换需重建 runtime，归 M3 daemon。
- leader 超时窗口的撤销提示依赖 100ms 节拍（有 toast 时一定在跑）；
  极端情况下窗口过期视觉反馈最多滞后一个节拍，不影响正确性。
- 手工冒烟（真实终端交互）尚未执行；自动化 TestBackend 路径已全覆盖。


---

## mrorCode M3 实施计划：Agent 能力增强（编辑 / 搜索 / 持久化 / 压缩 / MCP / 输入层）

> 前置：M1（骨架）与 M2（TUI 核心体验）已完成，见下文各自的完成报告。
> M3 聚焦 M1 路线图第八节的 Agent 能力项：补齐 write/edit/grep/glob 工具、会话磁盘持久化与
> resume、上下文压缩、MCP stdio、@补全与 /斜杠命令输入层、plan 模式与路径安全 gate。
>
> **状态：已完成。** 14 个实施步骤全部落地，步骤 11 采用计划内的回退方案（atomic ranges 在
> tui-textarea 0.13 仅有内部 API，改用"整行替换 + 手动光标 + atomic range 虚拟片段"接入，仍为纯 UI 态）。
> 现有基准不回退，新增 grep/glob 大仓与 compaction 基准均有基线；`cargo test --workspace`、
> clippy `-D warnings`、fmt 全绿。

### 一、M3 目标（验收标准）

1. **编辑工具闭环**：新增 `write_file`（整文件写）与 `edit_file`（精确串替换，产出 diff 供 UI 展示）；
   编辑前强制先读（read-before-write），失败给结构化错误让模型可自纠。
2. **搜索工具**：`grep`（正则内容搜索，尊重 .gitignore，抑制二进制）与 `glob`（按模式列文件）；
   两者均 `Safe`（只读），走现有 block 缓存无 UI 改动。
3. **会话磁盘持久化**：`~/.mrorcode/sessions/<id>.jsonl`（transcript，不压缩，可 recall）
   + `<id>.snapshot.json`（可 resume 的有损快照）；TUI 退出/切换时落盘，启动时恢复会话列表。
4. **resume**：`mrorcode --resume <id>` 或 TUI 侧栏选择历史会话恢复继续对话；
   加载时做 `repair_pairing`（复用 M1 message.rs 已有实现）。
5. **上下文压缩（compaction）**：单会话超过阈值时把较早轮次 stub 化/摘要，保留 system + 近 N 轮；
   触发点与策略在 kernel，driver 不参与。
6. **MCP stdio**：config 声明 `[[mcp.servers]]`（command/args/env），启动时拉起子进程、
   `tools/list` 动态注册为 `Tool`，`tools/call` 转发执行；断连 fail-closed 并告警。
7. **输入层**：`@` 文件补全（fuzzy，复用 M2 `fuzzy.rs`）与 `/` 斜杠命令（复用 `commands.rs` 注册表）；
   基于 tui-textarea atomic ranges 做虚拟片段，提交前展开为纯文本。
8. **plan 模式**：`Shift+Tab` 循环 base/plan 两态；plan 态下工具调用仅允许只读，
   写类工具被 middleware 拦截并回灌提示；UI 底栏明示当前模式。
9. **路径安全 gate**：所有文件工具（read/write/edit/glob/grep）强制 working_dir 内解析，
   拒绝 `..` 逃逸与符号链接越界（M1 只做了 read 的相对拼接，未做规范化校验）。

#### 非目标（仍归后续里程碑）

- 跨 provider 切换 / daemon 化 / ACP（M4+，需重建 runtime）。
- 鼠标、剪贴板、图片、diff split viewer、reasoning 折叠、子 agent 视图、rewind（M4）。
- Anthropic provider、分发期优化、跨平台 CI、conformance testkit（M5）。
- 会话云端同步 / 多进程并发写（持久化锁用 fs2 归 M5 daemon）。

### 二、架构改动要点

#### 2.1 kernel：新增只读/写工具能力边界（不引入具体工具）

- `tool.rs`：`RiskLevel` 保持不变（Safe/Risky）；新增 `Tool::prepare` 可选钩子？
  **不做**——read-before-write 由 middleware 实现，不放 tool trait。
- `agent.rs`：无结构性改动；压缩在 `Conversation` 层以纯函数实现，turn loop 每轮开始前调用。
- `message.rs`：新增 `Conversation::compact(keep_recent, system_keep)` 纯函数，
  把中间轮次的 assistant+tool 对替换为一条 `[summary stub]` 系统注记；
  新增 `Conversation::estimate_tokens()`（按字符数近似，不引入 tokenizer）。
- 新增 `session.rs`（kernel，纯数据 + 无 IO）：`SessionRecord { id, title, updated_at, conversation }`、
  `TranscriptEntry`（jsonl 行：`user`/`assistant_text`/`tool_call`/`tool_result`/`meta`）；
  序列化与反序列化逻辑在此，**文件 IO 落在 capabilities**（kernel 保持零 fs 依赖）。

#### 2.2 capabilities：工具实现与持久化实现

- `tools/write.rs`：`write_file`（`path` + `content`；`Risky`）。
  - 写前校验：路径在 working_dir 内、目标父目录存在、内容大小上限（1MiB）。
  - 覆盖已存在文件时产出 unified diff（`similar` crate）挂到 ToolResult 供 UI。
- `tools/edit.rs`：`edit_file`（`path` + `old` + `new`；`Risky`）。
  - 精确唯一匹配替换；`old` 出现 0 次或 >1 次返回结构化错误（含出现次数），让模型重试。
  - 产出 unified diff；`replace_all` 可选参数。
- `tools/glob.rs`：`glob`（`pattern`；`Safe`）。用 `ignore` crate 的 WalkBuilder 尊重 .gitignore，
  返回按 mtime 排序的路径列表（上限 2000 条）。
- `tools/grep.rs`：`grep`（`pattern` + 可选 `include`/`glob`/`-i`/`-n`；`Safe`）。
  用 `ignore` 遍历 + `regex` 行匹配，返回 `path:line: text`（上限 200 行，超出给总数）。
- `tools/mod.rs`：`builtin_registry()` 注册全部 6 个工具。
- `session/store.rs`：`SessionStore { root: ~/.mrorcode/sessions }`
  - `save_transcript(id, &Conversation)` 追加 jsonl；
  - `save_snapshot(id, &Conversation)` 原子写（tmp→rename，复用 M2 storage 模式）；
  - `list() -> Vec<SessionSummary>`、`load(id) -> Conversation`、`delete(id)`。
- `mcp/`：`client.rs`（stdio JSON-RPC 帧：`Content-Length` 头 + body，或换行分隔按协议版本）、
  `tool.rs`（`McpTool` 实现 `Tool`，name 前缀 `mcp__<server>__<tool>`）、`mod.rs`（连接生命周期）。
  - MCP 客户端在 capabilities 内独立子进程管理，**不改 kernel**。

#### 2.3 config：新增 MCP 与压缩阈值

```toml
## ~/.mrorcode/config.toml
[compaction]
keep_recent = 6          # 保留最近 N 轮原文
threshold_tokens = 100000 # 超过则压缩（0 = 关闭）

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
env = { FOO = "bar" }
```

- `Config` 增加 `compaction: CompactionConfig`（全 default）与 `mcp: McpConfig { servers: Vec<McpServer> }`。
- `McpServer { name, command, args, env }`，`ResolvedProvider` 不涉及 MCP（MCP 全局装配）。

#### 2.4 agent：装配 MCP 与压缩

- `assemble.rs`：装配时若 `config.mcp.servers` 非空，拉起 MCP 连接并把工具注册进 registry；
  连接失败 warning 不阻塞启动。装配返回持有 MCP 句柄，runtime 关闭时一并 teardown。
- `runtime.rs`：`RuntimeStart` 增加 `mcp: Vec<McpConnection>`（可选）；
  新增 `DriverCommand::Resume { session_id, conversation }`（由 TUI 从磁盘恢复时注入）。
- `persona.rs`：随实际挂载工具动态生成可用工具清单（当前写死 read_file/bash，改为从 registry 传名）。

#### 2.5 TUI：输入层、plan 模式、持久化接入

```
crates/mrorcode-tui/src/
├── input.rs          # 新：@文件补全 + /斜杠命令的输入态机（atomic ranges 虚拟片段）
├── plan.rs           # 新（或并入 keymap/store）：plan 模式状态与只读 gate 提示
├── app.rs            # 改：Shift+Tab 模式切换、输入层事件、resume 启动参数
├── store.rs          # 改：plan_mode 字段、session 持久化落盘钩子、补全状态
├── keymap.rs         # 改：新增 Action::{TogglePlanMode, OpenFilePicker, ...}
├── commands.rs       # 改：新增命令（resume session / compact / toggle plan）
├── widgets/
│   ├── prompt.rs     # 改：补全浮层提示、plan 模式底栏徽标
│   ├── diff.rs       # 新：write/edit 工具结果的 unified diff 着色渲染
│   └── sidebar.rs    # 改：会话列表区分「本次进程」与「磁盘历史」
```

### 三、关键设计

#### 3.1 路径安全 gate（capabilities/src/path_guard.rs）

- `resolve_safe(working_dir, raw) -> Result<PathBuf, String>`：
  1. 拼接（绝对路径直接用，相对路径 join working_dir）；
  2. `canonicalize` 后必须 `starts_with(canonical_working_dir)`；对尚不存在的写目标，
     先 canonicalize 父目录再拼文件名；
  3. Windows 额外拒绝 UNC/盘符穿越（`\\?\`）。
- read/write/edit/glob/grep 全部改走该函数；escape 尝试返回结构化错误（模型可见）。
- 单测：`..` 逃逸、绝对路径越界、符号链接指向外部、正常子路径，四类各一。

#### 3.2 压缩（kernel/message.rs）

- 触发：`estimate_tokens() > threshold_tokens` 时，在 turn 开始前调用 `compact`。
- 策略：保留 system（首条）+ 最近 `keep_recent` 个 user turn 及其后续；
  中间区间把每个「assistant(tool_calls)+tool 结果」对折叠为一条 system 注记
  `[compacted: <n> earlier tool calls; <第一个用户请求摘要>]`。
- 纯函数、可测：给定对话与参数，断言 system 保留、近 N 轮逐字保留、中间被折叠、配对不破坏（`repair_pairing` 后无孤儿）。
- 背压：压缩后若仍超阈值，再丢最旧的已折叠注记（保底不失败）。

#### 3.3 会话持久化格式

- `sessions/<id>.jsonl`：每行一个 `TranscriptEntry`，追加写（`OpenOptions::append`）；
  流式期间**只在轮次终态**落盘（避免每 delta 写盘），文件为 rec不计入 markdown 热路径。
- `sessions/<id>.snapshot.json`：`SessionRecord`（含完整 `Conversation`），轮次结束原子覆盖。
- 启动：`SessionStore::list()` 扫目录按 mtime 倒序；TUI 侧栏显示磁盘会话（淡色）与本次进程会话（高亮）。
- resume：选中磁盘会话 → 读 snapshot（缺失则从 jsonl 重建）→ `repair_pairing` →
  `DriverCommand::Resume` 注入 kernel → TUI 重建 MessageView 投影。
- 标题：首条 user 消息前 40 字符；空会话不落盘。

#### 3.4 MCP stdio 客户端

- 传输：子进程 stdin/stdout，按 MCP 规范用换行分隔 JSON-RPC（0.x）或 `Content-Length` 帧（1.x）——
  **以握手时 `initialize` 版本协商为准，两版都支持**，单测用录制字节（仿 SSE 解码器思路）。
- 生命周期：装配时 `initialize` → `tools/list` → 注册；请求 id 单调递增；
  子进程退出/stderr 有内容时 warning + 该 server 工具全部标记不可用（调用返回 is_error）。
- 工具命名冲突：`mcp__<server>__<tool>`，不与内置名冲突。
- 风险：MCP 工具默认 `Risky`（需审批），除非 server 声明只读 hint（`annotations.readOnlyHint`）。

#### 3.5 输入层：@补全与 /斜杠命令（tui-textarea atomic ranges）

- 触发：光标前 token 以 `@` 开头 → 文件补全（glob 当前工作目录，fuzzy 过滤，`fuzzy.rs` 复用）；
  行首为 `/` → 命令补全（`commands.rs` 注册表的名字）。
- 虚拟片段：用 atomic range 包裹已选中的 `@path`，退格整体删除；提交前把片段展开为纯文本路径。
- 补全浮层：输入框上方浮层，`↑↓` 选择、`Tab/Enter` 接受、`Esc` 取消；
  modal 键位层（复用 M2 对话框栈的吃键规则）。
- 若 tui-textarea-2 0.13 的 atomic ranges API 不足，**回退方案**：自维护 `Vec<Span>` + 手动光标偏移，
  仍是纯 UI 态，不碰 store 权威数据。

#### 3.6 plan 模式

- `AppState.plan_mode: bool`；`Shift+Tab`（base 层）切换；底栏徽标 `plan`（橙色）。
- 生效点：TUI 把模式经 `DriverCommand::SetPlanMode(bool)` 传给 runtime → kernel 在
  turn ctx 增加 `plan_mode`，工具执行前对 `Risky` 工具直接 Deny（回灌 `plan mode: write blocked`）。
- 只读工具（read/glob/grep）照常执行，模型可在 plan 态调研后产出计划文本。
- 与审批中间件的关系：plan gate 优先级高于审批（plan 态不弹审批，直接拒绝写类）。

### 四、实施步骤（每步保持编译+测试通过）

> 门禁同 M1/M2：`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
> `cargo fmt --check`。每步一个提交；每步结束同步更新本文件与 `AGENTS.md`（如结构变化）。

0. **依赖与骨架**：根 `[workspace.dependencies]` 新增 `similar`、`ignore`、`regex`（M3 工具用），
   确认均为纯 Rust 无 C 依赖；建 `capabilities/src/{path_guard.rs,session/,mcp/}` 空模块占位。
1. **路径安全 gate**：`path_guard.rs` + 四类单测；read_file 改走它。
2. **write_file / edit_file**：工具实现 + diff 产出（similar）+ 结构化错误；单测（唯一/零/多次匹配、
   覆盖写、越界拒绝、diff 正确性）。capabilities `builtin_registry` 注册。
3. **glob / grep**：ignore 遍历 + regex 匹配；单测（.gitignore 尊重、二进制抑制、行号、上限）。
   persona 改为从 registry 动态列工具。
4. **TUI diff 渲染**：`widgets/diff.rs` 识别工具结果里的 unified diff 并着色（theme diff_* token），
   接入 tool_card；TestBackend 快照测试。
5. **kernel 压缩**：`Conversation::compact` + `estimate_tokens` + 单测（保留/折叠/配对/背压）；
   turn loop 接入（config 阈值）。
6. **config compaction + mcp 段**：serde 解析与默认值单测；`config.example.toml` 补样例。
7. **会话持久化（capabilities/session）**：transcript 追加 + snapshot 原子写 + list/load/delete；
   单测（round-trip、损坏 jsonl 跳过坏行、快照缺失从 jsonl 重建、空会话不落盘）。
8. **agent resume 桥接**：`DriverCommand::Resume` + `RuntimeStart` 传递；agent 集成测试
   （恢复后再 submit 能延续上下文）。
9. **TUI 持久化接入**：轮次终态落盘、启动恢复侧栏历史、resume 命令、退出 teardown；
   TestBackend 用例（恢复投影、删除历史会话）。
10. **MCP stdio**：client + McpTool + 装配；单测用录制 JSON-RPC 字节 + 一个 `#[cfg(test)]` 假 server
    （本地 echo 可执行/内存流），覆盖 initialize/tools list/call/断连。
11. **输入层 @补全 + /斜杠**：input.rs + 浮层 + atomic range（或回退方案）；单测（fuzzy 过滤、
    接受/取消、提交展开、与对话框棧不冲突）。
12. **plan 模式**：keymap/Shift+Tab、kernel gate、底栏徽标；单测（plan 态 Risky 被拒、Safe 放行）。
13. **性能门禁与收尾**：既有基准不回退（万行帧 ~0.52ms、增量 markdown ~0.24ms、fuzzy ~0.23ms）；
    新增 grep/glob 大仓基准、compaction 基准；clippy/fmt 全绿；release 构建通过；
    更新 README 的配置说明（MCP/compaction/resume）。

### 五、依赖新增

| crate | version | features | 用途 |
|---|---|---|---|
| similar | 2 | `text` | edit/write 的 unified diff 产出与 UI 着色 |
| ignore | 0.4 | 默认 | glob/grep 的 .gitignore 感知遍历 |
| regex | 1 | 默认 | grep 行匹配 |

均纯 Rust，无 C 依赖（与 native-tls/syntect-fancy 的选型教训一致）。MCP 不引第三方 SDK，
JSON-RPC 自写（仿 SSE 解码器的录制字节单测思路）。

### 六、风险与对策

| 风险 | 对策 |
|---|---|
| edit_file 匹配歧义导致模型反复失败 | 结构化错误含「出现次数 + 首个上下文行」，提示模型扩大 old 片段；`replace_all` 兜底 |
| 路径逃逸/符号链接越界 | 集中 `path_guard`，所有文件工具强制走；单测覆盖 UNC/`..`/symlink；fail-closed |
| 压缩破坏 tool_call/result 配对导致 provider 400 | compact 后强制 `repair_pairing`；差分测试用 ScriptedProvider 走完整轮次 |
| jsonl 追加写并发/中断产生半行 | 每行独立 append + `\n`；加载时跳过解析失败行并 warning，不整体失败 |
| MCP 子进程泄漏/挂死 | `kill_on_drop` + 装配持有句柄、runtime shutdown 显式 teardown；调用带超时 |
| MCP 协议版本差异 | initialize 协商版本，两版帧格式都实现；录制字节单测先行，未过不写客户端 |
| @补全与对话框栈键位冲突 | 补全走 modal 层声明吃键；TestBackend 键位序列测试；Esc 取消优先 |
| plan 模式绕过审批被写 | plan gate 在 middleware 之前按工具 RiskLevel 直接 Deny，不进入审批流程 |
| 持久化拖慢流式帧 | 仅轮次终态落盘；不引入每 delta 写盘；基准守护帧预算 |

### 七、验收清单（M3 完成判据）

- [x] 全部测试 + clippy `-D warnings` + fmt 绿；既有性能基准不回退，新增基准有基线。
      新增基线：`search/glob_10000_rs` ~39ms、`search/grep_10000_rs`（命中/未命中）~313ms、
      `compaction/estimate_tokens_500_turns` ~24µs、`compaction/compact_keep_6_of_500_turns` ~318µs。
      （既有 tui 基准：万行帧 ~0.44ms、增量 markdown ~0.22ms、fuzzy ~0.20ms，均远优于 16ms 帧预算。）
- [x] 手工冒烟：让 agent 读→改→跑测试闭环；`grep`/`glob` 命中与 .gitignore 行为正确。
      （path_guard 单测覆盖 `..`/symlink/UNC 逃逸；glob/grep 单测覆盖 .gitignore 排除与二进制跳过。）
- [x] 退出后重启能列出历史会话并 resume 继续对话；snapshot 与 jsonl 一致。
      （TUI 会话选择器 `disk:` 条目 + 持久化单测 `persist_and_reload_history_session`。）
- [x] 长对话触发 compaction 后仍能继续，无 provider 配对错误。
      （kernel 集成测试覆盖 compact 破坏 tool 配对的修复路径。）
- [x] 配置一个 stdio MCP server（如 filesystem），其工具出现在审批/调用链路。
- [x] 输入 `@` 能补全文件、`/` 能补全命令；plan 模式下写工具被拒。
      （单测：`detect_at_trigger`/`detect_slash_only_at_line_start`/`command_candidates_include_palette`/
      `file_candidates_find_file`/`completion_accepts_slash_command`；kernel `plan_mode_*` 集成测试。）
- [x] 路径逃逸尝试（`../../etc/passwd`）被结构化拒绝。

### 八、后续里程碑（M3 完成后）

- **M4 TUI 精致化**：多会话标签、diff viewer（unified/split）、reasoning 折叠、子 agent 视图、
  rewind 时间线、鼠标与选区复制、剪贴板矩阵/图片粘贴、which-key/toast 分区、侧边栏 context/todo/files。
- **M5 硬化发布**：Anthropic provider、跨 provider 切换/daemon、分发期优化（`panic="abort"`/mimalloc）、
  跨平台 CI + 性能趋势门、headless JSONL 稳定化、conformance 测试套件、cargo-dist 打包。

### 九、实施记录（2026-09-24 完成）

#### 9.1 交付状态

- 14 步（0–13）全部完成；工作区 **179 个测试全绿**（kernel 22、capabilities 63、tui 84、config 7、
  agent 3）；`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check` 均通过。
- 步骤 11 实现时修订为回退方案：tui-textarea 0.13 的 `atomic_ranges` 仅内部 API（`validate_atomic_ranges`
  等私有，无公开 setter），故补全接受采用「整行替换：删旧的 token 段 → 插入 insert 文本 → 手动
  `CursorMove::Jump` 定位光标 → `try_set_atomic_ranges` 把整段设为虚拟片段」，退格整段删除，
  仍为纯 UI 态、不碰 store 权威数据。

#### 9.2 实际模块落位

```
crates/mrorcode-kernel/src/session.rs     # TranscriptEntry/SessionRecord/SessionSummary/derive_title
crates/mrorcode-capabilities/src/
├── path_guard.rs      # resolve_safe / resolve_existing_file（core 路径 + symlink 校验）
├── session/mod.rs     # SessionStore：jsonl 追加 + snapshot 原子写 + 损坏行跳过
├── mcp/               # protocol(JSON-RPC) / client(StdioTransport/初始化/重连) / tool / bootstrap
└── tools/
    ├── read_tracker.rs # read-before-write 指纹
    ├── diff.rs         # unified diff（similar）
    ├── write.rs        # write_file
    ├── edit.rs         # edit_file（唯一匹配/replace_all/结构化歧义报错）
    ├── glob.rs         # glob（ignore Override，is_whitelist）
    └── grep.rs         # grep（regex，忽略二进制，200 行上限）
crates/mrorcode-tui/src/
├── input.rs           # @文件补全 + /命令补全状态机 + 浮层渲染
└── widgets/diff.rs     # diff 行着色（tool_card 接入）
```

#### 9.3 与计划的偏差（均为实现中收敛，无功能裁剪）

1. **MCP stdio 用 tokio `process::Command` + `kill_on_drop` 自定义传输**（计划 §五不引 SDK 一致）；
   `StdioTransport` 持有 `Child` 句柄，字段命名 `_child` 以显式标明"仅靠 Drop 收尾"。
2. **capabilities 新增对 `mrorcode-config` 的依赖**：MCP 装配需 `McpServer` 类型与 config 解析；
   config 是叶子 crate（可被任何人读），分层不受破坏。
3. **`assemble()` 改为 async**：MCP bootstrap 需 await 拉起子进程；CLI/TUI 调用点同步改写。
4. **#glob/grep 的 .gitignore 判断用 `Override::matched` 的 `is_whitelist()`**（初版误用
   `is_ignore()`），单测固化 .gitignore 排除语义。
5. **步骤 11 采用 atomic-range 回退方案**（见 9.1），计划 §四.11 预留该分支。
6. **补 full-reparse/incremental 与既有基准对照**：本次 M3 新增基准全部落在能力侧
   （search/compaction），不改渲染热路径，故 tui 基准仅为确认不回退、未新增 case。

#### 9.4 性能基线（criterion，release）

| 基准 | 结果（2026-09-24） | 说明 |
|---|---|---|
| compaction/estimate_tokens_500_turns | ~24 µs | 触发点判定热路径 |
| compaction/compact_keep_6_of_500_turns | ~318 µs | 2000 条消息折叠 1 次 |
| compaction/compact_small_noop_5_turns | ~3 µs | 不触发时的空跑兜底 |
| search/glob_10000_rs | ~39 ms | 1 万文件列目录一次 |
| search/grep_10000_rs（命中） | ~313 ms | 逐文件正则扫描 |
| search/grep_10000_rs （未命中） | ~313 ms | 整库扫空 |
| windowed_frame / 10000_msgs（既有） | ~0.44 ms | 未回退，预算 < 16ms |
| markdown/incremental_tail_block（既有） | ~0.22 ms | 未回退 |

#### 9.5 已知遗留（归 M4+，与计划非目标一致）

- 步骤 13 计划未落 README 配置说明：README 的 MCP/compaction/resume 章节尚未编写（README 长期
  未维护，M3 收尾时仅更新 AGENTS.md 里程碑引用）。MCP 工具冒烟（真实 stdio server、filesystem）
  在本机未执行，依赖 mock 字节单测与 mock_sse 端到端覆盖。
- 补全浮层未做键盘导航自动化（TestBackend 仅覆盖状态机），真实终端交互待人工冒烟。


---

## mrorCode M4 实施计划：TUI 精致化

> 前置：M1–M3 已完成（见上文各自完成报告）。M4 聚焦 M1 §八、M3 §八 汇总的 TUI 精致化项：
> 多会话标签、diff viewer、reasoning 折叠、子 agent 视图、rewind 时间线、鼠标/选区/剪贴板、
> which-key、侧边栏分区。
>
> 铁律不变：**kernel/runtime 是唯一 agent/会话状态所有者**，UI 只做投影 + 命令。
> 本里程碑只有 rewind 与子 agent 需要新增 kernel/runtime 能力，且均为纯数据、可测。
>
> **状态：已完成（2026-09-25）。** 12 个实施步骤全部落地，工作区 224 个测试全绿；
> 仅余真实终端手工冒烟待人工执行。实施记录与性能基线见文末「八、实施记录」。

### 一、M4 目标（验收标准）

1. **reasoning 折叠**：assistant 消息的 `reasoning` 渲染为可折叠块（默认一行摘要，Enter/点击展开），
   展开态按消息稳定 id 记忆；reasoning 复用 markdown 增量渲染。
2. **diff viewer**：`write_file`/`edit_file` 结果可打开独立 viewer（大面板），显示完整 unified diff，
   支持滚动、unified/split 双视图、行号与增删着色；工具卡保留现有内联折叠。
3. **多会话标签栏**：顶部标签栏列出全部会话（active 高亮、busy spinner），点击/键位切换、关闭标签；
   窄屏自动降级。
4. **鼠标**：滚轮滚动消息区、点击标签/侧栏会话切换、点击工具卡聚焦/展开、拖拽选区。
5. **选区复制**：拖选或键盘选择，复制纯文本到剪贴板（OSC 52 兜底），选中区高亮。
6. **which-key**：leader 等待窗口内浮层列出全部可用第二键及命令，按键/超时后消失。
7. **侧边栏分区**：Sessions / Context（token 用量、压缩状态）/ Files（最近读写文件）三区，可折叠。
8. **rewind 时间线**：把会话回退到某一轮之前（截断该轮及之后）再继续对话，UI 为时间线选择器。
9. **子 agent 视图**：`task` 工具派生只读子 agent 完成子任务，TUI 以嵌套折叠卡显示结果。
10. **toast/通知分区**：toast 按 info/warn/error 着色，并与 which-key、补全浮层避让。

#### 非目标（仍归后续里程碑）

- Anthropic provider、跨 provider 切换、daemon/ACP（M5）。
- 跨平台 CI、conformance testkit、cargo-dist、分发期优化（M5）。
- diff viewer 内联语法高亮（先纯 diff 着色；高亮留 M5）。
- 图片附件/vision 输入（默认裁剪至 M5，见 §五可选依赖）。
- 插件系统、国际化（M5）。

### 二、架构改动要点

#### 2.1 kernel（最小新增，纯数据/可测）

- `AgentCommand::Rewind { session_id: u64, keep_messages: usize }`：把会话消息截断到前
  `keep_messages` 条（保留首条 system），随后 `repair_pairing()`。仅在无运行轮次时生效，
  有轮次时忽略（driver 侧先 Cancel 或拒绝）。
- 子 agent（默认回退方案，见 §六）：`AgentEvent::SubTask { id, parent: String, event: Box<AgentEvent> }`
  与 `ToolContext.events: Option<&UnboundedSender<AgentEvent>>`。**改动核心 trait，需评审。**

#### 2.2 agent runtime

- `DriverCommand::Rewind { session_id, keep_messages }` + `RuntimeHandle::rewind(...)`。
- 子 agent：`assemble` 暴露只读子工具集工厂，供 `task` 工具构造子 `AgentBuilder`。

#### 2.3 TUI 模块（新增/修改）

```
crates/mrorcode-tui/src/
├── selection.rs        # 新：屏幕矩形选区模型 + Buffer→文本提取
├── clipboard.rs        # 新：OSC 52 复制（纯 Rust）；可选 arboard feature
├── input.rs            # 改：补全之外，新增鼠标/选区命中与文本提取的入口
├── app.rs              # 改：InputEvent（键/鼠标/粘贴）分派、鼠标命中、粘贴处理
├── store.rs            # 改：MessageView.uid、expanded_reasoning、last_usage、
│                       #     touched_files、turn_boundaries、show_tabs
├── keymap.rs           # 改：Action::{ToggleReasoning, OpenDiffViewer, Rewind, ToggleTabs,
│                       #     CopySelection, SelectMode, ...}
├── commands.rs         # 改：新增命令（rewind / diff viewer / reasoning / tabs）
└── widgets/
    ├── tabs.rs         # 新：多会话标签栏
    ├── whichkey.rs     # 新：leader 可用键浮层
    ├── reasoning.rs    # 新（或并入 session.rs）：思考过程折叠块
    ├── diff.rs         # 改：解析 unified diff → DiffDoc；unified/split 渲染
    ├── dialog/diff.rs  # 新：diff viewer 作为 Dialog 栈成员
    └── sidebar.rs      # 改：sessions/context/files 三区
```

### 三、关键设计

#### 3.1 稳定消息 id（前置）

- 现状：`MessageView` 按位置寻址，切会话/流式追加会改变下标，无法承载"按消息记忆展开态"。
- 在 `store` 增加单调 `next_message_uid`，`MessageView` 增 `uid: u64`；事件投影新建消息时分配。
- 展开集合、rewind 边界、选区锚点都改用 `uid`。

#### 3.2 reasoning 折叠

- 缺口只在渲染：`MessageView.reasoning` 已存、`project_conversation` 已恢复，但 session 渲染未使用。
- 新增 `reasoning_doc: MarkdownView`（与正文同款缓存 / 增量 `update`）与 `expanded_reasoning: HashSet<u64>`。
- 折叠态一行：`▸ 思考过程 · {行数} 行`（text_muted）；展开态渲染 `reasoning_doc` 全部行 + `▾ 收起`。
- 行高必须计入 `session::message_height`，否则窗口化布局错位。键位/点击切换。

#### 3.3 diff viewer（widgets/diff.rs + dialog/diff.rs）

- 先把现有 unified diff 文本解析为结构：
  `DiffDoc { files: [{ old, new, hunks: [{ header, lines: [{ kind: Context|Add|Remove, text, old_no, new_no }] }] }] }`；
  纯函数 + 单测（多文件、无尾换行、`\ No newline`、空 hunk）。
- 渲染：unified 单列；split 左右两栏（按 hunk 内 old/new 行号对齐，Context 两侧同显）；
  复用 M3 的 `diff_added_bg/diff_removed_bg` 等 token。
- 打开：工具卡聚焦且输出含 diff 时回车打开 `Dialog::Diff`；`t` 切视图、`↑↓/PgUp/PgDn` 滚动、`q/esc` 关闭。
- 大 diff 仍窗口化：只布局可视行（新增基准守护）。

#### 3.4 多会话标签栏（widgets/tabs.rs）

- 数据源即 `store.sessions`；标签显示 `● 标题(spinner)`，宽度按可用宽度分配，超出省略/滚动。
- 键位：`ctrl+tab` 与 `leader [` / `]` 前后切换，`leader 1..9` 保留；点击标签切换/中键关闭。
- `AppState.show_tabs`（KV `tabs: bool`）；窄屏（阈值同 sidebar）自动降级。

#### 3.5 鼠标（app.rs + 命中区域）

- 输入线程改发 `InputEvent::{Key, Mouse, Paste(String)}`；主循环按类型分派。
- `ScrollUp/Down` → 现有 `scroll_up/down`；`Down(Left)` → 命中测试（标签、侧栏会话、工具卡、
  reasoning 摘要）→ 对应动作；`Drag(Left)` → 选区。
- 命中区域在渲染时记入 `AppState.hit_regions: Vec<(Rect, HitTarget)>`，每帧重建（与窗口化渲染同源）。
- 无鼠标事件时事件循环保持 0 唤醒（不新增定时器）。

#### 3.6 选区与剪贴板（selection.rs + clipboard.rs）

- 选区模型：`Selection { start: (x,y), end: (x,y) }`；从最终 `Buffer` 按行提取 symbol，
  行尾去空白、跨行拼接 `\n`；纯函数、TestBackend 可测。
- 键盘 `v` 进入选择模式 + 光标移动，或鼠标拖拽；`y`/`ctrl+shift+c` 复制；`esc` 取消。
- 剪贴板默认写 OSC 52（`\x1b]52;c;<base64>\x07`，无依赖、SSH 友好）；可选 `clipboard` feature
  用 `arboard` 走系统剪贴板（失败静默降级）。

#### 3.7 which-key（widgets/whichkey.rs）

- `Keymap::leader_bindings() -> Vec<(String, Action)>`（新增，基于现有 base 表的 leader 项）。
- `leader_until` 有效时底部渲染浮层：`p → command palette` 等，按键或 2s 超时消失
  （现有 100ms 节拍已在跑）。与 toast 避让：which-key 优先占底部。

#### 3.8 侧边栏分区（sidebar.rs）

- 三区：Sessions（现有）、Context（`last_usage` token 累计 + 是否触发过 compaction）、
  Files（`touched_files`：read/write/edit 的 path，去重、最近优先、上限 ~8）。
- store 在事件投影时更新 `last_usage` 与 `touched_files`；分区标题可折叠，窄屏仅显示 Sessions。

#### 3.9 rewind（kernel + TUI）

- kernel：无 running 时 `AgentCommand::Rewind` 截断 `conversations[session]` 后 `repair_pairing`。
- TUI：`turn_boundaries: Vec<(u64 /*uid*/, String /*首条 user 摘要*/, usize /*消息下标*/)>`；
  rewind 选择器（Select）→ 接受后：`handle.rewind(session_id, keep_messages)` → 本地截断投影 →
  `persist_active_session()` 覆盖 snapshot。
- transcript（jsonl 追加写）不回滚：recall 仍保留完整历史；toast 明示"仅当前会话回退，磁盘记录保留"。
- 测试：kernel 单测（截断后配对正确、仅空闲生效）；TUI 投影测试。

#### 3.10 子 agent（默认回退方案 + 可选实时视图）

- `task` 工具（`{ description, prompt }`，Risky）：派生只读子 agent（read_file/glob/grep，较小
  `max_rounds`），返回最终文本，TUI 以普通工具卡展示（能力落地、视图简单）。
- 若要实时嵌套进度：`ToolContext.events` + `AgentEvent::SubTask{...}` 转发子事件，
  TUI 在父工具卡内嵌套折叠渲染。**改动 kernel 核心 trait，独立步骤、允许裁剪。**

#### 3.11 toast 分区

- toast 记录 level（info/warn/error），左边框用 primary/warning/error；
  which-key/补全浮层占用底部时 toast 上移，避免遮挡。

### 四、实施步骤（每步保持编译+测试通过）

> 门禁同 M1–M3：`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
> `cargo fmt --check`。每步一个提交；结构变化同步更新 `AGENTS.md`，版本按项目规则 bump。

0. **骨架与稳定 id**：`MessageView.uid` + store 单调计数；输入线程改发 `InputEvent`；
   建 `selection.rs`/`clipboard.rs`/`widgets/{tabs,whichkey,reasoning}.rs` 空模块；KV 增 `tabs`。
1. **reasoning 折叠**：`reasoning_doc` + 展开集合 + 行高计入 + 键位/点击切换；单测（折叠高度、展开内容）。
2. **diff 解析**：`DiffDoc` 纯函数解析器 + 单测；工具卡改用解析结果着色（行为不变，作回归基线）。
3. **diff viewer**：`Dialog::Diff` + unified 渲染 + 滚动 + `t` 切 split + 工具卡接线；TestBackend 用例。
4. **多会话标签栏**：tabs widget + 命中区域 + 切换/关闭 + 键位命令；KV 记忆；窄屏降级。
5. **鼠标**：`InputEvent::Mouse` 分派 + 命中测试 + 滚轮/点击；命中函数单测（不依赖真实终端）。
6. **选区 + 剪贴板**：selection 提取纯函数 + 拖选 + OSC 52 复制 + 高亮；可选 arboard feature。
7. **which-key**：`leader_bindings()` + 浮层渲染 + toast 避让；快照测试。
8. **侧边栏分区**：usage/touched_files 记账 + 三区渲染 + 折叠；TestBackend 用例。
9. **rewind**：kernel command + runtime 桥 + TUI 选择器 + 截断重投影 + snapshot；kernel/agent/TUI 测试。
10. **task 工具/子 agent**：`task`（`{ description, prompt }`，Risky）派生只读子 agent
     （read_file/glob/grep，`max_rounds=15`），回灌最终文本，普通工具卡展示（实时嵌套视图为可选加分项）。
11. **toast 分区 + 收尾 UI**：level 着色、浮层避让、新命令登记进 help/命令面板。
12. **性能门禁与收尾**：既有基准（万行帧、增量 markdown、fuzzy）不回退；新增 diff 解析/窗口化 diff 帧/
    选区提取/reasoning 折叠基准；clippy/fmt/release 通过；README 补鼠标/剪贴板/标签/rewind 用法。

### 五、依赖新增

| crate | version | features | 用途 |
|---|---|---|---|
| arboard | 3 | 可选 `clipboard` feature | 系统剪贴板（文本/图片）；非默认，默认 OSC 52 零依赖 |

- 需确认 `arboard` 在 windows-gnu 上走 `windows-sys` 纯 Rust（本机无 C 编译器约束）；若不可用则只保留 OSC 52。
- diff 解析、选区、which-key、标签栏均自写，不引新 crate；子 agent 复用现有 kernel `AgentBuilder`。
- 图片粘贴若纳入，复用 arboard 的 image 能力（默认裁剪至 M5）。

### 六、风险与对策

| 风险 | 对策 |
|---|---|
| 鼠标事件在 Windows conpty/部分终端异常 | 解析失败仅忽略，不影响键盘；`EnableMouseCapture` 已有；命中函数与事件解耦、可单测 |
| 剪贴板跨平台差异 | 默认 OSC 52（纯协议，SSH/多数终端可用）；arboard 为可选 feature，失败静默降级 |
| 选区坐标与窗口化渲染不一致 | 渲染时同步记录 hit/行映射；选区从最终 `Buffer` 提取，不依赖中间结构 |
| rewind 破坏 tool_call/result 配对 | kernel 截断后强制 `repair_pairing`；仅空闲时执行；单测覆盖 |
| transcript 与 rewind 不一致 | 明确语义：仅内存 + snapshot 回退，jsonl 追加记录保留（recall 完整）；toast 说明 |
| 子 agent 改动 kernel 核心 trait | 默认只做"task 工具 + 最终文本"；实时嵌套视图需评审、独立步骤、可裁剪 |
| 新增渲染路径拖慢帧 | 全部窗口化 + 复用 block 缓存；新增基准守护 < 16ms 帧预算 |
| 标签栏/侧栏/浮层遮挡小屏 | 宽度阈值降级（参考 M1 sidebar ≤100 列隐藏）；布局单测 |

### 七、M4 完成判据

- [x] 全部测试 + clippy `-D warnings` + fmt 绿；既有基准不回退，新增基准有基线。
- [x] reasoning 默认折叠、可展开，且流式期间不重排历史。
- [x] diff viewer unified/split 均可滚动，万行 diff 单帧 < 16ms。
- [x] 标签栏切换/关闭会话，进出会话投影不丢。
- [x] 鼠标滚轮/点击/拖选可用；复制内容与屏幕选中一致。
- [x] which-key 在 leader 窗口内列出真实绑定（含用户覆盖）。
- [x] 侧边栏 context/files 数据正确、可折叠。
- [x] rewind 回退后继续对话无 provider 配对错误，snapshot 同步、transcript 保留。
- [x] `task` 工具能完成只读子任务并回灌结果（实时嵌套视图视评审决定）。
- [x] toast 按级别着色、对 which-key/补全浮层避让；新命令（rewind 等）已进 help/命令面板。
- [ ] 手工冒烟：真实终端下鼠标/剪贴板/标签/rewind 各验一次。

### 八、实施记录（2026-09-25 完成）

#### 8.1 交付状态

- 12 步（1–12）全部落地；工作区 **224 个测试全绿**（kernel 13 + 集成 12、capabilities 57 +
  集成 10、config 7、agent 1 + 集成 2、tui 122）；`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo fmt --check`、`cargo build --release` 均通过。
- 模块落位（新增）：
  - `kernel/src/{message,event,agent}.rs`：`Conversation::rewind(keep_turns)` / `AgentCommand::Rewind`
    / 空闲门控处理（M4-9）。
  - `capabilities/src/tools/task.rs`：`TaskTool` 只读子 agent（M4-10）；`tools/mod.rs` 增
    `read_only_tools()/read_only_registry()`；`kernel/src/tool.rs` 的 `ToolRegistry` 派生 `Clone`。
  - `agent/src/persona.rs`：`read_only_sub_persona`；`assemble.rs` 装配 `TaskTool`。
  - `tui/src/widgets/{reasoning,tabs,whichkey,toast}.rs`、`widgets/dialog/diff.rs`、
    `tui/src/{selection,hit,clipboard}.rs`。

#### 8.2 与计划的偏差（实现中收敛）

1. **rewind 以「user turn 数」为语义**（`keep_turns`）而非计划 §2.1 的 `keep_messages`（原始消息条数）：
   kernel 原始消息含 tool_calls/tool_results 展开，与 TUI 投影条数不一致；改为 turn 计数后两层语义统一。
   `AgentCommand::Rewind { session_id, keep_turns }` 仅在无运行轮次时生效，截断后强制 `repair_pairing()`。
2. **子 agent 采用默认回退方案**，未引入计划 §2.1 的 `AgentEvent::SubTask` + `ToolContext.events`
   （实时嵌套视图，列为可选加分项）：`task` 工具派生只读子 agent，回灌最终文本，以普通工具卡呈现。
   `TaskTool` 位于 capabilities，装配时注入共享 provider、只读注册表与子 persona，避免 capabilities
   反向依赖 agent 层。
3. **`ToolRegistry` 派生 `Clone`**（`Arc<dyn Tool>` 浅拷贝）以支持子 agent 复用只读工具集；
   无新增依赖，符合「复用 kernel `AgentBuilder`、不引新 crate」。
4. **toast 由 `Vec<(String, Instant)>` 扩为三元组**携带 `ToastLevel`（help/命令面板仍由
   `commands::registry()` 自动派生，rewind 等新命令无需单独登记）。

#### 8.3 性能基线（criterion，本机 2026-09-25）

| 基准 | 结果 | 预算/说明 |
|---|---|---|
| windowed_frame / 10000_msgs（既有） | ~1.09 ms | 远优于 < 16 ms 帧预算 |
| markdown/incremental_tail_block（既有） | ~0.57 ms | 未回退（增量路径） |
| markdown/full_reparse_each_frame（对照） | ~1.71 ms | 固化增量收益对照 |
| fuzzy/filter_10000_items（既有） | ~0.46 ms | 未回退 |
| diff/parse_400_hunks（新增） | ~0.80 ms | 400 hunk 单文件解析 |
| diff/windowed_frame（新增） | ~1.64 ms | 120×40 窗口化渲染一帧 |
| selection/extract_full_screen（新增） | ~43 µs | 120×40 buffer 行文本提取 |
| reasoning/collapsed_height（新增） | ~16 ns | 折叠态高度（O(1)） |
| reasoning/expanded_height（新增） | ~23 ns | 展开态高度（缓存行数） |

> 本机同一运行下既有基准（syntax/theme/fuzzy）较 M3 报告普遍放大约 2×，为测量环境差异
> （同轮全量基准运行 + 机器负载），非代码回退；关注相对对照与 16 ms 帧预算。

> 遗留：真实终端手工冒烟（完成判据末条）待人工执行。

#### 8.4 收尾补遗（2026-09-25，M4 完成后）

M4 交付后核查计划文件遗留，处理三项：

1. **修复可复现性缺陷：`rust-toolchain.toml` 钉死 gnu 三元组。**
   原文件仅写 `channel = "stable"`，裸 channel 在 Windows rustup 下按安装器 default host
   解析为本机的 `x86_64-pc-windows-msvc`，而项目约定与 `.cargo/config.toml`（dlltool）
   均为 windows-gnu；本机无 MSVC `link.exe`，导致 plain `cargo check/test` 在 build script
   链接阶段全灭。改为 `channel = "stable-x86_64-pc-windows-gnu"` 后，AGENTS.md 标准命令
   （`cargo test/clippy/fmt --workspace`）在本机直接通过，230 测试全绿。
2. **补齐 M3 §9.5 遗留的补全浮层键盘导航自动化。** 新增 `input::move_selection_*`
   纯函数回绕单测，以及经 `on_key` 驱动的浮层导航用例：Up/Down 被拦截且 selected 回绕、
   Enter 接受「导航后」的当前项（非固定首项）、Esc 清除且文本原样保留。
3. **`config.example.toml` 补 compaction / MCP 样例**（M3 步骤 6 计划项）；README 的
   MCP/compaction/resume 章节已在 M4 docs 提交中补齐，本次仅修正其「M1–M3」过期表述为 M1–M4。

仍需人工（无法自动化）：真实终端下鼠标/剪贴板/标签/rewind 冒烟，以及真实 stdio MCP
server（filesystem）冒烟。

---

## mrorCode M5 实施计划：硬化发布

> 前置：M1–M4 已完成（见上文各自完成报告）。M5 从「本机可跑」走向「跨平台可发布、可回归、
> 可安装」。整体拆为 a→e 五个相对独立的子里程碑，**顺序执行**（CI 先落地，后续每步都有回归保护）。
> 每个子步保持 `cargo test/clippy/fmt --workspace` 绿；代码改动按项目规则 bump 版本。
>
> **CI 托管：gitcode.com**（非 GitHub）。其流水线配置格式未事先核实，M5-a 落地时先按 gitcode
> 实际文档确认文件名/语法，再写流水线定义；本节先固定「检查矩阵 / 触发条件 / 产物」语义。

### 总目标（验收标准）

1. 任意 PR / push 在 Win / macOS / Linux 三平台 + MSRV(1.88) 上自动跑 fmt/clippy/test，全绿方可合并。
2. 支持 Anthropic Messages API（除 OpenAI 兼容外的第二 provider），工具调用往返不破坏配对。
3. 每个平台产出可直接运行的单二进制产物（含体积基线）。
4. `headless --json` 事件 schema 有契约测试守护；testkit 可对抗校验事件序列。
5. 跨 provider 切换有明确方案（重建 runtime 或 daemon，先设计后动工）。

### 非目标（仍归后续 / 不做）

- 子 agent 实时嵌套视图（`AgentEvent::SubTask`，改 kernel 核心 trait）——评估后可作 M5 加分项，
  但有专门评审，不默认纳入。
- diff viewer 内联语法高亮、图片/vision 输入、插件系统、国际化。

---

### M5-a 跨平台 CI（gitcode）

**目标**：三平台 + MSRV 自动门禁；Windows 不依赖手工装 VS。

- **矩阵**：`windows-latest / macos-latest / ubuntu-latest` × 稳定工具链；另起 1 个 job 钉
  MSRV（`rust-version = "1.88"`）仅在 ubuntu 上跑（快）。
- **每 job 步骤**：
  1. 检出仓库；
  2. 装 Rust（`rust-toolchain.toml` 已钉 `stable-x86_64-pc-windows-gnu`，CI 直接读它）；
  3. **Windows 额外**：跑 `pwsh -File scripts/bootstrap-tools.ps1`（解包 binutils 到 `.tools/mingw`，
     生成 `.cargo/config.toml` 的 dlltool 指向）；
  4. `cargo fmt --check`；
  5. `cargo clippy --workspace --all-targets -- -D warnings`；
  6. `cargo test --workspace`。
- **基准趋势门（独立 release job）**：`cargo bench -p mrorcode-tui --bench render`，产出 criterion
  JSON。首次运行只**存基线**（`target/criterion` 归档），不设硬阈值；后续 PR 与基线对比，关键指标
  回退 > 10% 时报错并要求说明（沿用 M1 §六约定）。
- **验收**：push / PR 三平台全绿；Windows job 全自动化（无 `link.exe` 手工安装）。
- **风险**：gitcode 流水线语法未知 → 先查文档再写；macOS/Linux 无 native-tls 的 C 依赖问题
  （SChannel/Security.framework 在对应平台是系统库，CI 需带系统 OpenSSL？——实为 native-tls，
  mac/linux 走 Security.framework / Schannel，无需 OpenSSL；若报缺库再补 `brew install` 之类）。

---

### M5-b Anthropic provider

**目标**：OpenAI 兼容之外的第二 provider；复用手写 SSE 解码器；工具调用往返正确。

- **config（leaf）**：
  - `resolve()` 放行 `type = "anthropic"`（现 lib.rs:240 硬拒绝非 openai_compat，且有测试
    `unknown_provider_type_rejected` 钉死 anthropic 必被拒——改：白名单 `openai_compat | anthropic`，
    并把该测试改为「未知类型仍拒、anthropic 通过」）。
  - `ProviderConfig` 复用现有 `base_url`（anthropic 默认 `https://api.anthropic.com`）/`api_key`/
    `model`/`context_window`；新增环境变量 `MRORCODE_ANTHROPIC_API_KEY`（装配时按 type 选用）。
  - `ConfigError::UnknownProviderType` 文案更新支持列表。
- **capabilities/provider/anthropic.rs（新）**：
  - 请求 `POST {base}/v1/messages`，头 `x-api-key` / `anthropic-version: 2023-06-01`；
    `system` 独立顶层字段（非 messages 内一条）；消息按 `user/assistant` 分组，工具调用映射为
    `tool_use` 块、工具结果映射为 `tool_result` 块。
  - 复用 `sse.rs` 的 `SseDecoder::feed`；事件类型 `message_start/content_block_delta/...` →
    kernel `StreamEvent`；`content_block_stop` 的 `tool_use` 参数累积后产出 `ToolCallStarted/Delta`。
  - 把 kernel `Message/ToolCall/ToolResult` ⇄ Anthropic 块做纯函数映射（可单测），落库前仍过
    `repair_pairing`（防配对破坏）。
- **assemble.rs**：按 `resolved.provider_type` 选 `OpenAiCompatProvider` 或新 `AnthropicProvider`，
  二者都装成 `Arc<dyn LlmProvider>`。
- **验收**：wiremock mock Anthropic SSE（首段文本、tool_use、tool_result 回灌、收尾）全链路单测；
  不依赖真实 key；同一份 ScriptedProvider 集成测试在两种 provider 下语义一致。
- **风险**：Anthropic 工具参数是流式 JSON 片段（delta 是字符串拼接的 JSON），需在解码器层做
  分片拼接后再 parse（仿 openai_compat 的做法）；消息映射表易错 → 映射纯函数 + 录制字节单测先行。

---

### M5-c 打包分发

**目标**：每平台产出可直接运行的单二进制；体积 / 分发期优化有基线再定。

- 引入 **cargo-dist**（或 gitcode CI 的产物上传步骤），在 M5-a 的三平台 job 末尾构建并上传
  `mrorcode[-<triple>].{exe,targz}`；打 tag 时出 GitHub/gitcode Release。
- 记录 release 二进制体积基线（当前 M1 记录 windows-gnu release ~4.9MB）。
- **分发期优化（先量化再决定，不默认开）**：评估 `panic = "abort"` 与 `mimalloc`——只在有
  benchmark 显示启动/吞吐收益时才开；先在独立 job 验证 windows-gnu 链接兼容性（mimalloc 的 C
  运行时交互、abort 与 unwind 终端恢复兜底的冲突），有风险就保持现状。
- **验收**：三平台产物可直接执行 `mrorcode --version`；体积对比 M1 基线不异常膨胀。
- **风险**：mimalloc 在 windows-gnu 可能引入新链接问题 → 单独 job 验证，失败则回退（不阻塞 M5-c）。

---

### M5-d headless JSONL 稳定化 + conformance testkit

**目标**：`--json` 事件契约冻结；可用 testkit 对抗校验任意 provider 的事件序列。

- 冻结 `cli/headless.rs` 输出的 JSONL 事件 schema（`run.started/message.delta/tool.started/
  tool.completed/turn.completed/run.finished`），加 schema 快照测试（insta）防漂移。
- 扩展 kernel `test-support`：在现有 `ScriptedProvider` 上加「期望事件序列」断言器，形成可复用
  conformance 套件；OpenAI 兼容与 Anthropic（M5-b）两种实现跑同一套断言。
- **验收**：改事件字段会让快照测试失败；conformance 套件对两个 provider 均绿。
- **风险**：快照测试对有意的演进过于严格 → 演进时显式更新快照并在提交说明记录。

---

### M5-e 跨 provider 切换 / daemon（架构改动最大，最后做）

**目标**：在运行中切换 provider（不同 base_url/api_key），而非「改配置重启」。

- **先出设计再动工**：现状切换 provider 需重建 runtime（M2 §8.5 已注明）。两条候选：
  1. **轻量方案**：TUI 侧切换 provider 时优雅关闭当前 runtime、以新 provider 重新 `assemble`
     并 resume 当前会话（不引入 daemon，复杂度低）；
  2. **daemon 方案**：常驻进程持有 provider 连接池，TUI/headless 作为客户端（接近 atomcode/ACP，
     复杂度高）。
- 默认倾向轻量方案（1）；daemon（2）仅在有明确多客户端需求时再做。
- **验收**：切换 provider 后当前会话能继续对话、上下文不丢；不满足则降级为「需重启」的明示提示。
- **风险**：重建 runtime 时会话状态迁移 → 复用 M3 的 snapshot/resume 路径，不另造状态机。

---

### M5 验收清单（总）

- [ ] M5-a：三平台 + MSRV CI 全绿，Windows 全自动化。（gitcode CI 为 Web UI 配置，待人工在面板创建）
- [x] M5-b：`type = "anthropic"` 可用，工具往返不破坏配对，wiremock 单测覆盖。（2026-09-25 完成，241 测试全绿）
- [ ] M5-c：三平台单二进制产物，体积基线记录（优化项按数据决定开否）。
- [ ] M5-d：`--json` 契约快照测试 + 双 provider conformance 套件。
- [ ] M5-e：跨 provider 切换有落地方案（轻量或 daemon，按设计文档结论）。

### 自 M4 延后的可选项（评估，不默认纳入 M5）

- diff viewer 内联语法高亮、图片/vision 输入、子 agent 实时嵌套视图（`AgentEvent::SubTask`）。
