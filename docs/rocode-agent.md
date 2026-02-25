# rocode-agent

文档基线：v2026.2.25（更新日期：2026-02-25）

`rocode-agent` 负责 Agent 的定义、注册、执行编排与消息封装。

## 主要职责

- 维护 Agent 信息与能力元数据
- 驱动 Agent 执行流程（调用 provider / tool / permission）
- 生成和转换 Agent 侧消息结构

## 当前分支变化（v2026.2.25）

- `AgentExecutor` 增加 subsession 持久化能力：支持 `with_persisted_subsessions()` 注入、`export_subsessions()` 导出。
- 执行链路中对 `ToolError::InvalidArguments` 做统一兜底：自动转发到 `invalid` 工具返回结构化错误，避免主循环卡死。
- 执行流程显式触发插件 hook（如 `SessionStart`、`ChatSystemTransform`），并在 `ToolContext` 中注入 subsession 回调。

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
