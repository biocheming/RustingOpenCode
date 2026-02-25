# rocode-mcp

文档基线：v2026.2.25（更新日期：2026-02-25）

`rocode-mcp` 实现 MCP（Model Context Protocol）客户端体系，支持多传输协议和 OAuth。

## 主要职责

- MCP 客户端连接与会话维护
- 工具同步与注册
- OAuth 鉴权状态管理
- SSE/HTTP/stdio 传输抽象

## 模块结构

- `client.rs`：客户端与注册表
- `tool.rs`：MCP 工具封装
- `oauth.rs` / `auth.rs`：OAuth 与认证流程
- `transport.rs`：传输层（HTTP/SSE/Stdio）
- `protocol.rs`：JSON-RPC 协议结构

## 当前分支变化（v2026.2.25）

- `transport.rs` 的 `StdioTransport` 使用 Content-Length 帧进行 JSON-RPC 读写，行为与 MCP stdio 规范一致。
- `HttpTransport` 同时支持普通 JSON 响应与 `text/event-stream` 响应体（POST 返回 SSE 分片），会把事件缓冲到统一接收队列。
- 传输层对 HTTP 非 2xx 与协议解析错误统一映射为 `McpClientError::TransportError/ProtocolError`，便于上层分类处理。

## 关键导出

- `McpClient` / `McpClientRegistry`
- `McpToolRegistry`
- `McpOAuthManager` / `OAuthRegistry`
- `MCP_TOOLS_CHANGED_EVENT`

## 使用场景

- CLI `rocode mcp ...`
- server `/mcp/*` 路由
- tool/session 动态扩展工具能力

## 验证

```bash
cargo check -p rocode-mcp
```
