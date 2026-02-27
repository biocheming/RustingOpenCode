# rocode-server

文档基线：v2026.2.27（更新日期：2026-02-27）

`rocode-server` 提供 HTTP/SSE/WebSocket 服务接口，是 CLI、TUI 与外部系统的桥接层。

## 主要职责

- 暴露统一 API 路由
- 管理会话、配置、Provider、MCP、权限、文件与搜索能力
- 提供事件流和 TUI 控制端点
- 承载 OAuth 回调、PTY 与工作区操作

## 路由分组（节选）

以 `crates/rocode-server/src/routes.rs` 为准：

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

## 当前分支变化（v2026.2.27）

- 启动阶段新增会话持久化同步：`new_with_storage_for_url()` 会从 SQLite 回放历史会话，路由变更后按需 `sync_sessions_to_storage()`。
- Provider 路由补充目录能力：`/provider/known`（models.dev 已知 provider 列表）与 `/provider/auth`（插件鉴权状态）。
- 插件鉴权链路增强：`/plugin/auth` 及 `/{name}/auth/*` 路由可主动唤醒 plugin loader，并在授权状态变更后重建 provider registry。
- 内建插件空闲回收机制：`ROCODE_PLUGIN_IDLE_SECS`（默认 90 秒，设为 0 可禁用）到期后自动关闭子进程并卸载 custom fetch 代理。
- 工具发现路由已稳定为 `/tool/ids` 与 `/tool`，供 TUI/CLI 动态拉取工具清单。
- 会话流式更新链路新增 coalescing 持久化 worker：UI 广播优先，数据库写入异步合并，降低首屏与流式卡顿。
- prompt 结束后改为 `flush_session_to_storage(session_id)` 单会话刷盘，不再每轮全量同步所有会话。
- `message_to_info` 的 `finish` 输出改为优先读取结构化字段 `message.finish`，并兼容 metadata 回退。
- 问答交互链路已固定：`/question`、`/question/{id}/reply`、`/question/{id}/reject` 由服务端统一管理 pending question 生命周期与超时。
- `ask_question` 回调在主会话与子会话（task/subsession）均已接线，TUI 可通过同一套 Question 队列完成交互回复。

## 开发建议

- 新增路由时先定义输入/输出模型，再写 handler
- 高并发路径注意避免阻塞（I/O、数据库、网络）
- 变更 API 时同步更新 CLI/TUI 侧调用

## 验证

```bash
cargo check -p rocode-server
```
