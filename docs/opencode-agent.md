# opencode-agent

`opencode-agent` 负责 Agent 的定义、注册、执行编排与消息封装。

## 主要职责

- 维护 Agent 信息与能力元数据
- 驱动 Agent 执行流程（调用 provider / tool / permission）
- 生成和转换 Agent 侧消息结构

## 模块结构

- `agent.rs`：Agent 定义与注册能力
- `executor.rs`：执行器与流程编排
- `message.rs`：消息结构与转换

## 依赖关系

- 下游依赖：`opencode-provider`、`opencode-tool`、`opencode-permission`、`opencode-plugin`
- 上游使用：`opencode-session`、`opencode-server`、`opencode-cli`

## 开发建议

- 新增 Agent 前先定义清晰的模式边界（职责、工具范围、提示词）
- 执行器改动需关注流式输出和中断行为

## 验证

```bash
cargo check -p opencode-agent
```
