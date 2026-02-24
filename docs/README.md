# RustingOpenCode 文档索引

本文档集合对应 `RustingOpenCode (ROCode)` 当前代码状态（版本标识：`2026.02.24`）。

## 快速入口

- 项目总览：`README.md`
- 用户手册：`USER_GUIDE.md`
- CLI 入口：`docs/rocode-cli.md`
- TUI 入口：`docs/rocode-tui.md`
- 服务端入口：`docs/rocode-server.md`
- 插件与 Skill 示例：`docs/plugins_example/`

## 模块文档

- `docs/rocode-agent.md`：Agent 注册、执行与消息封装
- `docs/rocode-cli.md`：`rocode` 命令与子命令
- `docs/rocode-command.md`：Slash Command 注册与渲染
- `docs/rocode-config.md`：配置加载与合并
- `docs/rocode-core.md`：事件总线与 ID 基础设施
- `docs/rocode-grep.md`：代码与文本搜索封装
- `docs/rocode-lsp.md`：LSP 客户端与注册表
- `docs/rocode-mcp.md`：MCP 客户端、OAuth、传输层
- `docs/rocode-permission.md`：权限规则与决策引擎
- `docs/rocode-plugin.md`：Hook 系统与 TS 子进程桥接
- `docs/rocode-provider.md`：多 Provider 模型适配层
- `docs/rocode-server.md`：HTTP 路由、事件流、控制端点
- `docs/rocode-session.md`：会话生命周期与消息流
- `docs/rocode-storage.md`：SQLite 存储与仓储层
- `docs/rocode-tool.md`：内置工具与工具注册中心
- `docs/rocode-tui.md`：终端 UI 架构与交互
- `docs/rocode-types.md`：跨模块共享数据类型
- `docs/rocode-util.md`：文件系统、日志、通用工具
- `docs/rocode-watcher.md`：文件系统监听器

## 代码与文档约定

- 命令名当前为 `rocode`。
- 文档内容优先以源码和 `--help` 输出为准。
- 涉及行为差异或重构计划，统一写入 `docs/overview/`（若该目录存在）。
