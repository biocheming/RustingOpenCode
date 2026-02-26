# rocode-agent

文档基线：v2026.2.26（更新日期：2026-02-26）

`rocode-agent` 负责 Agent 的定义、注册、执行编排与消息封装。

## 主要职责

- 维护 Agent 信息与能力元数据
- 驱动 Agent 执行流程（调用 provider / tool / permission）
- 生成和转换 Agent 侧消息结构

## 当前分支变化（v2026.2.26）

- `AgentExecutor` 的工具执行路径改为保留完整 `ToolResult`（`output/title/metadata`），不再只传字符串，便于上层保留工具附件与结构化元数据。
- 执行流中的 `ToolResult` 事件现在回传真实工具标题与元数据，便于 TUI/Server 统一展示。
- 仍保持 `InvalidArguments -> invalid tool` 的兜底策略，避免子代理执行链卡死。

## 模块结构

- `agent.rs`：Agent 定义与注册能力
- `executor.rs`：执行器与流程编排
- `message.rs`：消息结构与转换

## 依赖关系

- 下游依赖：`rocode-provider`、`rocode-tool`、`rocode-permission`、`rocode-plugin`
- 上游使用：`rocode-session`、`rocode-server`、`rocode-cli`

## 开发建议

- 新增 Agent 前先定义清晰的模式边界（职责、工具范围、提示词）
- 执行器改动需关注流式输出和中断行为

## 验证

```bash
cargo check -p rocode-agent
```
