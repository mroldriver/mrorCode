# 代码审查记录：正确性、安全与上下文管理（2026-10-19）

> 审查范围：`crates/` 六个 crate 的核心路径（kernel agent / capabilities provider·tools·MCP·session / agent runtime / TUI store·app / CLI headless）。
> 基线：`b961a10`（0.5.60），工作区无未提交代码改动。
> 结论：架构分层与「kernel 单一状态所有者」主线成立；当前 HEAD 有 **1 个红灯测试 + fmt 未通过**，
> 另有若干会造成会话数据错乱、resume 丢 persona、取消后请求 400 的正确性缺陷，以及 bash
> 「Safe」判定的可绕过点。以下按优先级记录，均带文件:行号与建议修复。
>
> 状态：**待办**（本次仅记录，未改动代码）。完成一项勾一项，并在 roadmap 记录归属里程碑。

---

## 零、门禁现状（先修）

| 检查 | 结果 | 说明 |
|---|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过（exit 0） | 仅有增量缓存 hardlink 的 cargo 警告 |
| `cargo fmt --check` | **失败** | `crates/mrorcode-kernel/tests/agent_loop.rs:538/571/604/639/674`、`crates/mrorcode-tui/src/widgets/permission.rs:25/40/58/65` |
| `cargo test --workspace` | **1 失败** | `mrorcode-tui::app::tests::help_dialog_lists_real_keybinds_and_closes` |

- [x] **修复红灯测试**（2026-10-20）：`crates/mrorcode-tui/src/app.rs:1894` 断言 help 含 `"leader t"`，但
  `7868358` 删除主题切换时已删掉该绑定（现默认 leader 见 `crates/mrorcode-tui/src/keymap.rs:699-713`）。
  已改为断言 `ctrl+shift+t`（timeline 的真实绑定，`crates/mrorcode-tui/src/commands.rs:216`）。
- [x] **修 fmt**（2026-10-20）：上述两文件跑 `cargo fmt`，实际 9 处（agent_loop.rs 5、permission.rs 4）。
- [x] 同步 `AGENTS.md` 的测试数表述（2026-10-20：已改为「323 tests, all green」）。PowerShell 退出码
  属 CI 建议，本次未改脚本。

---

## P0：数据与协议正确性

### P0-1 取消后留下悬空 tool_calls，下一次请求必 400

- 位置：`crates/mrorcode-kernel/src/agent.rs:510-524`（带 `tool_calls` 的 assistant 消息已落库）、
  `crates/mrorcode-kernel/src/agent.rs:535-539`（工具循环中途取消直接 `return`）。
- 现象：取消时剩余 call 没有对应 tool 结果。OpenAI/Anthropic 均要求每个 `tool_call` 有 `tool` 消息，
  下一次提交会收到 400，会话在 rewind/压缩前不可用。
- 修复：
  1. 取消/异常退出时，为未执行的 call 补一条 `{"status":"cancelled"}` 的 tool_result；
  2. 扩展 `crates/mrorcode-kernel/src/message.rs:141-153` 的 `repair_pairing`：除删除孤儿 tool 消息外，
     还要处理「有 call 无结果」（补合成结果或整条丢弃）；
  3. snapshot 恢复路径（`crates/mrorcode-capabilities/src/session/mod.rs:95` 只在 transcript 重建时调用）
     也要跑一次修复。
- 测试：取消于多工具轮次中途 → 再提交一轮，断言历史合法（mock provider 校验配对）。

> 状态：**已修复**（2026-10-20，0.5.62）。取消时为当前及后续 call 补 `{"status":"cancelled"}` 的
> tool_result 并发出 ToolResult 事件；`repair_pairing` 扩展为同时修复「有 call 无结果」；
> `LoadSession` 入库前也跑一次修复。回归测试：
> `message::tests::repair_synthesizes_missing_tool_results` /
> `repair_synthesizes_all_results_when_tool_block_missing`、
> `agent_loop::cancel_mid_tool_round_leaves_valid_tool_pairing`。
> （注：snapshot 加载路径原本已调用 `repair_pairing`，本次只是补强该函数本身。）

### P0-2 resume / provider 切换丢 system prompt

- 位置：`crates/mrorcode-tui/src/store.rs:2100` 的 `conversation_from_view` 只映射 user/assistant，
  **从不包含 system**；`crates/mrorcode-kernel/src/agent.rs:291-293` 的 `LoadSession` 原样插入；
  `crates/mrorcode-kernel/src/agent.rs:198-205` 的 `take_conversation` 只对「新会话」补 system。
- 现象：`--resume` 后 persona、工具纪律全部消失。provider 切换更糟：空会话时
  `crates/mrorcode-tui/src/store.rs:594` 的 `active_conversation()` 返回空 Conversation，
  `crates/mrorcode-tui/src/app.rs:279` 仍会 `resume`，首次提交就没有 system。
- 修复：`LoadSession` 时若首条不是 system 且 `state.system_prompt` 存在，前插一条。
- 测试：持久化→重新加载→提交，断言 provider 收到的首条消息是 system。

### P0-3 多会话事件归属错位（`ctrl+b` 后台任务必现）

- 位置：`crates/mrorcode-agent/src/runtime.rs:143/162/168/206` 用「最后一次 Submit 的 session」
  给所有事件打标；kernel 在 `running` 时把 Submit 排队（`crates/mrorcode-kernel/src/agent.rs:312-319`）。
- 现象：A 运行中提交 B，A 的后续 TextDelta/ToolResult/TurnComplete 全被标成 B，
  TUI 把 A 的输出投影进 B（`crates/mrorcode-tui/src/store.rs:1708` 起）。
- 修复：`TurnStarted` 携带 `session_id`（或新增事件），runtime 在 TurnStarted 时切换 `active_session`；
  不能只依赖 Submit 顺序。
- 测试：A 运行中提交 B，断言事件分别落入 A/B 的 bucket。

### P0-4 会话 id 重启后覆盖旧会话

- 位置：`crates/mrorcode-tui/src/store.rs:327`（`next_session_id = 2`）、
  `crates/mrorcode-tui/src/store.rs:1038-1043`（attach 只加载历史列表，不抬升 id）、
  `crates/mrorcode-tui/src/store.rs:1124`（新建直接用 2）。
- 现象：重启后新建会话 → 覆盖磁盘上的 `2.snapshot.json`，静默数据丢失。
  两个 TUI 实例也会撞 id。
- 修复：attach 时取 `max(history ids, sessions ids) + 1`；长期改用时间戳/随机 id，
  并考虑会话目录文件锁。

### P0-5 `updated_at: 0` 让「最近会话」排序失效

- 位置：`crates/mrorcode-tui/src/store.rs:1099` 永远写 0；
  `crates/mrorcode-capabilities/src/session/mod.rs:147-152` 按 `updated_at` 倒序后按 id。
- 现象：所有快照并列 0，侧栏实际按 id 排，而非最近使用。
- 修复：写 `now_secs()`（capabilities 已有该函数）。

### P0-6 transcript 层是死代码

- 位置：`crates/mrorcode-tui/src/store.rs:1102` 只 `save_snapshot`；
  `append_conversation_tail` 仅测试调用（`crates/mrorcode-capabilities/src/session/mod.rs:204`）。
- 现象：snapshot 一损坏会话就丢；`crates/mrorcode-tui/src/store.rs:1524` 提示的
  「磁盘 transcript 保留完整历史」不成立。
- 修复：终态增量追加 transcript（记录每会话已写消息数），或删除该层并同步改文档/提示。

### P0-7 全局 busy 判定缺失，TUI 与 kernel 分叉

- 位置：TUI 只用当前会话的 `phase`（`crates/mrorcode-tui/src/store.rs:1496` 的 rewind、
  provider switch 等）；kernel 的 `Rewind` 是全局
  `running.is_none() && pending.is_empty()`（`crates/mrorcode-kernel/src/agent.rs:298-299`）。
- 现象：后台会话运行时 rewind：TUI 本地已截断，kernel 忽略 → 下一次提交上下文与界面不一致。
  `LoadSession` 在运行中也会被 turn 结束后的 `insert` 覆盖（`crates/mrorcode-kernel/src/agent.rs:334`）。
- 修复：TUI 维护全局 busy 计数（任意会话 TurnStarted++ / 终态--），或 kernel 提供查询；
  rewind 改为按 session 生效。

---

## P1：bash「Safe」是实际权限边界，目前可绕过

> `RiskLevel::Safe` 在 ReadOnly 模式直接执行、Workspace 模式跳过审批，
> 因此分类器等同于安全边界。`crates/mrorcode-capabilities/src/tools/bash.rs:245-265`。

### P1-1 换行命令注入（最严重）

- 位置：`split_segments` 只切 `& | ;`（`crates/mrorcode-capabilities/src/tools/bash.rs:286`），
  `segment_is_safe` 用 `split_whitespace` 取首 token（`:301`）。
- 现象：`ls\nrm -rf /` 被判为 **Safe** 并直接执行，ReadOnly 模式也放行。
- 修复：含 `\n`/`\r` 一律 Risky（或按行切分逐段检查），并加回归测试。

### P1-2 只读词表漏洞

- 位置：`crates/mrorcode-capabilities/src/tools/bash.rs:178`（`find`）、
  `:326`（tar 只查 `starts_with('c')`）、`:226-242`（git 子命令无参数校验）。
- 可绕过示例：
  - `find . -delete`、`find . -exec ... +`；
  - `tar xf`、`tar --create`（`-xf`/`--create` 都不以 `c` 开头）；
  - `sort -o out.txt`、`tree -o out.txt`、`rg --pre 'cmd'`、`xxd -r`、`file -C`；
  - `git branch <new>`、`git remote add/remove`、`git log --output=...`。
- 另有一个反向误判：`git stash list` 因为 args 里含 `"stash"`，在
  `crates/mrorcode-capabilities/src/tools/bash.rs:360-365` 永远返回 Risky。
- 修复：把 `find/tar/sort/tree/rg/xxd/less/more/file` 移出 Safe；git 只保留
  `status/diff/log/show/rev-parse/ls-files` 并做严格参数白名单；表驱动测试覆盖上述反例。

### P1-3 bash 不是工作区沙箱

- 现象：`read_file` 有 `path_guard`，但 Safe 的 `cat`/`type`/`find` 可读任意路径
  （`~/.ssh/id_rsa`、含 API key 的 config）。ReadOnly ≠ 只读工作区。
- 建议：至少文档明确；更好的是对 Safe 命令的路径参数做 `resolve_safe` 校验，
  越界（绝对路径/`..`）降级为 Risky。

### P1-4 bash 输出无上限，可能 OOM

- 位置：`crates/mrorcode-capabilities/src/tools/bash.rs:74-85` 用 `Command::output()` 全量读入内存；
  64 KiB 截断发生在 agent 之后。
- 现象：`yes` 或 `cat huge.log` 在 120s 内可打爆内存。
- 修复：流式读取并在上限处截断/终止进程；Windows 注意 `kill_on_drop` 只杀直接子进程，
  孙进程可能残留（job object 或 `taskkill /T`）。

### P1-5 MCP 无超时、不可取消、启动可能卡死

- 位置：`crates/mrorcode-capabilities/src/mcp/client.rs:136-161`（无 timeout）、
  `crates/mrorcode-capabilities/src/mcp/tool.rs:60`（忽略 `ctx.cancel`）、
  `crates/mrorcode-capabilities/src/mcp/mod.rs` 的 `bootstrap` 串行 await 且无超时。
- 修复：每请求 timeout + cancel select；bootstrap 加 per-server timeout/并行。

### P1-6 审批「始终允许」粒度过粗

- 位置：`crates/mrorcode-capabilities/src/approval.rs:53-66` 对非 bash 只按工具名授权。
- 现象：对 `task` 或 MCP 写工具点一次「始终允许」，之后任意参数都放行；
  provider 切换重建 assembly 会换掉 store（`crates/mrorcode-agent/src/assemble.rs:137-139`），授权又全丢。
- 建议：key 带参数摘要（path/命令），store 提升到 app 级跨 assembly 复用。

---

## P2：健壮性与一致性

- [ ] **reqwest 总超时会切断长流**：`crates/mrorcode-capabilities/src/provider/openai_compat.rs:58-59`、
  `crates/mrorcode-capabilities/src/provider/anthropic.rs:67-68` 的 `Client::timeout(300s)`
  覆盖整个响应体读取，长回答/长工具轮会中途失败。改用 `connect_timeout` + `read_timeout`（空闲超时）。
- [ ] **Anthropic 丢掉压缩注记**：`crates/mrorcode-kernel/src/message.rs:217` 把注记作为 system 插在中间；
  Anthropic 只取首条 system 到顶层，其余在 `crates/mrorcode-capabilities/src/provider/anthropic.rs:447`
  被 `Role::System => {}` 丢弃。把所有 system 合并进顶层 system，或注记改成 user 文本。
- [ ] **上下文管理没闭环**：`context_window()` / `is_context_overflow()` 无调用方
  （`crates/mrorcode-kernel/src/provider.rs:64/107`）；压缩阈值固定 100k，只在 turn 开始检查
  （`crates/mrorcode-kernel/src/agent.rs:386`）；`estimate_tokens` 按 chars/4 估算，
  中文约 1 字/token，会低估约 4 倍（`crates/mrorcode-kernel/src/message.rs:155-174`）。
  建议阈值取 `min(config, context_window*0.8)`、每 round 前检查、overflow 时强制压缩并重试一次，
  CJK 字符单独加权。
- [ ] **流式错误丢部分输出**：`crates/mrorcode-kernel/src/agent.rs:490-492` 在 Error 时已 emit 的
  TextDelta 没有落库；TUI 已显示、conversation 里却没有。把 partial assistant 以 interrupted 标记落库，
  或发 reset 事件让 TUI 回滚。
- [ ] **`write_file` 非 UTF-8 绕过读前校验**：`crates/mrorcode-capabilities/src/tools/write.rs:90`
  用 `read_to_string(...).ok()`，存在但非 UTF-8 的文件变成 `existing=None`，既不要求先读也不生成 diff。
  用 `path.exists()` 判断存在，读取失败按「不可覆盖/需先读」处理。
- [ ] **`read_file` 带 offset 仍全量读**：`crates/mrorcode-capabilities/src/tools/read.rs:122` 只在
  `offset.is_none()` 时限制 1 MiB，`:128` 无条件 `fs::read`；`offset=1` 读 5 GB 文件仍全量进内存。
  用 `BufReader` 跳行 + `take`。
- [ ] **默认 provider 不确定**：`crates/mrorcode-config/src/lib.rs:203/230/241` 用
  `HashMap::keys().next()`，多 provider 且未配 `default_provider` 时每次运行可能选到不同条目。
  改用 BTreeMap 或排序，或直接报错要求显式默认。
- [ ] **`--resume` 对 `run` 无效**：`crates/mrorcode-cli/src/main.rs:41-43` 参数说明写「TUI 与 run 均可用」，
  但 Run 分支未使用。要么 run 前 `LoadSession`，要么限定为 TUI 并改文档。
- [ ] **headless 的 `process::exit` 跳过 Drop**：`crates/mrorcode-cli/src/main.rs:134-135` 在 `assembly`
  仍存活时直接退出，MCP 子进程的 `kill_on_drop` 不执行，Windows 下遗留 npx/node。
  先 `drop(assembly)` 或返回 `ExitCode`。
- [ ] **`-C` 与 TUI 投影不一致**：main 把 canonicalize 后的 working_dir 交给 assemble
  （`crates/mrorcode-cli/src/main.rs:80-82`），但 `crates/mrorcode-tui/src/app.rs:68` 又取
  `current_dir()`；`@` 补全、git 分支、导出、rewind 回滚都用原 cwd。
  把 working_dir 放进 Assembly 或作为 `run` 参数。
- [ ] **后台会话结果不落盘**：`crates/mrorcode-tui/src/store.rs:1780` 只在
  `session_id == active_session` 时持久化；后台 turn 完成只更新 bucket，此时退出会丢结果。
  加 `persist_session(id)` 支持 bucket。
- [ ] **TUI 把 Error 当终态**：`crates/mrorcode-tui/src/store.rs:1675-1677` 与 headless 文档
  「Error 是诊断、非终态」冲突，会提前 Idle + 落盘，TurnComplete 再落一次；
  且 `TurnComplete` 不清 `self.permission`，审批 5 分钟超时后弹窗残留并吃按键。
  统一 terminal 判定并在终态清理弹窗。
- [ ] **恢复保真度**：`conversation_from_view`（`crates/mrorcode-tui/src/store.rs:2100`）把多轮
  assistant + 工具调用压成单条 assistant，工具 `Running` 状态也没有收尾。根本解法是让 kernel
  导出权威 Conversation（新增命令/事件），不要从 UI 投影反推——这也更符合 AGENTS 的
  「kernel 唯一所有者」。
- [ ] **性能**：`MarkdownView::update` 每次 delta 都全量 `split_blocks` 并为每个 block 做两次
  `block_key` 分配（`crates/mrorcode-tui/src/widgets/markdown.rs:44-63`），并非真正增量；
  `lines()` 每帧 clone 全部行（`:70-87`）。按项目规则先加「长消息流式」bench 用例，再决定是否优化。
  另：`persist_active_session` 在 UI 线程同步写，并 `store.list()` 全量反序列化所有会话
  （`crates/mrorcode-tui/src/store.rs:1102-1106`、`crates/mrorcode-capabilities/src/session/mod.rs:142`），
  每轮 O(全部历史)。

### 小项

- [ ] 无调用方的 API：`RequestBroker::emit`（`crates/mrorcode-kernel/src/request.rs:58`）、
  `denial_payload`（`crates/mrorcode-kernel/src/middleware.rs:54`）、`StopReason::Cancelled`、
  `UnavailableMcpTool`（仅测试）。删除或接线。
- [ ] retry jitter 只向下且注释写 ±25%（`crates/mrorcode-capabilities/src/provider/retry.rs:37-44`）；
  `parse_retry_after` 不支持 HTTP-date（`:52`）。
- [ ] Anthropic 忽略 `options.max_tokens` / `tool_choice`；OpenAI 的 `ToolCallStarted` 在 name
  后到时可能发空 name（`crates/mrorcode-capabilities/src/provider/openai_compat.rs:316-318`），
  导致 unknown tool。
- [ ] `Cancel` 不清 pending 队列，Esc 后排队消息仍会立刻执行（可能非预期）。

---

## 建议落地顺序

1. 门禁：红灯测试 + fmt + AGENTS 表述（半天内）。
2. P0-1/P0-2/P0-3：取消悬空 tool_calls、resume system prompt、事件归属（各配回归测试）。
3. P0-4 ~ P0-7：会话 id、updated_at、transcript、全局 busy。
4. P1-1/P1-2/P1-4：换行注入、只读词表、bash 输出上限（安全优先）。
5. P2 的 provider 超时、上下文闭环、Anthropic system 注记。
6. 其余按收益排序；性能项先补 bench 再动代码（遵循 AGENTS「无 benchmark 不加缓存」）。

## 验证命令

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

> 本次审查未修改任何源码；仅新增本文档。

---

## 审核补记（2026-10-20，文档复核）

> 对本文档本身的复核结论，供后续施工参考。全量实测：323 tests、clippy、fmt 均通过（版本 0.5.61）。

- **P0-1 建议 3 前提不成立**：`crates/mrorcode-capabilities/src/session/mod.rs:88` 在 snapshot 加载成功时
  已调用 `repair_pairing`，并非「只在 transcript 重建时调用」。真正缺口是 `repair_pairing`
  只删孤儿 tool 消息、不补「有 call 无结果」，扩展该函数即可。
- **小项修正**：`StopReason::Cancelled` 并非无调用方（`crates/mrorcode-capabilities/src/tools/task.rs:139`
  构造、`:176` 使用）。真正无调用方的是 `RequestBroker::emit`（`Requester::request_with_timeout`
  重复实现了其逻辑）、`denial_payload`、`UnavailableMcpTool`。
- **P1-6 升级为文档/实现不一致**：M5-e 记录声称新 runtime 自动继承审批授权，但
  `crates/mrorcode-agent/src/assemble.rs:138` 每次都新建 `InMemoryPermissionStore`。修复时应同步
  修正 `docs/roadmap.md` 的表述。
- **P0-7 影响面补充**：后台会话运行时切换 provider 会 shutdown 旧 runtime
  （`crates/mrorcode-tui/src/app.rs:271`），直接取消后台 turn，不只是 rewind 不一致。
- **P0-3 可复现性**：`submit_prompt` 只检查当前会话的 phase（`crates/mrorcode-tui/src/store.rs:1000`），
  `ctrl+b` 后新会话为 Idle，所以提交必然放行 → 事件归属错位必现。
- **环境注记**：本机用户 `TMP/TEMP` 下 `dlltool` 无法创建临时文件，会导致 `windows-sys` 编译失败、
  `broken_toml_produces_warning_not_panic` 出现假红灯；将 `TMP/TEMP` 指向 `target/tmp` 后门禁全绿。
