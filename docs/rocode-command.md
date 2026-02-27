# rocode-command

文档基线：v2026.2.27（更新日期：2026-02-27）

`rocode-command` 实现 Slash Command 系统，支持内置命令、文件命令、MCP 命令与技能命令。

## 本轮状态（v2026.2.27）

- 本轮未引入命令协议破坏性变更，注册表与上下文模型保持兼容。

## 主要职责

- 维护命令注册表（`CommandRegistry`）
- 统一命令元数据（`Command` / `CommandSource`）
- 负责模板变量替换和命令执行上下文注入
- 支持从 `.opencode/commands/*.md` 动态加载命令

## 核心类型

- `Command`
- `CommandSource`
- `CommandContext`
- `CommandRegistry`

## 内置命令

当前内置模板包括：

- `init`
- `review`
- `commit`
- `test`

## 使用路径

- CLI / TUI 的斜杠命令入口
- 服务端命令执行端点
- 插件 hook 前置处理（如 `command.execute.before`）

## 源码入口

- `crates/rocode-command/src/lib.rs`

## 验证

```bash
cargo check -p rocode-command
```
