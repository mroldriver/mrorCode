# 架构决策记录（ADR）

> 本文档记录 mrorCode 项目中的关键架构决策。每条决策包含：背景、决策内容、后果（正面 + 负面）、替代方案。
> 新增决策追加到文末，旧决策不删除，保持历史可追溯。

---

## ADR-001：六层 crate 单向依赖架构

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

终端 AI agent 项目容易出现"一层大杂烩"的问题——UI 逻辑、agent 核心、工具实现搅在一起，难以测试、难以替换组件。atomcode 提供了一个经过验证的分层模型。

### 决策

采用六层 crate，严格单向依赖：

```
cli ─┬─> tui ─> agent ─> capabilities ─> kernel
     └─> agent ──┘            └─> config（leaf，任何人可读）
```

- **kernel**（L0）：纯机制，零 IO，零产品策略
- **capabilities**（L1）：具体 provider / 工具 / MCP / 持久化实现
- **agent**（L2）：装配层，把内核和能力组装成可用的运行时
- **config**（leaf）：配置解析，可被任何层读取
- **tui / cli**（L3）：驱动层，只持有 runtime handle，发命令、收事件

编译期通过依赖方向保证单向性，不允许反向依赖。

### 后果

✅ **正面**：
- 内核可独立测试（脚本化 provider，无真实 API 依赖）
- 替换 UI 技术栈不影响核心
- 新增 provider / 工具不影响其他层
- 每层职责清晰，code review 有明确边界

⚠️ **负面**：
- 跨层修改需要改多个 crate，初期略繁琐
- 类型在层间传递有时需要重复定义或 re-export
- 新人需要理解分层才能找到正确的修改位置

### 替代方案

- **单层大 crate**：简单但不可维护，否决
- **内核 + UI 两层**：中间层太薄，能力层和装配层会混，否决

---

## ADR-002：kernel 是 agent 状态的唯一所有者

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

很多 TUI 应用（包括早期 opencode 的一些分支）会在 UI 层维护一份"自己理解的"会话状态，然后和后端同步。这很容易产生状态不一致、竞态、重复逻辑。

### 决策

kernel runtime 是 agent / 会话状态的**唯一权威所有者**。TUI 和 CLI 都只是 driver：

- 只发 `AgentCommand`（提交、取消、回应审批、切换模型...）
- 只收 `AgentEvent`（文本增量、工具调用、结果、错误...）
- store 中只存**投影态**（滚动位置、展开折叠、输入框内容），不存会话权威数据

### 后果

✅ **正面**：
- 不会有两份状态不一致的问题
- 新增 driver（如未来的 IDE 插件、daemon 客户端）只需要实现事件投影
- 内核逻辑集中，测试只需要测内核

⚠️ **负面**：
- 一些 UI 交互（如 rewind）需要先想清楚"内核语义是什么"，不能直接在 UI 层动手
- UI 状态投影可能有延迟（事件合批），但对用户体验影响可接受

### 替代方案

- **UI 也有状态，和后端双向同步**：复杂度高，容易出 bug，否决
- **UI 持有引用**：借用检查 + 异步生命周期地狱，否决

---

## ADR-003：手写 SSE 解码器，不依赖 eventsource crate

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

LLM streaming 普遍用 SSE（Server-Sent Events）。Rust 生态有 `eventsource` / `eventsource-client` 等 crate，但有几个问题：

1. 依赖 `reqwest` 以外的 HTTP 客户端或自己做连接管理
2. 重连策略不可控
3. 难以做单元测试（绑定了真实网络）

atomcode 的做法是手写字节级解码器，用录制字节做测试。

### 决策

在 `capabilities/src/provider/sse.rs` 手写 `SseDecoder`：

- 纯函数：`feed(&[u8]) -> Vec<RawEvent>`
- 有状态：保留半包字节，等待下一次 feed
- UTF-8 安全：只在字符边界切分
- 识别 `[DONE]` 标记
- 完全无网络依赖，可用录制字节单测

provider 层用 `reqwest` 的 `bytes_stream()` 喂给解码器。

### 后果

✅ **正面**：
- 可测试性极强：半包、半 UTF-8 字符、多工具分片、`[DONE]`、SSE comment 行都能测
- 不引入额外依赖
- 重连/重试策略完全在 provider 自己手里
- 不同 provider（OpenAI 兼容 / Anthropic）都复用同一个解码器

⚠️ **负面**：
- 需要自己处理边界情况（但这也是为什么要写单测的原因）
- 新人可能不知道 SSE 规范细节，但代码+注释+单测足够说明

### 替代方案

- `eventsource` crate：绑定网络、难测试、多一个依赖，否决
- `reqwest-eventsource`：稍好但仍绑定 reqwest 层，不如纯函数灵活，否决

---

## ADR-004：native-tls 而非 rustls

**日期**：2026-09-23（M1，完成报告 9.3 节）
**状态**：现行

### 背景

最初计划用 rustls（纯 Rust TLS）。但本机开发环境是 windows-gnu 目标且**没有可用的 C 编译器**，rustls 的 `ring`  crate 需要 C 编译。

### 决策

改用 `native-tls`：

- Windows 走系统 SChannel
- macOS 走 Security.framework
- Linux 走 OpenSSL（系统库）

`reqwest` 用 `default-features = false, features = ["native-tls", "stream", "json", "charset", "http2"]`。

同时用 `scripts/bootstrap-tools.ps1` 解决 windows-gnu 的 `raw-dylib` 链接问题（dlltool 需要同目录 `as.exe`）。

### 后果

✅ **正面**：
- 纯 Rust 绑定，零 C 编译器依赖
- Windows 用系统 TLS，证书管理跟随系统

⚠️ **负面**：
- Linux 上依赖系统 OpenSSL，静态编译稍麻烦（但不是主要目标平台）
- 不能用 rustls 的一些高级特性（如自定义证书校验更灵活）
- 跨平台行为略有差异（各家 TLS 实现细节不同）

### 替代方案

- **装 MSVC 工具链**：需要 VS 安装，本机不想装，否决
- **用 ring 的预编译二进制**：版本匹配问题多，否决
- **换 MSVC 目标**：与 dlltool 问题耦合，且项目约定是 gnu 目标，否决

---

## ADR-005：脏标记驱动渲染，无固定帧率

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

传统 TUI 框架（和很多游戏引擎）用固定帧率（如 60fps），每帧重绘。但对于 AI agent 这种"大部分时间在等 API 响应"的场景，固定帧率完全是浪费——空闲时也在空转。

同时 opencode（TS 版）因为 60fps + Solid 全树 reconcile，CPU 占用偏高，这正是 mrorCode 要解决的问题。

### 决策

采用**脏标记 + 尾沿合批**模型：

1. 任何输入/事件只置 `dirty = true`，不立即重绘
2. 首个脏信号启动一个 ~8ms 的尾沿定时器
3. 期间到达的所有 delta 全部合并
4. 到点重绘一帧并清脏
5. 无事件时事件循环完全阻塞在 `select!`，**0 唤醒**
6. Spinner 动画只在有 running 工具时才启用独立 100ms 慢 tick，工具结束立即取消

### 后果

✅ **正面**：
- 空闲时 0 CPU 占用（实测验证）
- 流式输入下自动合批，不会每 token 一帧（也不会浪费）
- 8ms 合批延迟对用户不可感知（~120fps 等效）

⚠️ **负面**：
- 实现比固定帧率略复杂（需要跟踪脏状态、定时器启停）
- 动画（spinner）需要单独处理节拍
- 极端情况下（连续不断的事件）可能一帧延迟略高，但 8ms 可接受

### 替代方案

- **固定 60fps**：空闲浪费 CPU，否决
- **每事件一帧**：高 token 速率下帧过多，CPU 浪费，否决
- **双帧率（空闲慢 / 流式快）**：不如脏标记简洁，否决

---

## ADR-006：不做 unicode-width 缓存

**日期**：2026-09-23（M1，完成报告 9.2 节）
**状态**：现行

### 背景

直觉上，`unicode-width` 的字符宽度查询可能是热点，加个全局缓存应该会更快。M1 初期实现了 `Mutex<HashMap>` 缓存。

### 决策

**实测后否决了缓存方案**。基准测试显示：

- 缓存版：~455 ns / 查询
- 裸 `unicode-width`：~47 ns / 查询

缓存反而慢了约 10 倍——锁的开销 + hash 计算远大于查表本身。

保留 ASCII 快路径（ASCII 字符宽度直接算，不进 unicode-width 表），其余直接查表。

### 后果

✅ **正面**：
- 代码更简单（没有缓存失效、并发等问题）
- 速度更快（反直觉，但数据说话）

⚠️ **负面**：
- 极端混合宽字符串场景下可能仍是热点，但目前基准显示万行帧 ~0.5ms，远低于 16ms 预算

### 教训

**先量化再优化，禁止凭感觉加缓存。** 这一条已写入项目风险表和文化规范。

---

## ADR-007：ratatui + 自研 store，不用自研渲染器

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

atomcode 的 TUI 是**完全自研**的 retained 渲染器（crossterm 之上，单 event_loop 约 1 万行）。这条路最终可以做得很极致，但前期工作量巨大。

mrorCode 的目标是"快速达到可用体验"，需要在"从零自研"和"用成熟框架"之间选一个平衡点。

### 决策

- **渲染层**：用 [ratatui](https://github.com/ratatui-org/ratatui) 0.30（crossterm 0.29 后端）
- **状态层**：自研 Elm 式 store（`AppState` + `update()` + `render()` 纯函数）
- **组件层**：自研 widgets（markdown、tool_card、diff_viewer 等），不依赖第三方组件库

### 后果

✅ **正面**：
- ratatui 成熟稳定，基本布局/样式/文本处理开箱即用
- 自研 store 保证状态管理完全可控，不受框架限制
- 自研 widgets 可以深度优化（如增量 markdown 解析、窗口化渲染）

⚠️ **负面**：
- 复杂组件（markdown、diff、表格）需要自己写
- ratatui 的 immediate mode 模型意味着每帧都会调用 widget 函数——但通过缓存和窗口化，性能仍然足够

### 替代方案

- **完全自研渲染器**（atomcode 路线）：工作量太大，M1 做不完，否决
- **用 ratatui + 第三方组件库**：组件库质量参差，且 markdown/diff 这类定制需求还是要自己写，否决

---

## ADR-008：Markdown 增量解析 + 窗口化渲染

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

AI 输出是流式的，每来一个 token 就重解析整篇 markdown 会很慢。同时长对话下消息列表可能有上万行，全量渲染不可行。

### 决策

两层优化结合：

**1. 增量解析**（block 粒度）：
- 已闭合的 block 解析一次后永久缓存（block 下标 → spans）
- 流式追加时只把最后一个未闭合 block 的原文重新解析
- delta 只改最后一个 block，已渲染历史零开销
- 折行结果按 `(block 下标, 渲染宽度)` 缓存，宽度不变时滚动不重新折行

**2. 窗口化渲染**（消息列表）：
- 渲染前先按缓存的每 block 行高算累计偏移
- 只对"可视行区间 ± 一屏余量"内的消息执行布局/绘制
- 视口外的消息不产生任何 Cell 写入
- 滚动只改变偏移量，不重排不可见内容

### 后果

✅ **正面**：
- 流式帧极快（增量 markdown ~0.2ms）
- 万行消息滚动稳定（窗口化单帧 ~0.5ms）
- 内存增量合理，没有全量结构克隆

⚠️ **负面**：
- 实现复杂度较高（需要跟踪 block 边界、缓存失效、窗口化偏移计算）
- 宽度变化时整块重排（但属于低频事件）

### 替代方案

- **每帧全量重解析**：太慢，200 block 就要 0.7-1.7ms，否决
- **行级增量**（每行一个独立解析器）：太复杂，收益不明显，否决

---

## ADR-009：单二进制，panic = abort

**日期**：2026-09-26（M5-c）
**状态**：现行

### 背景

M5 目标之一是分发优化。初始 release profile 用的是 `panic = "unwind"`（保留栈展开，便于测试和 panic 时恢复终端）。

评估 `panic = "abort"` 对二进制体积的影响：

- 展开版：~7.9 MB
- abort 版：~7.0 MB

体积减少约 11%。

### 决策

开启 `panic = "abort"`：

```toml
# Cargo.toml release profile
panic = "abort"
```

终端恢复逻辑改为在 `catch_unwind` 不可用的前提下，用 `Drop` + 进程退出前的清理保证。

**同时否决了 mimalloc**：本机无完整 gcc，与 ring/rustls 同约束，引入成本过高且收益不确定。

### 后果

✅ **正面**：
- 二进制更小（~7 MB）
- 少了 unwinder 代码，冷启动可能略快
- 代码路径更简单（不用考虑 unwind safety）

⚠️ **负面**：
- panic 时无法捕获栈展开，终端恢复依赖 Drop
- 调试时拿不到完整 backtrace（但 debug profile 仍是 unwind）

### 替代方案

- **保持 unwind**：体积大 11%，否决
- **mimalloc**：无 C 编译器，否决
- **更激进的 LTO / opt-level = z**：已用 thin LTO，再激进会损失性能，否决

---

## ADR-010：工具风险默认 Risky（安全默认）

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

工具调用的安全分级是核心设计之一。如果默认安全，漏判一个危险工具就可能出事。

### 决策

`Tool` trait 的 `risk()` 方法**默认返回 `RiskLevel::Risky`**：

- 明确安全的工具（read_file、glob、grep）显式覆盖为 `Safe`
- 写类工具（write_file、edit_file）显式覆盖为 `WorkspaceWrite`
- 不确定的（所有 MCP 工具默认）保持 `Risky`
- bash 用风险词表判定（只读命令降为 Safe，其余 Risky）

审批中间件按风险等级决定是否弹审批。**fail-closed**：拿不准就拒绝。

### 后果

✅ **正面**：
- 新增工具时忘了设 risk 也不会漏审批
- MCP 工具默认审批，不会因为第三方 server 有危险工具而中招
- 安全方向上的保守是正确的默认

⚠️ **负面**：
- 一些实际上安全的操作可能也会弹审批，用户体验略差
- bash 风险词表需要持续维护（新命令可能漏判）

### 替代方案

- **默认 Safe**：太危险，否决
- **三级以上分类**：粒度更细但复杂度上升，当前三级够用，否决

---

## ADR-011：审批走 RequestBroker + id 路由，不走 oneshot

**日期**：2026-09-23（M1）
**状态**：现行

### 背景

审批是一个"agent 问 driver 要答案"的过程。最直接的做法是用 `oneshot::channel`：middleware 发一个请求，带一个 sender，等 driver 通过 Respond 把值发回来。

但 atomcode 的做法是走 `RequestCtx` 的 oneshot（不进事件体），而我们的协议需要可序列化（为未来 daemon/跨进程做准备）。

### 决策

审批走 **`RequestBroker` + id 路由**：

1. middleware 调用 `Requester::request(kind, payload).await`
2. `RequestBroker` 生成唯一 id，存 `HashMap<id, oneshot::Sender>`，通过 `AgentEvent::Request { id, kind, payload }` 发给 driver
3. driver 决策后回 `AgentCommand::Respond(id, value)`
4. `RequestBroker` 收到后找到对应 sender 发值
5. middleware 的 await 被唤醒，拿到结果

oneshot 内部仍然用，但**协议层是 id + 序列化事件**，不是直接传 channel。

### 后果

✅ **正面**：
- 事件/命令都是纯数据，可序列化、可跨进程
- 未来做 daemon / 多客户端时，协议层不用改
- driver 不需要持有 channel 引用，生命周期更简单

⚠️ **负面**：
- 比直接传 oneshot 多一层 HashMap 查找
- 需要 id 生成和管理逻辑

### 替代方案

- **纯 oneshot，不可序列化**：简单但堵死了跨进程的路，否决
- **直接回调函数**：更不可序列化，否决

---

## ADR-012：会话持久化用 jsonl + snapshot 双文件

**日期**：2026-09-24（M3）
**状态**：现行

### 背景

会话需要落盘，既要支持 resume（快速加载最新状态），也要支持 recall（完整历史审计）。

### 决策

双文件格式：

1. **`<id>.jsonl`**（transcript，追加写）
   - 每行一条 `TranscriptEntry`（user / assistant_text / tool_call / tool_result / meta）
   - 只在轮次终态落盘（不每 delta 写）
   - 用于完整历史回溯

2. **`<id>.snapshot.json`**（snapshot，原子覆盖）
   - 完整 `SessionRecord`（含 Conversation）
   - 轮次结束时 `tmp → rename` 原子替换
   - 用于快速 resume

启动时优先读 snapshot，缺失则从 jsonl 重建。

### 后果

✅ **正面**：
- snapshot 加载快（一次读取即可）
- jsonl 可追加，不怕写中断（半行跳过即可）
- 双文件互为备份，一个坏了另一个能用
- 格式都是 JSON，人类可读，易调试

⚠️ **负面**：
- 两个文件要保持同步，写的顺序要小心（先写 jsonl 再写 snapshot）
- jsonl 会越来越大（但不影响启动速度，因为启动读 snapshot）
- 空间占用是单文件的约 2 倍

### 替代方案

- **单文件 jsonl，每次从头读**：简单但长会话启动慢，否决
- **sqlite**：太重，且需要 C 依赖（rusqlite 要 libsqlite3），否决
- **单一 snapshot，没有 transcript**：丢失完整历史，否决
