# rocode-session

文档基线：v2026.2.27（更新日期：2026-02-27）

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
- `prompt/`：提示词主循环与子模块（`mod.rs`、`shell.rs`、`subtask.rs`、`tools_and_output.rs`、`hooks.rs`）
- `compaction.rs` / `summary.rs`：压缩与摘要
- `revert.rs` / `snapshot.rs`：回滚与快照
- `status.rs` / `todo.rs`：状态与待办

## 当前分支变化（v2026.2.27）

- `llm.rs` 已移除，MCP 相关能力改由 `mcp_bridge.rs` 统一桥接到工具执行链。
- `compaction.rs` 与插件 `SessionCompacting` hook 对齐，可从 hook payload 注入自定义 compaction prompt/context。
- `message_v2` 路径成为压缩和工具状态的核心输入，支持 tool part `pending/running/completed/error` 及 compacted 时间戳。
- `SessionMessage` 新增 `finish: Option<String>`，用于显式记录 provider finish reason（如 `stop`、`tool-calls`）。
- prompt loop 的提前退出条件改为基于 `finish` 与消息 index 顺序判断，修复“模型先产出文本再调用工具”时提前 break 的问题。
- `FinishStep` 事件会把 finish reason 写入 assistant message；`tool-calls/tool_calls/unknown` 会继续下一轮，终止理由才结束回合。
- 已补充针对 early-exit 的回归测试：`finish=tool-calls` 不退出、`finish=stop` 退出、`finish=None` 不退出。
- `prompt/` 逻辑拆分为 `file_parts`、`message_building`、`tool_calls`、`tool_execution` 四个子模块，降低主循环复杂度并便于定向测试。
- 新增工具执行前预校验：`write` 缺少 `file_path` 或 `content` 时直接转 `invalid`，避免进入执行层后重复失败。
- 工具参数历史写回统一走 `sanitize_tool_call_input_for_history`，不可恢复 payload 会写入可诊断对象，减少后续回放污染。

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
