# rocode-provider

文档基线：v2026.2.25（更新日期：2026-02-25）

`rocode-provider` 是多模型供应商适配层，负责请求构建、流式响应解析、重试和模型能力查询。

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
- `transform.rs`：消息去重、cache hint、interleaved thinking 规范化
- `<vendor>.rs`：各厂商适配实现

## 当前分支变化（v2026.2.25）

- `transform.rs` 强化了 Provider 兼容层：包含 `dedup_messages`、`normalize_messages`、`normalize_messages_for_caching`、`apply_interleaved_thinking` 等路径。
- 新增统一 `OUTPUT_TOKEN_MAX = 32000` 默认值，与 TS 侧行为对齐。
- OpenAI 适配对 `/chat/completions` 响应采用宽松解析（nullish 字段、`tool_calls.function.arguments` 容错），减少第三方兼容 API 失败率。
- Anthropic 适配支持 reasoning/thinking 内容映射，并附带 `interleaved-thinking`、`fine-grained-tool-streaming` 相关 beta header。

## 关键导出

- `create_registry_from_bootstrap_config`
- `create_registry_from_env`
- `with_retry` / `with_retry_and_hook`
- `get_model_context_limit`

## 与其他模块的关系

- 被 `rocode-session`、`rocode-agent`、`rocode-cli` 直接使用
- 与 `rocode-plugin` hooks 联动（如请求参数/headers 修改）

## 验证

```bash
cargo check -p rocode-provider
```
