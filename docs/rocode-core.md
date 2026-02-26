# rocode-core

文档基线：v2026.2.26（更新日期：2026-02-26）

`rocode-core` 是工作区最底层的通用基础库，提供全局事件总线、ID 生成与进程注册能力。

## 主要职责

- 提供异步事件总线（`bus`）
- 提供统一 ID 工具（`id`）
- 提供全局进程注册表（`process_registry`）
- 作为上层 crate 的轻依赖基础

## 模块结构

- `bus.rs`：事件发布/订阅基础设施
- `id.rs`：ID 生成、解析、格式化
- `process_registry.rs`：跨模块进程注册、资源采样与终止
- `lib.rs`：统一导出

## 当前分支变化（v2026.2.26）

- 新增 `process_registry` 导出，统一跟踪 Plugin/Bash/Agent 子进程。
- 进程信息包含 `pid/name/kind/started_at/cpu_percent/memory_kb`，供 TUI 侧栏实时展示。
- Linux 下通过 `/proc` 聚合父子进程 CPU/内存；支持带 SIGTERM->SIGKILL 的终止流程。
- 本轮补充了 `/proc` 子进程枚举路径的健壮性分支处理，避免目录项解析异常影响进程树采集。

## 依赖关系

- 上游依赖：无业务依赖
- 下游使用：几乎所有业务 crate（session、tool、provider、server 等）

## 开发建议

- 避免把业务逻辑放入 core
- 新增能力时优先保证无副作用、低耦合
- 任何变更都应考虑全 workspace 影响面

## 验证

```bash
cargo check -p rocode-core
```
