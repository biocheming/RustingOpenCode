# rocode-lsp

文档基线：v2026.2.25（更新日期：2026-02-25）

`rocode-lsp` 提供 Language Server Protocol 客户端能力，用于代码智能分析与编辑辅助。

## 主要职责

- 启动并管理 LSP 子进程
- 处理 JSON-RPC 请求/响应/通知
- 维护客户端状态与事件流
- 管理多语言服务器注册表

## 关键类型

- `LspClient`
- `LspClientRegistry`
- `LspServerConfig`
- `LspEvent`
- `JsonRpcRequest` / `JsonRpcResponse` / `JsonRpcNotification`
- `LspError`

## 使用场景

- `rocode-tool` 的 LSP 工具（需启用 `lsp` feature）
- TUI / Server 的 LSP 状态展示与调试

## 开发建议

- 处理超时、重试与 server 未初始化场景
- 关注 URI/Path 转换的一致性
- 对事件广播通道做背压与容量评估

## 验证

```bash
cargo check -p rocode-lsp
```
