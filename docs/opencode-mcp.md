# opencode-mcp

`opencode-mcp` 实现 MCP（Model Context Protocol）客户端体系，支持多传输协议和 OAuth。

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

## 关键导出

- `McpClient` / `McpClientRegistry`
- `McpToolRegistry`
- `McpOAuthManager` / `OAuthRegistry`
- `MCP_TOOLS_CHANGED_EVENT`

## 使用场景

- CLI `opencode mcp ...`
- server `/mcp/*` 路由
- tool/session 动态扩展工具能力

## 验证

```bash
cargo check -p opencode-mcp
```
