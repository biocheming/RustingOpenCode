# rocode-plugin

文档基线：v2026.2.26（更新日期：2026-02-26）

`rocode-plugin` 提供全局 Hook 系统，以及 TS 插件子进程桥接能力。

## 主要职责

- 定义 Hook 事件与上下文
- 注册/触发 Hook（含收集返回值）
- 提供全局插件系统实例
- 管理 TS 插件子进程（JSON-RPC）

## 核心类型

- `HookEvent`
- `HookContext`
- `HookOutput`
- `Hook`
- `PluginSystem`
- `PluginRegistry`

## 关键事件（节选）

- `ToolExecuteBefore` / `ToolExecuteAfter`
- `ToolDefinition`
- `ChatSystemTransform` / `ChatMessagesTransform` / `ChatHeaders`
- `CommandExecuteBefore`
- `PermissionAsk`
- `SessionCompacting`

## 子进程桥接

- 目录：`crates/rocode-plugin/src/subprocess`
- 职责：插件发现、子进程生命周期、hook 转发、auth bridge

## 当前分支变化（v2026.2.26）

- Hook 系统区分可缓存事件（如 `ConfigLoaded`、`ShellEnv`）与 fire-and-forget 事件（如 `SessionCompacting`、`Error`），并内建缓存失效能力。
- 子进程 RPC 写入/读取统一纳入超时控制（默认 30 秒），降低插件 host 卡住时主流程阻塞风险。
- JS Runtime 检测支持 `ROCODE_PLUGIN_RUNTIME` / `OPENCODE_PLUGIN_RUNTIME` 覆盖，默认优先 `bun > deno > node(>=22.6)`。
- 插件子进程启动后会注册到 `rocode_core::process_registry`，用于 TUI 进程面板可视化与手动终止。
- 本轮插件接口未新增破坏性字段，重点是与会话 `finish` 语义对齐后的显示链路兼容。

## 开发建议

- 需要插件修改输出时，调用方应使用 `trigger_collect()` 并应用返回 payload
- Hook input/output 字段应按事件语义分离，避免把 context 全量透传
- 全局系统初始化应只做一次并保证入口一致

## 验证

```bash
cargo check -p rocode-plugin
```
