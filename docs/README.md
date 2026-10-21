# docs 索引

本目录存放 mrorCode 的设计与规划文档。开发者上手请先看仓库根的 [AGENTS.md](../AGENTS.md)，
安装/用法/配置请看 [README.md](../README.md)。

| 文档 | 内容 | 状态 |
|---|---|---|
| [project-plan-atomcode.md](project-plan-atomcode.md) | 产品总体设计：定位、TUI 技术选型、6-crate 架构、功能对标清单、技术栈 | 现行（随实现维护） |
| [tui-layout-spec.md](tui-layout-spec.md) | TUI 视觉布局规范（对齐 opencode 的外壳/气泡/输入框/光标/侧栏/权限弹窗/键位/主题） | 现行（改布局前必读） |
| [roadmap.md](roadmap.md) | M1–M4 实施计划与完成报告（M1–M4 存档），M5 硬化发布（a–e 计划 + f 对标实施记录） | 混合（历史存档 + 进行中计划） |
| [crossrepo-review-20261019.md](crossrepo-review-20261019.md) | 三库战略对标（mrorCode vs atomcode vs opencode）：架构方向、安全短板、能力补齐清单、M6 建议 | 现行（战略评审） |
| [todos/](todos/) | 单项对标/发布待办清单（如 [TUI 对 opencode 差异](todos/20260928-tui-parity-opencode-对标差异.md)、[2026-10-19 代码审查发现](todos/20261019-code-review-findings.md)） | 现行 |

## 维护约定

- **project-plan-atomcode.md** 描述「现在是什么样」，架构/选型/状态变化时同步更新。
- **roadmap.md** 中 M1–M4 是已完结里程碑的历史存档，保留原文与实测数据，不回改历史；
  新里程碑在末尾追加新章节（M5 为方向）。
- **todos/** 存放进行中的对标清单，随实现逐项勾选，完成后并入 roadmap 的实施记录。
- 文档为中文（与既有注释、设计文档保持一致）；相对链接必须可跳转。
- 项目结构、构建/测试命令、架构边界或开发约定变化时，同步更新根 [AGENTS.md](../AGENTS.md)。
