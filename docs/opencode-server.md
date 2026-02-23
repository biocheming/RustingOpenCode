# opencode-server

`opencode-server` 提供 HTTP/SSE/WebSocket 服务接口，是 CLI、TUI 与外部系统的桥接层。

## 主要职责

- 暴露统一 API 路由
- 管理会话、配置、Provider、MCP、权限、文件与搜索能力
- 提供事件流和 TUI 控制端点
- 承载 OAuth 回调、PTY 与工作区操作

## 路由分组（节选）

以 `crates/opencode-server/src/routes.rs` 为准：

- 基础：`/health`、`/event`、`/path`、`/vcs`
- 会话：`/session/*`
- Provider：`/provider/*`
- 配置：`/config/*`
- MCP：`/mcp/*`
- 文件：`/file/*`
- 搜索：`/find/*`
- 权限：`/permission/*`
- 项目：`/project/*`
- PTY：`/pty/*`
- TUI 控制：`/tui/*`
- 实验：`/experimental/*`
- 插件鉴权：`/plugin/*`

## 模块结构

- `server.rs`：服务启动与生命周期
- `routes.rs`：路由定义与处理
- `oauth.rs` / `mcp_oauth.rs`：OAuth 流程
- `pty.rs`：终端会话桥接
- `worktree.rs`：工作区相关操作

## 开发建议

- 新增路由时先定义输入/输出模型，再写 handler
- 高并发路径注意避免阻塞（I/O、数据库、网络）
- 变更 API 时同步更新 CLI/TUI 侧调用

## 验证

```bash
cargo check -p opencode-server
```
