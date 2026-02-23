# RustingOpenCode 文档索引

本文档集合对应 `RustingOpenCode (ROCode)` 当前代码状态（版本标识：`2026.02.23`）。

## 快速入口

- 项目总览：`README.md`
- 用户手册：`USER_GUIDE.md`
- CLI 入口：`docs/opencode-cli.md`
- TUI 入口：`docs/opencode-tui.md`
- 服务端入口：`docs/opencode-server.md`

## 模块文档

- `docs/opencode-agent.md`：Agent 注册、执行与消息封装
- `docs/opencode-cli.md`：`opencode` 命令与子命令
- `docs/opencode-command.md`：Slash Command 注册与渲染
- `docs/opencode-config.md`：配置加载与合并
- `docs/opencode-core.md`：事件总线与 ID 基础设施
- `docs/opencode-grep.md`：代码与文本搜索封装
- `docs/opencode-lsp.md`：LSP 客户端与注册表
- `docs/opencode-mcp.md`：MCP 客户端、OAuth、传输层
- `docs/opencode-permission.md`：权限规则与决策引擎
- `docs/opencode-plugin.md`：Hook 系统与 TS 子进程桥接
- `docs/opencode-provider.md`：多 Provider 模型适配层
- `docs/opencode-server.md`：HTTP 路由、事件流、控制端点
- `docs/opencode-session.md`：会话生命周期与消息流
- `docs/opencode-storage.md`：SQLite 存储与仓储层
- `docs/opencode-tool.md`：内置工具与工具注册中心
- `docs/opencode-tui.md`：终端 UI 架构与交互
- `docs/opencode-types.md`：跨模块共享数据类型
- `docs/opencode-util.md`：文件系统、日志、通用工具
- `docs/opencode-watcher.md`：文件系统监听器

## 代码与文档约定

- 命令名当前仍为 `opencode`（兼容历史脚本）。
- 文档内容优先以源码和 `--help` 输出为准。
- 涉及行为差异或重构计划，统一写入 `docs/overview/`（若该目录存在）。
