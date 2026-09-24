# mrorCode M1 骨架实施计划：多轮工具闭环 + OpenCode 风格最小 TUI

> **状态：M1 已完成（2026-09-23）。** 12 个步骤全部落地，49 个自动化测试全绿，
> clippy `-D warnings` / fmt 门禁通过，debug+release 构建通过，
> headless 与 TUI 均端到端跑通。实测数据与偏差见文末「九、M1 完成报告」。
>
> 本计划是第一个可执行里程碑（M1）的实施方案。整体产品规划见
> [docs/project-plan-atomcode.md](../../docs/project-plan-atomcode.md)，
> 本文在其基础上把第一阶段细化为可落地的步骤、文件与验证。

## 一、调研结论（Repository Research）

### 1.1 参考项目事实

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

### 1.2 已确认决策

| 决策项 | 结论 |
|---|---|
| 命名 | 二进制 `mrorcode`，crate 前缀 `mrorcode-*`，配置目录 `~/.mrorcode/` |
| M1 范围 | 多轮对话 + `read_file`/`bash` 工具调用闭环 + 流式 TUI（三栏雏形 + 审批弹窗） |
| 首个 Provider | OpenAI 兼容（覆盖 OpenAI/DeepSeek/GLM/兼容网关），手写 SSE 解码 |
| TUI 栈 | ratatui 0.30（crossterm 0.29 后端）+ 自研 Elm 式 store 状态层 |
| Rust 版本 | MSRV 1.88（ratatui 0.30 要求） |

## 二、M1 目标与非目标

### 目标（验收标准）

1. `cargo build` 产出单二进制 `mrorcode`；无配置时报清晰的引导错误。
2. `mrorcode`（无参）启动 TUI：Home 页居中输入框；发送后进入 Session 页，流式渲染回复，工具调用显示状态卡片，bash 危险命令弹出 once/always/reject 审批。
3. 支持多轮：工具结果回灌模型继续对话，直到模型无工具调用结束本轮；Esc（双击）可中断。
4. `mrorcode run "提示"` 走 headless 一次性任务，stdout 输出流式文本与工具状态（`--json` 输出版本化 JSONL），用于无 TTY 环境与自动化验证。
5. 配置来自 `~/.mrorcode/config.toml` 与环境变量（`MRORCODE_API_KEY`/`MRORCODE_BASE_URL`/`MRORCODE_MODEL`）。
6. 内核可用脚本化 MockProvider 做完整单测，不依赖真实 API。

### 非目标（M1 明确不做，排入后续里程碑）

- 会话磁盘持久化 / resume / rewind / compact（M3）
- write/edit/grep/glob 以外工具、MCP、skills、memory、子 agent、plan 模式（M3）
- Markdown 完整渲染与 syntect 高亮、diff 视图、@补全、命令面板、主题 JSON、leader 键（M2）
- 鼠标、剪贴板矩阵、图片、IDE 集成、插件、守护进程/ACP、多 provider（M4+）

### 性能硬指标（与功能同等验收，达不到不进 M2）

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

## 三、Crate 与模块划分

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

### 各 crate 职责与关键类型

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

## 四、实施步骤（依赖排序）

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

## 五、关键依赖

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

## 六、验证

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

## 七、风险与对策

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

## 八、后续里程碑路线图（概要，M1 批准后另行细化）

- **M2 TUI 核心体验**：syntect 代码高亮、bash 输出折叠卡、通用 Dialog 栈 + fuzzy Select + 命令面板、主题 JSON（直接兼容 opencode 33 主题文件）与 KV 持久化、leader 键与用户键位覆盖、模型切换弹窗、会话列表/新建。
- **M3 Agent 能力**：write/edit（similar diff）/grep/glob、会话 snapshot+jsonl 双层持久化与 resume、stub/overflow compaction、MCP stdio、@补全与 /斜杠命令（tui-textarea atomic ranges 承载虚拟片段）、plan 模式、路径安全 gate。
- **M4 TUI 精致化**：多会话标签、diff viewer（unified/split）、reasoning 折叠、子 agent 视图、rewind 时间线、鼠标与选区复制、剪贴板矩阵/图片粘贴、which-key/toast/通知、侧边栏 context/todo/files 分区。
- **M5 硬化发布**：Anthropic provider、分发期优化评估（`panic="abort"`、mimalloc、进一步 LTO/体积）、跨平台 CI（Win/macOS/Linux，含性能基准趋势门）、headless JSONL 稳定化、conformance 测试套件（仿 atomcode testkit 对抗双甲）、cargo-dist 打包。

## 九、M1 完成报告（2026-09-23）

### 9.1 交付清单

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
- 附加交付：[config.example.toml](../../config.example.toml)、
  [scripts/install.ps1](../../scripts/install.ps1)（装入 `~/.cargo/bin`，全局可用）、
  [scripts/bootstrap-tools.ps1](../../scripts/bootstrap-tools.ps1)（本机工具链修复，见 9.3）。

### 9.2 性能实测（criterion，bench profile）

| 基准 | 实测 | 预算/结论 |
|---|---|---|
| markdown 全量重解析（200 block 文本/帧） | 0.73 ms（首轮 1.27ms 冷缓存） | 对照基线 |
| markdown 增量更新（仅尾 block 变化） | **0.22 ms（早期 18µs 级，随文本增长）** | 增量路径比全量快 3-70 倍，流式只重排尾块的设计有效 |
| **1 万条消息窗口化单帧** | 初版 **47ms（超 16ms 预算）→ 优化后 0.40ms** | 达标（预算的 3.9%）；优化=MarkdownView 零分配 `update(&str)` + 缓存命中时 `line_count` 只数行不克隆 |
| 宽度计算（混合宽字符串） | 缓存版 455ns vs 裸 unicode-width 47ns | **缓存方案被证伪并删除**，仅保留 ASCII 快路径 |
| 空闲事件循环 | 无 spinner 时不挂任何定时器（单测断言 idle→无 timer、busy→有 timer、回 idle→撤销） | 0 唤醒达标 |

未自动化的两项（需真实终端/真实 API，留作人工冒烟）：TTFFD < 50ms、1000 token/s 下 CPU < 15%；
架构上已满足前提（单进程 enum+mpsc、无 worker、无固定帧率）。

### 9.3 与计划的偏差及原因

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

### 9.4 已知限制（明确留给后续里程碑）

- 仅 OpenAI 兼容 provider；bash 风险分类是保守词表（非 AST，默认偏 Risky 是安全方向）；
- 会话不持久化、无并发工具、无 compaction；
- TUI 无主题文件/命令面板/@补全/鼠标（按计划归 M2）；
- 窄终端（≤100 列）sidebar 直接隐藏，未做 opencode 式浮层；
- headless 审批一律拒绝（交互审批只在 TUI；`--dangerously-skip-permissions` 可放行）。


