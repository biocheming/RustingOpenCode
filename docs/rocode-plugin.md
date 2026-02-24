# rocode-plugin

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

## 开发建议

- 需要插件修改输出时，调用方应使用 `trigger_collect()` 并应用返回 payload
- Hook input/output 字段应按事件语义分离，避免把 context 全量透传
- 全局系统初始化应只做一次并保证入口一致

## 验证

```bash
cargo check -p rocode-plugin
```
