# mrorCode 内核架构深度文档

> 目标读者：想深入理解 mrorCode 内部机制的开发者。
> 前置阅读：[project-plan-atomcode.md](project-plan-atomcode.md)（总体设计）、[AGENTS.md](../AGENTS.md)（开发者快速上手）。
> 本文聚焦 `mrorcode-kernel` crate 的设计细节与关键决策。

---

## 一、内核定位

`mrorcode-kernel` 是整个项目的 L0 中立内核，**只定义机制，不包含任何产品策略**：

- ❌ 不包含具体 provider（OpenAI/Anthropic）
- ❌ 不包含具体工具（read/bash/edit 等）
- ❌ 不包含审批规则、persona、配置解析
- ✅ 定义消息模型与会话结构
- ✅ 定义 `LlmProvider` / `Tool` / `ToolMiddleware` 抽象 trait
- ✅ 实现 agent 执行循环（多轮对话 + 工具调用 + 取消 + 压缩）
- ✅ 定义事件/命令协议（`AgentEvent` / `AgentCommand`）
- ✅ 实现会话纯数据操作（compaction、rewind、配对修复）

依赖面极小：仅 `tokio` / `futures` / `async-trait` / `serde` / `thiserror` / `tracing`，零 fs、零网络。

---

## 二、核心类型关系

```
Conversation (Vec<Message>)
  └─ Message
       ├─ role: Role (System/User/Assistant/Tool)
       ├─ text: String
       ├─ tool_calls: Vec<ToolCall>   (assistant 发起)
       ├─ tool_call_id: Option<String> (tool 结果对应)
       ├─ reasoning: Option<String>
       └─ meta: MessageMeta (token 用量)

AgentBuilder → AgentHandle
  ├─ commands: UnboundedSender<AgentCommand>
  └─ events: UnboundedReceiver<AgentEvent>

ToolRegistry (BTreeMap<String, Arc<dyn Tool>>)
  └─ dyn Tool: name / description / parameters_schema / risk() / execute()

ToolMiddleware (before / after)
  └─ 可多个，按注册序执行

LlmProvider
  └─ chat_stream() -> BoxStream<StreamEvent>
```

关键设计原则：**trait 对象用 `Arc<dyn Trait>`**，支持廉价 Clone，注册后可被多处共享（如子 agent 复用只读工具集）。

---

## 三、Agent 循环（agent.rs）

### 3.1 整体结构

Agent 由一个后台 task 驱动，核心是 `run_agent()` 函数的 `tokio::select!` 循环：

```
┌─────────────────────────────────────────────────┐
│  run_agent (单 task)                            │
│                                                 │
│  select! {                                      │
│    cmd = cmd_rx.recv() => 处理命令              │
│    returned = running.done => 轮次结束,还会话   │
│  }                                              │
│                                                 │
│  ┌─ pending: VecDeque<(session_id, text)>       │
│  └─ running: Option<RunningTurn>                │
└─────────────────────────────────────────────────┘
```

### 3.2 会话所有权模型

这是内核最核心的设计之一：**同一时刻只有一个 turn 持有会话**。

```
空闲时: conversations HashMap 中保存所有会话
           ↓
开始 turn: take_conversation(id) 把会话从 HashMap 中移出
           ↓
turn task: 独自持有 &mut Conversation，流式请求 → 累积 → 执行工具 → 回灌
           ↓
结束 turn: 会话被归还到 conversations HashMap
```

这样做的好处：
- 无锁，无并发竞争（同一时刻只有一个 owner）
- turn task 可以 `&mut` 直接修改会话，不需要内部可变性
- 取消时直接 drop turn task，会话被丢弃，下一轮重新取就是干净的

### 3.3 轮次（Turn）内部流程

一轮 turn 内部是一个 `for _round in 0..max_rounds` 循环：

```
┌──────────────────────────────────────────────────────┐
│  TurnCtx::run()                                       │
│                                                       │
│  1. TurnStarted 事件                                  │
│  2. 用户消息入栈                                      │
│  3. 若超阈值，compact 上下文                          │
│                                                       │
│  for round in 0..max_rounds:                         │
│    ┌──────────────────────────────────────────────┐   │
│    │  流式请求 provider                           │   │
│    │    ├─ TextDelta    → 累积 + 发射事件        │   │
│    │    ├─ Reasoning    → 累积 + 发射事件        │   │
│    │    ├─ ToolCall*    → 按 id 累积参数        │   │
│    │    └─ Usage        → 取最大值合并           │   │
│    └──────────────────────────────────────────────┘   │
│    ┌──────────────────────────────────────────────┐   │
│    │  助手消息落库（文本 + tool_calls + reasoning）│   │
│    └──────────────────────────────────────────────┘   │
│    无 tool_calls? → TurnComplete + return             │
│    ┌──────────────────────────────────────────────┐   │
│    │  顺序执行每个 tool_call:                     │   │
│    │    1. mode gate (ReadOnly/Workspace/Full)    │   │
│    │    2. middleware before() 链                 │   │
│    │    3. ToolStarted 事件                       │   │
│    │    4. tool.execute()                         │   │
│    │    5. middleware after() 链                  │   │
│    │    6. 结果截断 (head+tail, 64KiB)            │   │
│    │    7. ToolResult 事件 + 回灌会话            │   │
│    └──────────────────────────────────────────────┘   │
│                                                       │
│  超过 max_rounds → TurnComplete(MaxRounds)            │
└──────────────────────────────────────────────────────┘
```

### 3.4 取消机制

取消通过 `CancellationToken` 实现，在三个检查点生效：

1. **流式请求中**：`select!` 中 `cancel.cancelled()` 与 `stream.next()` 竞速
2. **工具执行中**：同上，`cancel.cancelled()` 与 `tool.execute()` 竞速
3. **round 循环入口**：每轮开始前检查一次

取消时的关键行为：
- 已执行的工具结果正常保留
- **未执行的工具补合成 cancelled 结果**，防止留下悬空 tool_call（否则下一次请求 provider 会 400）
- 发射 `Cancelled` 终态事件

这个"补合成结果"的设计是从 OpenAI/Anthropic 协议约束反向推导的——每个 `tool_call` 必须紧跟对应的 `tool_result`，否则 provider 拒绝请求。

### 3.5 工具结果截断

默认 64 KiB，采用 **head + tail 中央截断**：

```
[头部 budget 字节]
...[N bytes truncated]...
[尾部 budget 字节]
```

为什么不用中间省略号或纯尾部截断？
- 工具输出通常开头有结构信息（如 ls 的前几行、grep 的上下文）
- 尾部可能有结论/错误信息
- 中间最可能是冗余内容

截断函数是 UTF-8 安全的（`floor_char_boundary` 自实现，因 MSRV 1.88 不支持 `str::floor_char_boundary`）。

---

## 四、事件 / 命令协议

### 4.1 设计原则

`AgentCommand` 和 `AgentEvent` 都是 `Serialize + Deserialize`，未来可跨进程。

- **命令（driver → agent）**：单向指令，不携带返回值
- **事件（agent → driver）**：单向通知，不期待应答
- **请求-应答**：走 `RequestBroker`，`AgentEvent::Request` 发起 + `AgentCommand::Respond` 回应

### 4.2 请求-应答机制（request.rs）

审批、提问等需要 driver 回应的场景，不直接用 oneshot channel（不可序列化），而是走 id 路由：

```
driver: AgentCommand::Respond(id, value)
  ↓
agent:  RequestBroker::resolve(id, value)
  ↓
middleware: Requester::request(kind, payload).await → Result<Value>
```

`RequestBroker` 内部用 `HashMap<u64, oneshot::Sender<Value>>` 维护 pending 请求。超时或取消时：
- oneshot sender 被 drop
- 接收端得到 `RecvError`
- middleware 按 fail-closed 处理（拒绝执行）

---

## 五、权限系统

### 5.1 三层防御

```
┌─────────────────────────────────────────┐
│  第一层: PermissionMode gate            │  ← 模式硬限制，不弹窗
│  ReadOnly → 只放行 Safe                 │
│  Workspace → WorkspaceWrite 也放行      │
│  Full → 全部放行(跳过审批)              │
└─────────────────────────────────────────┘
                    ↓ 未被拦住的
┌─────────────────────────────────────────┐
│  第二层: ToolMiddleware::before() 链    │  ← 审批/参数修复/审计
│  按注册序执行                           │
│  任意一个 Deny → 终止链, 拒绝执行       │
└─────────────────────────────────────────┘
                    ↓ 通过的
┌─────────────────────────────────────────┐
│  第三层: tool.execute() 内部校验        │  ← 工具自身的安全检查
│  路径安全、参数合法性、超时             │
└─────────────────────────────────────────┘
```

### 5.2 风险等级（RiskLevel）

`Tool::risk(&arguments) -> RiskLevel` 只是**建议元数据**，强制力在 middleware 和 mode gate。

| 等级 | 含义 | 典型工具 |
|---|---|---|
| `Safe` | 纯只读，无副作用 | read_file、glob、grep |
| `WorkspaceWrite` | 限定在工作目录内的写操作 | write_file、edit_file |
| `Risky` | 副作用超工作区，需审批 | bash、task 子 agent、MCP 工具 |

安全默认：如果工具不显式覆盖 `risk()`，默认返回 `Risky`。

### 5.3 中间件链

`ToolMiddleware` trait 有两个钩子：

```rust
async fn before(&self, call: &mut ToolCall, risk: RiskLevel, tool: &Arc<dyn Tool>,
                request: &Requester, cancel: &CancellationToken) -> BeforeOutcome

async fn after(&self, result: &mut ToolResult, tool: Option<&Arc<dyn Tool>>) -> AfterOutcome
```

- `before` 可以**修改参数**（`&mut ToolCall`）—— 参数修复类中间件利用这一点
- 注册顺序 = 执行顺序；参数修复必须排在审批之前（约定）
- `BeforeOutcome::Proceed` / `Allow` / `Deny` 三种结果
  - `Proceed`：继续下一个中间件
  - `Allow`：语义上"显式放行"（如 always grant），行为同 Proceed
  - `Deny`：终止链条，工具不执行，原因作为工具结果回灌模型

---

## 六、会话模型（message.rs）

### 6.1 Conversation

`Conversation` 是一条按时间排序的消息序列，核心操作都是纯函数：

| 操作 | 说明 |
|---|---|
| `push / extend` | 追加消息 |
| `system_prompt()` | 取首条 system 消息 |
| `estimate_tokens()` | 粗略 token 估算（约 4 字符/token），只用于压缩触发 |
| `compact(keep_turns)` | 折叠较早轮次为 system 注记 |
| `rewind(keep_turns)` | 回退到第 N 个 user turn 之前 |
| `repair_pairing()` | 修复悬空 tool_call/tool_result 配对 |

### 6.2 配对修复（repair_pairing）

这是防御性设计——正常循环中调用与结果总是成对落库，但取消、异常退出、resume 旧版本数据时可能出现不一致。

修复做两件事：
1. **移除孤儿 tool 消息**：找不到对应 assistant tool_call 的 tool 消息直接删掉
2. **补合成缺失的 tool 结果**：assistant 发起了 tool_call 但没有对应 tool_result 的，补一条 `status: cancelled` 的结果

为什么必须有这个？因为 OpenAI/Anthropic API 严格要求每个 tool_call 都有对应结果，缺失会导致 400 错误。

### 6.3 压缩策略（compaction）

触发条件：`estimate_tokens() > threshold_tokens`（默认 100k），在每轮开始前检查。

压缩逻辑：
- 保留首条 system 消息
- 保留最近 `keep_turns`（默认 6）个 user turn 及其后所有消息
- 中间区间折叠为一条 system 注记：`[compacted N earlier message(s) incl. M tool call(s); first request: ...]`
- **强制 `repair_pairing()`**：保证折叠后没有孤儿 tool 结果

这是最简单的压缩策略（"折叠为注记"），不是摘要式压缩（不用模型总结）。更复杂的策略（cache-friendly stub、sacred floor、net-loss guard 等）留给后续里程碑。

### 6.4 Rewind

与 compaction 不同，rewind 是**用户主动操作**（`leader w`），截断到指定 turn 之前继续对话。

- 只保留首条 system + 最早 `keep_turns` 个 user turn
- transcript（jsonl）不回滚，recall 仍保留完整历史
- snapshot 同步更新
- 仅在 agent 空闲时生效（有运行轮次则忽略）

---

## 七、多会话模型

### 7.1 会话管理

内核用 `HashMap<u64, Conversation>` 存多会话，但**不做并发执行**：

- 同时只有一个 turn 在跑（单 owner 模型的延伸）
- busy 时收到其他 session 的 submit → 进 `pending` 队列（FIFO）
- 上一轮结束 → 从 pending 取下一个继续

为什么不并发？
- 工具执行（特别是 bash/file 写）并发会有副作用冲突
- provider 并发请求配额可能不够
- 单用户场景下并发收益不大，复杂度显著上升

### 7.2 会话 id 分配

session_id 是 `u64`，由 driver（TUI/CLI）分配，内核不分配。

- TUI 用递增计数器分配
- headless 用固定 `HEADLESS_SESSION = 1`
- resume 时用磁盘记录里的 id

---

## 八、Provider 抽象

### 8.1 LlmProvider trait

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn model_name(&self) -> &str;
    fn context_window(&self) -> u32 { 128_000 }

    fn chat_stream<'a>(
        &'a self,
        messages: &'a [Message],
        tools: &'a [ToolDef],
        options: &'a ChatOptions,
        cancel: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ProviderResult<BoxStream<'a, StreamEvent>>> + Send + 'a>>;
}
```

关键设计点：
- 返回 `BoxStream<'a, StreamEvent>`，所有 provider 统一为同一种流事件
- `cancel` 是引用，provider 实现需在自己的 select 中参与取消
- `ChatOptions` 是纯转发，内核不做策略（模型覆盖、temperature、max_tokens、tool_choice）

### 8.2 StreamEvent

```
TextDelta(String)          → 正文增量
Reasoning(String)          → 推理/思考增量
ToolCallStarted { id, name }  → 新工具调用开始
ToolCallDelta { id, arguments } → 工具参数 JSON 片段
Usage { input, output }    → token 用量（同轮取最大值合并）
Error(ProviderError)       → 流错误
```

工具调用参数是**分片到达**的（SSE chunk 中可能只来一部分 JSON），所以分 Started + Delta 两种事件，由 agent 层累积成完整 `ToolCall`。

### 8.3 ProviderError

- `retryable: bool` 决定上层是否重试
- `is_context_overflow()` 用启发式关键词检测上下文溢出（未来用于自动压缩重试）
- 包含 `http_status` 和 `code`（可选）

---

## 九、测试支持（test-support feature）

`mrorcode-kernel` 有一个 `test-support` feature，暴露给上层 crate 做集成测试：

- **`ScriptedProvider`**：按预定义脚本吐 StreamEvent，模拟各种场景（纯文本、工具调用、错误、延迟）
- **`EchoTool`**：把输入原样返回的测试工具
- **`stream_signature` / `event_signature`**：把事件序列压缩成签名字符串，用于 conformance 断言
- **`assert_stream_sequence` / `assert_event_sequence`**：conformance 测试断言器

设计思路：**内核的正确性不依赖真实 API**，所有核心路径（agent 循环、取消、配对修复、压缩、rewind）都能用脚本化 provider 覆盖。

---

## 十、扩展点

内核预留了以下扩展缝，上层可以按需挂载：

| 扩展点 | 位置 | 用途 |
|---|---|---|
| `LlmProvider` trait | provider.rs | 新增 provider 实现 |
| `Tool` trait | tool.rs | 新增工具 |
| `ToolMiddleware` trait | middleware.rs | 审批、参数修复、审计、日志等 |
| `ChatOptions` | provider.rs | 扩展请求参数（内核只转发） |
| `CompactionPolicy` | agent.rs | 调整压缩策略 |
| `PermissionMode` | event.rs | 扩展权限模式 |

---

## 十一、与 atomcode kernel 的差异

mrorCode kernel 借鉴了 atomcode 的分层思路，但在以下地方做了简化（符合「先跑通再优化」的原则）：

1. **工具执行**：atomcode 是 CLASSIFY → EXECUTE（并发门 + 信号量）→ APPLY（保序）三阶段；mrorCode 是顺序执行（M1 设计，后续可加并发）
2. **压缩**：atomcode 是 cache-friendly stub + sacred floor + net-loss guard；mrorCode 是简单折叠注记
3. **会话持久化**：atomcode 有 lease + 跨进程互斥 + repair pipeline；mrorCode 是单进程 jsonl + snapshot
4. **请求-应答**：atomcode 是 `RequestCtx` oneshot；mrorCode 是 `RequestBroker` + id 路由（为了可序列化）
5. **SSE**：思路一致（手写解码器 + 录制字节测试），实现细节不同
