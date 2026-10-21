# 三库对标战略评审（mrorCode vs atomcode vs opencode）

> 评审日期：2026-10-19
> 基线：`b961a10`（v0.5.60，工作树干净）
> 范围：**战略层面对比**——架构方向、功能成熟度、能力缺口、优先级建议。
> 微观代码缺陷（文件:行号级 bug、安全绕过、会话持久化问题）已另立
> [todos/20261019-code-review-findings.md](todos/20261019-code-review-findings.md)，本文不重复，
> 仅在相关处交叉引用。
> 调研方式：mrorCode 读源码 + roadmap/todos；atomcode（D:\atomcode v5.2.1）与 opencode
> （D:\opencode v2.0.29）派只读子代理扫描架构、功能清单、可借鉴点。

---

## 一、三库定位对照

| 维度 | mrorCode（本项目） | atomcode（D:\atomcode） | opencode（D:\opencode） |
|---|---|---|---|
| 性质 | 快速单二进制 TUI agent | 成熟商业级 agent 平台 | 生态型 agent 框架（30 包） |
| 规模 | 6 crate，约 2.6 万行 Rust | 14 crate，大型 Rust 工作区 | Bun/TS monorepo，多语言 |
| 后端分层 | kernel→capabilities→agent→cli/tui/config | kernel→capabilities→coding(唯一 runtime)→cli/tuix/daemon | Effect Service core + HTTP server |
| TUI 栈 | ratatui 0.30 + 自研 Elm store（脏标记驱动） | crossterm 自研 retained 渲染器（~3000 行差分网格） | SolidJS + OpenTUI |
| Provider | openai_compat + anthropic（2 家） | 13 家 preset，账号/模型 profile 分离 | 33 家，models.dev 目录快照 |
| MCP | stdio JSON-RPC | stdio / HTTP / OAuth PKCE / trust / 动态工具目录 | stdio / remote OAuth / resources |
| 工具集 | read/write/edit/grep/glob/bash/task | 内置 + codeintel(LSP/tree-sitter) + askpass + 子 agent driver | read/write/edit/patch/glob/grep/shell/subagent/question/skill/webfetch/websearch |
| 扩展 | 无 | hooks(TOML/CC 兼容)、skills、plugin marketplace、memory、ACP 2.0 | 插件(Promise/Effect/TUI)、markdown 约定、skills、CodeMode、SDK |
| 会话 | snapshot + jsonl transcript + resume + rewind | meta+snapshot+append-only transcript+lease+跨进程互斥+repair | 事件溯源 + inbox 排队 + revert/fork/move + SQLite 投影 |
| Compaction | `Conversation::compact` 简单折叠（keep_recent） | cache-friendly 单调 stub、turn 边界触发、net-loss guard、sacred floor | 内置 compaction 策略 |
| 分发 | gitcode Linux CI（已删）+ 本地 Win 手工 | docker 三镜像、updater 自替换、多平台 release GH | install 脚本/homebrew/scoop/AUR/nix/npm |
| IDE/扩展 | 无 | VS Code（TS+React）+ JetBrains（Kotlin）现役 | VS Code、Electron desktop、Web/PWA、GUI extension SDK |
| 遥测/更新 | 无 | 7 事件脱敏遥测 + 自更新器 | 无 |

---

## 二、已验证正确的架构方向（保持，勿回退）

以下是从两个成熟产品反推、并已在 mrorCode 落地的正确决策，后续不要因"opencode/atomcode 有"而动摇：

1. **后端分层抄 atomcode，TUI 交互抄 opencode，渲染层不用自研**。
   atomcode 的 retained 渲染器（`capabilities/src/render/retained.rs` 约 3000 行）是 5.0 商业产品的复杂度；
   mrorCode 立项时明确否决，用 ratatui + Elm store 在 M1 就兑现了帧预算（万行消息 ~0.4–1ms，远低于 16ms）。
2. **单进程内类型化事件**：TUI 直接持有 `RuntimeHandle`，零 JSON 序列化、零网络往返、零 worker 冷启动。
   这是立项动机（"opencode 太慢"）的架构兑现，也是当前对 opencode 的真实速度优势。
3. **内核中立 + 单一 runtime owner**：`AgentCommand/AgentEvent` 全序列化、UI 只投影不另建状态机——
   与 atomcode 的"driver 不持有第二份状态"、opencode 的"TUI 通过 SDK 读 event feed"同构。
4. **手写 SSE 解码（无 eventsource 库）+ 录制字节单测**，与 atomcode 的 `sse.rs` 思路一致；
   provider conformance testkit（`mrorcode-kernel/test-support` 的 `stream_signature`/`assert_event_sequence`）
   是直接对标 atomcode conformance kit 的正确动作。
5. **headless JSONL 冻结契约（`SCHEMA_VERSION=1`，黄金文件守护）**——
   atomcode 同样以版本化 `HeadlessEvent` 驱动它的 Python evals/；这条资产被低估，后续 evals 直接建立在它之上。
6. **性能门禁文化**："先量化再优化，无 benchmark 不加缓存"，unicode-width 缓存被实测慢 10× 删掉的先例
   写进 AGENTS.md，这是 atomcode/opencode 项目里都没明文写的工程纪律。

---

## 三、P0：当前就需要处理（半天内）

> 微观细节已记录在 [code-review-findings.md](todos/20261019-code-review-findings.md)「零、门禁现状」，
> 本节只列战略影响。

### 1. 红灯测试 + fmt 失败（CI 缺位下混进 develop）

- 实测 `cargo test --workspace`：**1 失败**——`app::tests::help_dialog_lists_real_keybinds_and_closes`
  断言 help 含 `"leader t"`（`app.rs:1894`），但 `7868358`（移除主题切换）已删该绑定，测试未同步。
- `cargo fmt --check` 失败：`kernel/tests/agent_loop.rs`、`tui/src/widgets/permission.rs`。
- AGENTS.md 首行"282 tests, all green"已不成立。

### 2. CI 被删除（M5-a 白建）

提交 `b2587ff` 移除了 `.gitcode/workflows/ci.yml` 与 `release.yml`，当前仓库无任何 CI 配置。
上述红灯测试/fmt 漂移就是在无门禁下混进来的。**建议补回最小 Linux CI**：
`fmt --check` + `clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace`，
托管在 AtomGit/GitHub Actions 或 gitcode 均可；Windows 验证维持本地手工（沿用 M5-a 决策）。

### 3. 文档与现状大面积漂移

| 文档 | 过期说法 | 现状 |
|---|---|---|
| `AGENTS.md` | "282 tests, all green" | 1 失败 + fmt 失败 |
| `AGENTS.md` Gotchas | "themes/*.json 33 个 vendored" | 只剩 `opencode.json` 1 个（7868358 决策） |
| `AGENTS.md` | gitcode CI 存在 | 已删 |
| `project-plan-atomcode.md` | 版本 0.5.3、M5 待开工 | 0.5.60，M5 全完 |
| `roadmap.md` | 终止于 M5 实施记录 | v0.5.7–0.5.60 的 v0.5.11–v0.5.25 增量（后台子 agent、timeline、文件回滚、三档权限、三阶段弹窗、单主题、CI 撤除）只在 `docs/todos/` 和 commit 里 |

**建议在 `roadmap.md` 末尾追加 M6 章节**收口 v0.5.7–v0.5.60，避免项目知识只存在于 git log。

---

## 四、P1：安全短板（优先级高于新功能）

三档权限模式（ReadOnly/Workspace/Full）+ 三阶段权限弹窗（`b961a10`）的**交互外壳**已经对齐 opencode，
但**内核**仍是"bash 词表 + 内存 grant"，外壳与内核不匹配。详见微观文档 P1 节；战略上要补的是：

### 4.1 bash 不是工作区沙箱（真实绕过路径）

- 实测分类：`cat a.txt | grep x` → **Safe 自动放行**。`~/.ssh/id_rsa`、含 API key 的 config、
  `cp ~/.mrorcode/config.toml /tmp/x` 都能被只读模式读出。**path_guard 只管内置文件工具，管不了 shell**。
- atomcode 的解法（`capabilities/src/tools/bash_workspace_gate.rs`、`permission_rules.rs`、
  `docs/security/permission-model.md`）值得直接照搬：
  1. cat/cp/head/tail/less/sed/awk 等**文件命令按参数路径做与 path_guard 同源的工作区判定**；
  2. ReadOnly 档拦截工作区外读，不只是拦写；
  3. 敏感路径清单（`.env`、`.ssh`、凭据）无论档位一律审批；
  4. 引入 allow/deny 规则（通配符，兼容 Claude Code `settings.json`）。

### 4.2 "始终允许"只活在内存里

`InMemoryPermissionStore` 重启即丢，provider 切换重建 assembly 又换一遍 store（见微观 P1-6）。
三阶段弹窗里"始终允许"按钮的 UX 已经做好，但授权寿命应落到 `~/.mrorcode/settings.toml`，
并按"工具名 + 参数摘要"粒度授权（当前非 bash 工具一次 always 等于任意参数都放行）。

### 4.3 其他已记录的高危微观缺陷（不在此展开）

取消时悬空 tool_calls 导致下次 400（P0-1）、resume 丢 system prompt（P0-2）、多会话事件归属错位（P0-3）、
会话 id 重启覆盖（P0-4）、bash 输出无上限 OOM（P1-4）、MCP 无超时不可取消（P1-5）——
这些都在微观文档里带文件:行号，属于 M6 必修。

---

## 五、P2：能力补齐清单（性价比最高的一批）

按"日常编码体验提升 / 实现成本"排序，全部长在 mrorCode 现有 seam 上，不引入架构风险：

| # | 功能 | 为什么值得 | 落点（现成 seam） | 参考 |
|---|---|---|---|---|
| 5 | **`question` / `request_user_input` 结构化提问工具** | agent 歧义处主动发起选择题，而不是瞎猜。kernel **已有现成通道**：审批 oneshot 扩展一种 request kind + TUI 一个选择对话框即可 | `kernel::request::RequestCtx`、TUI 对话框栈 | opencode `tool/plugin/question.ts`；atomcode `docs/plans/2026-08-07-request-user-input-review-design.md` |
| 6 | **web_fetch / web_search 工具** | 查文档、查报错的刚需。reqwest 已在依赖树，HTML→text 一行处理；对 URL 做内网/元数据地址黑名单 | `capabilities/src/tools/` 新增 | opencode `webfetch`/`websearch` |
| 7 | **Hooks 系统** | `pre_tool`（可 deny/改参数）、`post_tool`、`post_turn`、`system_prompt`；TOML 声明 + shell 脚本；兼容 Claude Code settings.json 钩子格式可白捡用户 | kernel `ToolMiddleware::before/after` seam——hooks 本质是"脚本版 middleware" | atomcode `capabilities/src/hooks.rs`、`cc_hooks.rs`、`docs/hooks.md` |
| 8 | **Skills（SKILL.md 目录约定）** | `.mrorcode/skills/` 与 `~/.mrorcode/skills/` 下放 markdown 指令目录，按模型需要注入系统提示；零新基础设施 | `assemble.rs` persona 拼接 | opencode `.opencode/skills/`；atomcode capabilities/skills |
| 9 | **Markdown 化扩展（自定义 agent/command）** | `.mrorcode/agent/*.md`（自定义 persona）、`command/*.md`（自定义斜杠命令） | 现有斜杠命令注册表 + 文件发现 | opencode `.opencode/agent`、`command` |
| 10 | **MCP 补 HTTP transport + 授权记忆 + 工具表回合内冻结** | 现状仅 stdio；远程 MCP 越来越多是 streamable HTTP；工具在回合中途消失/出现会破坏配对 | `capabilities/src/mcp/` | atomcode `mcp/registry.rs` + ADR 0002；继续手写协议，不引 rmcp |
| 11 | **Compaction 升级** | 现状是简单折叠；长对话是 coding agent 核心场景。atomcode 的 cache-friendly stub（旧工具结果原地 stub、单调不回胀、net-loss guard、sacred floor）针对前缀缓存优化，对真实 provider 调用成本/质量都有帮助 | `kernel/src/message.rs::compact` | atomcode `capabilities/src/compaction.rs`、`docs/compact-durable-checkpoint-design.md` |
| 12 | **上下文管理闭环** | `context_window()`/`is_context_overflow()` 无调用方；阈值固定 100k；CJK token 估算 chars/4 中文约低估 4 倍 | `kernel/src/provider.rs`、`agent.rs` turn loop | 微观 P2 节 |

---

## 六、P3：中长期与"明确不做"

### 建议做（按真实需求排，不提前）

- **Evals 回归 harness**：headless JSONL v1 冻结是被低估的资产。抄 atomcode `evals/`
  （Python prepare/run/judge，events.jsonl 为权威流），攒 20–30 个真实任务用例，
  改 prompt/压缩逻辑时防回归——比任何新功能都保值。
- **会话 fork、QUEUED 排队提交、SubagentFooter 父子导航**：TUI 对标文档 P3 长尾，按需取。
- **Provider preset 与账号/profile 分离**：现状 2 家、配置扁平。atomcode 教训是趁早把
  "账号（连接+凭据）/模型 profile（模型+额度）"拆开（`docs/plans/2026-07-26-...`），
  晚做要数据迁移。可先内置 5–8 家常见端点 preset（DeepSeek/GLM/通义/Kimi/智谱/Ollama），不碰鉴权。
- **侧边栏 MCP/Modified Files 分区、宽屏 split diff、frecency 补全排序**：TUI 对标文档已列存量项，继续按那份文档节奏走。

### 明确不做（继续坚持，有明确需求前不碰）

- **自研 retained 渲染器**：atomcode 的 3000 行差分网格是商业产品复杂度，与"快速单二进制"立身之本冲突。
- **插件/slot 扩展系统**：TUI 对标文档已标"明确不做"。
- **daemon / ACP / IDE 扩展 / updater / 遥测**：atomcode 作为商业平台的复杂度；多客户端需求出现前不引入。
- **opencode 的持久内嵌 PTY / Composer 三标签**：大工程，后置。
- **opt-level="z"、mimalloc、ring/rustls**：本机无 C 编译器约束下已决策，不重启议题。

---

## 七、战略判断（三条）

1. **守住速度叙事，但要把"快"变成可验证的数字**。立项动机是 opencode 慢，架构（0 唤醒、窗口化、
   增量 markdown）确实兑现了帧预算。但 TTFFD < 50ms、1000 token/s 下 CPU < 15% 两项从 M1 起一直是
   "架构上满足、从未实测"。建议补一个 release 二进制冷启动实测脚本（PowerShell `Measure-Command`
   跑 30 次取 p95），把速度从信仰变成 README 里的数字；同时补"长消息流式"bench 用例（微观 P2 已指出
   `MarkdownView::update` 每次 delta 全量 split_blocks，并非真正增量）。

2. **安全模型是当前最薄弱的一环，优先级应高于功能**。三档权限 + 三阶段弹窗的交互外壳已很精致，
   但内核（bash 词表、内存 grant、无敏感路径）挡不住真实绕过。建议 M6 的核心是**P1 安全项**，
   而不是继续加 TUI 花活。

3. **M6 建议 = "硬化第二轮"**：①修红测试 + fmt + 恢复最小 CI；②bash 路径门 + 敏感路径 +
   规则化持久授权；③question 工具；④hooks；⑤roadmap 收口 v0.5.7–0.5.60；⑥微观 P0-1~P0-7
   的会话正确性缺陷（取消悬空 tool_calls、resume system prompt、事件归属、会话 id、updated_at、
   transcript 落盘、全局 busy）。这组全是 atomcode 验证过、且都长在现有 seam 上的东西，
   不引入架构风险；做完项目就从"demo 级对标"进入"可日用"阶段。

---

## 附：本次评审的工具与边界

- 本项目：读 `docs/roadmap.md`、`docs/todos/`、`docs/project-plan-atomcode.md`、
  `crates/*` 目录结构、最近 60 条 commit、实跑 `cargo test --workspace`。
- atomcode / opencode：派两个只读子代理分别扫描（28 轮 / 55 轮工具调用），
  报告见会话历史；所有结论在本文标注了对应文件路径，可回查。
- 本文未修改任何源码；微观代码缺陷以 [todos/20261019-code-review-findings.md](todos/20261019-code-review-findings.md) 为准。
