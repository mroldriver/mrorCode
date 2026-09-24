# mrorCode M2 实施计划：TUI 核心体验（opencode 对标度拉满）

> 前置：M1 已完成（见 [mrorcode_m1_plan.md](mrorcode_m1_plan.md) 第九节）。
> M2 聚焦第八节路线图中的 TUI 体验项；会话磁盘持久化/MCP/编辑工具仍在 M3。

## 一、M2 目标（验收标准）

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

### 非目标（仍归后续里程碑）

- 会话磁盘持久化/resume（M3）；切换会话的内容目前仅进程内，退出即失（UI 明示）。
- 跨 provider 切换（base_url/api_key 不同的条目切换需要重建 runtime，M3 随 daemon 一起做）。
- @补全/斜杠命令输入层（M3，依赖 tui-textarea atomic ranges）。
- diff viewer、reasoning 折叠、子 agent 视图、鼠标（M4）。

## 二、架构改动要点

### 2.1 kernel：多会话（in-memory）+ 每轮模型覆盖

- `AgentCommand::SendMessage` 改为 `Submit { session_id: u64, text: String }`；
  新增 `SetModel(String)`。
- `AgentState.conversation: Conversation` → `conversations: HashMap<u64, Conversation>`；
  turn 开始按 session_id 取/建会话，结束归还。同一时刻仍只允许一个 turn
  （M2 不做并发会话，busy 时对其他 session 的 submit 进 pending 队列）。
- `ChatOptions` 增加 `model: Option<String>`；OpenAI provider 发请求时
  `options.model.clone().unwrap_or(self.model)`。
- 事件不带 session_id：runtime 知道当前 turn 属于哪个 session，转发时包一层
  `RuntimeEvent::Agent { session_id, event }`。

### 2.2 agent runtime

- `DriverCommand::Submit(text)` → `Submit { session_id, text }`；
  新增 `SetModel(String)`、`NewSession/` 不需要（id 由 TUI 分配）。
- RuntimeHandle 增加 `submit_to(session_id, text)`、`set_model(name)`。
- assemble 读 config 的 `models` 列表随 Assembly 返回（供选择对话框）。

### 2.3 config

- `ProviderConfig` 增加 `models: Vec<String>`（缺省回退单 `model`）。

### 2.4 TUI 模块（新增/修改）

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

## 三、关键设计

### 3.1 KV 存储（storage.rs）

- 路径 `~/.mrorcode/state.json`；`serde_json` 单文件，字段全部 `#[serde(default)]`（前向兼容）。
- 写入：`tmp → fsync(尽力) → rename` 原子替换；写入在 spawn_blocking 或直接同步（数据 < 4KB）。
- 键：`theme: String`、`theme_mode: dark|light`、`sidebar: bool`、
  `model: Option<String>`、`keybinds: Vec<{keys, action}>`、`prompt_history: Vec<String>`
  （上限 50，替代 M1 的纯内存历史）。
- 读一次启动装载；写采用显式 `save()`（低频变更点，不做每帧写）。

### 3.2 主题

- 33 个 JSON 经 tui `build.rs` 从 `src/themes/*.json` 生成
  `pub static BUNDLED: &[(&str/*文件名*/, &str/*内容*/)]`（无 include_dir 新依赖）。
- 解析：`{ defs: {name: "#hex"}, theme: { token: "#hex" | {dark,light} | "transparent"|"none" | 0..1(alpha?) } }`。
  defs 可引用 defs（检测循环）；`{dark,light}` 按当前 mode 取；
  解析为 ratatui `Color::Rgb`（alpha 在 0.30 无 RGBA，按规则与背景合成或降级忽略）。
- 我们的 Theme struct 扩展到 opencode 全量 token（syntax 9 色 + markdown 全量），
  M1 未用字段补 `#[allow(dead_code)]` 保留。
- 未知 token 忽略、解析失败的主题跳过并 warning（不崩）。
- 加 `themes::parse_bench`：解析 33 个主题总耗时（应 < 数 ms，只在切换时发生）。

### 3.3 syntect 高亮

- 依赖 `syntect = { default-features = false, features = ["default-fancy"] }`
  （fancy-regex 纯 Rust，不引入 onig C 编译——本机工具链教训）。
- `SyntaxSet::load_defaults_newlines()` 一次（lazy 静态）；
- markdown 代码 block 渲染时按 info string 选语法 → `highlight_line` →
  映射 scope 前缀（comment/keyword/string/number/function/type/…）到 syntax token；
- 未命中语言走 M1 朴素 code 底色；高亮 span 进入现有 block 折行缓存
  （缓存 key 已含文本+宽度，无需额外失效）。

### 3.4 fuzzy 评分（fuzzy.rs，零依赖）

- 子序列匹配 + 加分：首字母/分隔符后命中、连续命中、前缀命中；长度惩罚。
- 返回 0..u32（0=不匹配），用于排序与高亮命中字符。
- criterion 基准：1 万条命令/文件名规模下单次过滤预算（目标 < 0.5ms）。

### 3.5 对话框栈与命令面板

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

### 3.6 键位系统

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

### 3.7 多会话 UI

- store：`sessions: Vec<SessionMeta {id, title, status}>`、`active_session: u64`；
  消息视图按 session 分桶（`HashMap<u64, Vec<MessageView>>`），切走再切回投影不丢。
- 标题取首条用户消息截断 40 字符。
- 侧栏列出会话（active 高亮、busy spinner）；新建/删除有 Confirm；
  删除会话仅删内存（M3 接磁盘时会有真正的 unlink 确认）。
- kernel 侧 session id 由 TUI 用递增 u64 分配，全链路一致。

## 四、实施步骤（每步保持编译+测试通过）

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

## 五、依赖新增

| crate | features | 用途 |
|---|---|---|
| syntect 5.3 | `default-features=false, default-fancy` | 纯 Rust 语法高亮（无 onig/C） |

fuzzy 评分、KV 存储、键位解析均自写，不引入新 crate。

## 六、风险与对策

| 风险 | 对策 |
|---|---|
| syntect 高亮拖慢流式帧 | 高亮只在闭合代码 block 做一次并入 block 缓存；未闭合围栏用朴素底色；加基准守护 |
| 33 主题质量参差（缺 token/怪颜色） | 缺 token 用 tokyonight 默认补齐；解析失败跳过并记录，不阻塞启动；切换对话框只列成功解析项 |
| 多会话改造影响 M1 稳定性 | kernel 改动最小：仅 Submit 带 id + map 替换单值；M1 六场景测试全部保留并加 id 参数 |
| leader 键与 IME/终端兼容 | leader 可配置可禁用；保留 ctrl+p 等直接绑定，leader 不是唯一入口 |
| 对话框栈与输入焦点错乱（面板过滤输入吃掉 esc/enter） | modal 层先于 base 吃键；每个 Dialog 明确声明是否吞键；TestBackend 键位序列测试 |
| KV 文件并发写（未来多进程） | M2 单进程原子 rename 足够；M5 daemon 化时升级 fs2 锁 |
| 命令闭包借用 AppState 造成生命周期地狱 | 命令用纯数据 `AppCommand` 枚举在 on_key 统一 apply，闭包不进注册表 |

## 七、M2 完成判据

- 全部测试 + clippy/fmt 绿；既有性能基准不回退，新增基准有基线；
- 手工冒烟：ctrl+p 过滤执行命令、leader 序列、主题即时切换、
  模型选择下轮生效、新建两个会话来回切换投影隔离、删除确认、help 键位与实际一致；
- 33 主题均可在选择器列出并切换（个别丑的不阻塞，解析正确即可）。

## 八、实施记录（2026-09-24 完成）

### 8.1 交付状态

- 全部 13 步（0–12）完成；工作区 **121 个测试全绿**（其中 mrorcode-tui 74 个）；
  `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --check`、
  `cargo build --release --workspace` 均通过。
- 33 个 vendored 主题全部成功解析（dark/light 双模式），可在选择器列出与即时切换。

### 8.2 实际模块落位

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

### 8.3 与计划的偏差（均为实现中收敛，无功能裁剪）

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

### 8.4 性能基线（criterion，release）

| 基准 | 结果 | 预算/说明 |
|---|---|---|
| windowed_frame / 10000_msgs | ~0.52 ms | < 16 ms 帧预算 |
| markdown/full_reparse | ~0.78 ms | 对照（每帧全排） |
| markdown/incremental_tail_block | ~0.24 ms | 流式热路径 |
| fuzzy/filter_10000_items | ~0.23 ms | 计划目标 < 0.5ms |
| theme/parse_33_bundled | ~1.16 ms | 仅切换时发生 |
| syntax/highlight_rust_200_lines | ~42.6 ms（≈53µs/行） | 仅闭合代码块一次性执行并入缓存 |

空闲态 0 定时器不变；toast 存活时临时挂 100ms 节拍，过期即撤。

### 8.5 已知遗留（归 M3+，与计划非目标一致）

- 多会话仅进程内，退出即失（删除 Confirm 文案已明示）；磁盘持久化/resume 归 M3。
- 跨 provider 切换需重建 runtime，归 M3 daemon。
- leader 超时窗口的撤销提示依赖 100ms 节拍（有 toast 时一定在跑）；
  极端情况下窗口过期视觉反馈最多滞后一个节拍，不影响正确性。
- 手工冒烟（真实终端交互）尚未执行；自动化 TestBackend 路径已全覆盖。
