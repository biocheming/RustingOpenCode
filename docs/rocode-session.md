# rocode-session

文档基线：v2026.2.25（更新日期：2026-02-25）

`rocode-session` 是会话引擎核心，负责消息流、状态机、重试、压缩、摘要、回滚和系统提示构造。

## 主要职责

- 会话生命周期管理（创建、继续、结束、中断）
- 消息组织（用户/助手/工具/系统）
- 与 provider、tool、mcp、lsp、plugin 的协调
- 会话压缩、摘要和快照
- 撤销/回滚信息维护

## 核心模块

- `session.rs`：会话实体与管理器
- `mcp_bridge.rs`：把 MCP 工具桥接为标准 ToolRegistry 工具
- `message.rs` / `message_v2.rs`：消息结构与操作
- `prompt.rs`：提示词构建
- `compaction.rs` / `summary.rs`：压缩与摘要
- `revert.rs` / `snapshot.rs`：回滚与快照
- `status.rs` / `todo.rs`：状态与待办

## 当前分支变化（v2026.2.25）

- `llm.rs` 已移除，MCP 相关能力改由 `mcp_bridge.rs` 统一桥接到工具执行链。
- `compaction.rs` 与插件 `SessionCompacting` hook 对齐，可从 hook payload 注入自定义 compaction prompt/context。
- `message_v2` 路径成为压缩和工具状态的核心输入，支持 tool part `pending/running/completed/error` 及 compacted 时间戳。

## 关键导出（节选）

- `Session`
- `SessionManager`
- `SessionEvent`
- `SessionStatus`
- `SessionSummary`

## 开发建议

- 涉及消息顺序的改动要覆盖流式与中断场景
- 与插件 hook 的输入输出字段保持稳定
- 回滚与摘要逻辑优先保证可恢复性

## 验证

```bash
cargo check -p rocode-session
```
