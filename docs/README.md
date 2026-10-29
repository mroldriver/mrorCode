# docs 索引

本目录存放 mrorCode 的设计与规划文档。开发者上手请先看仓库根的 [AGENTS.md](../AGENTS.md)，
安装/用法/配置请看 [README.md](../README.md)。

| 文档 | 内容 | 状态 |
|---|---|---|
| [project-plan-atomcode.md](project-plan-atomcode.md) | 产品总体设计：定位、5-crate 架构、技术栈（TUI 部分为历史存档，0.6 起改为裸 ANSI REPL） | 现行（随实现维护） |
| [kernel-architecture.md](kernel-architecture.md) | 内核深度文档：核心类型、Agent 循环、事件/命令协议、权限、会话模型、Provider 抽象、test-support | 现行（随实现维护） |
| [tools-reference.md](tools-reference.md) | 内置工具协议：每个工具的调用契约（参数 schema、风险等级、返回格式、行为约定） | 现行（工具名待同步：正文仍写 `read_file`/`write_file`/`edit_file`，代码已改为 `read`/`write`/`edit`） |
| [architecture-decisions.md](architecture-decisions.md) | 架构决策记录（ADR）：每条含背景、决策、后果、替代方案；只追加不回改历史（ADR-005–008 是 TUI 时代决策，0.6 移除 TUI 后仅作历史参考） | 现行（可追溯历史） |
| [roadmap.md](roadmap.md) | M1–M4 实施计划与完成报告（M1–M4 存档），M5 硬化发布（a–e 计划 + f 对标实施记录） | 混合（历史存档 + 进行中计划） |
| [crossrepo-review-20261019.md](crossrepo-review-20261019.md) | 三库战略对标（mrorCode vs atomcode vs opencode）：架构方向、安全短板、能力补齐清单、M6 建议 | 现行（战略评审） |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 贡献指南：环境搭建、项目结构、常用命令、代码规范、提交流程、调试技巧、架构边界误区 | 现行 |
| [todos/](todos/) | 单项对标/发布待办清单，按日期命名（如 [M5 硬化发布](todos/20260928-m5-hardening-release-硬化发布待办.md)、[代码审查发现](todos/20261019-code-review-findings.md)、[REPL 排版改进](todos/20261028-repl-output-polish.md)）；**issue 关闭后删除** | 现行 |

## 维护约定

- **project-plan-atomcode.md** 描述「现在是什么样」，架构/选型/状态变化时同步更新。
- **roadmap.md** 中 M1–M4 是已完结里程碑的历史存档，保留原文与实测数据，不回改历史；
  新里程碑在末尾追加新章节（M5 为方向）。
- **todos/** 只放**进行中**的清单（首行挂 issue 链接）；issue 关闭时 `git rm` 删除对应文件，
  值得沉淀的经验先并入 roadmap 的实施记录再删——不留过期副本。
- **architecture-decisions.md** 只追加新决策、不删除旧决策；被后续变更取代的（如 0.6 移除 TUI
  后的 ADR-005–008）要在条目里注明，避免读者误当成现行方案。
- 新增/重命名/删除文档时同步更新本索引——它是 docs 的唯一入口，索引漏项等于文档不存在。
- 文档为中文（与既有注释、设计文档保持一致）；相对链接必须可跳转。
- 项目结构、构建/测试命令、架构边界或开发约定变化时，同步更新根 [AGENTS.md](../AGENTS.md)。
