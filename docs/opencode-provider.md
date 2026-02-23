# opencode-provider

`opencode-provider` 是多模型供应商适配层，负责请求构建、流式响应解析、重试和模型能力查询。

## 主要职责

- 统一 Provider 抽象接口
- 对接多厂商 API（OpenAI、Anthropic、Google、xAI、Groq 等）
- 提供模型注册、上下文窗口、能力标注
- 实现重试与流式事件处理

## 关键模块

- `provider.rs`：Provider trait 与统一调用入口
- `bootstrap.rs`：从配置/环境创建注册表
- `models.rs`：模型元数据与能力查询
- `stream.rs`：流式事件抽象
- `retry.rs`：重试策略与可重试判断
- `<vendor>.rs`：各厂商适配实现

## 关键导出

- `create_registry_from_bootstrap_config`
- `create_registry_from_env`
- `with_retry` / `with_retry_and_hook`
- `get_model_context_limit`

## 与其他模块的关系

- 被 `opencode-session`、`opencode-agent`、`opencode-cli` 直接使用
- 与 `opencode-plugin` hooks 联动（如请求参数/headers 修改）

## 验证

```bash
cargo check -p opencode-provider
```
