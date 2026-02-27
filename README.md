# RustingOpenCode (ROCode)

**A Rusted OpenCode Version**

RustingOpenCode（简称 ROCode）是 OpenCode 的 Rust 实现与演进版本，提供完整的 CLI/TUI/Server 工作流，用于本地 AI 编码代理、会话管理、工具调用、MCP/LSP 集成和插件扩展。

## 当前状态

- 品牌名：`RustingOpenCode` / `ROCode`
- 版本标识：`v2026.2.27`
- 可执行命令：`rocode`

## 本轮更新（2026-02-27）

- 插件系统完成一轮稳定性与性能改造：Hook 顺序执行、`[plugin-perf]` 度量、超时自愈、熔断保护与大 payload 文件通道。
- Provider/Session/Tool 的工具参数链路增强：鲁棒 JSON 解析、JSON-ish 恢复、不可恢复参数哨兵对象，降低历史坏条目反复污染。
- `write` 调用新增执行前预校验：缺少 `file_path` / `content` 时直接路由 `invalid`，避免工具执行层反复报错。
- `question` 交互链路对齐：工具优先走 `ctx.question(...)` 回调并兼容字符串化 `questions`；TUI 支持 `Up/Down`、`Tab/Shift+Tab`、`Space`、`Enter` 全键盘操作。

## 功能概览

- 交互模式：TUI（默认）、CLI 单次运行、HTTP 服务、Web/ACP 模式
- 会话能力：创建/继续/分叉会话，导入导出会话
- 工具系统：内置读写编辑、Shell、补丁等工具链
- 模型体系：多 Provider 适配、Agent 模式切换
- 扩展能力：插件桥接（含 TS 插件）、MCP、LSP
- 终端体验：增强排版、可折叠侧栏、代码高亮、路径补全

## 快速开始

### 1. 环境要求

- Rust stable
- Cargo
- Git（建议）

### 2. 构建

```bash
cargo build -p rocode-cli
```

### 3. 查看帮助

```bash
./target/debug/rocode --help
```

或

```bash
cargo run -p rocode-cli -- --help
```

### 4. 启动方式

- 默认进入 TUI：

```bash
cargo run -p rocode-cli --
```

- 显式进入 TUI：

```bash
cargo run -p rocode-cli -- tui
```

- 非交互运行：

```bash
cargo run -p rocode-cli -- run "请检查这个仓库中的风险点"
```

- 启动 HTTP 服务：

```bash
cargo run -p rocode-cli -- serve --port 3000 --hostname 127.0.0.1
```

## CLI 命令总览

以下命令来自当前 `rocode --help`：

- `tui`：启动交互式终端界面
- `attach`：附加到已运行的服务
- `run`：单次消息运行
- `serve`：启动 HTTP 服务
- `web`：启动 headless 服务并打开 Web 界面
- `acp`：启动 ACP 服务
- `models`：查看可用模型
- `session`：会话管理
- `stats`：token/cost 统计
- `db`：数据库工具
- `config`：查看配置
- `auth`：凭据管理
- `agent`：Agent 管理
- `debug`：调试与排障
- `mcp`：MCP 管理
- `export` / `import`：会话导出导入
- `github` / `pr`：GitHub 相关能力
- `upgrade` / `uninstall`：升级与卸载
- `generate`：生成 OpenAPI 规范
- `version`：查看版本

常用帮助：

```bash
rocode tui --help
rocode run --help
rocode serve --help
rocode session --help
```

## 配置

项目配置会在以下路径中按优先级合并（向上查找）：

- `opencode.jsonc`
- `opencode.json`
- `.opencode/opencode.jsonc`
- `.opencode/opencode.json`

全局配置默认路径：

- Linux/macOS：`~/.config/opencode/opencode.jsonc`（或 `.json`）

参考：`docs/rocode-config.md`

## 仓库结构

- `crates/rocode-cli`：CLI 入口（binary: `rocode`）
- `crates/rocode-server`：HTTP/SSE/WebSocket 服务
- `crates/rocode-tui`：终端 UI
- `crates/rocode-session`：会话与消息
- `crates/rocode-tool`：工具注册与执行
- `crates/rocode-provider`：模型 Provider 适配
- `crates/rocode-plugin`：插件系统与子进程桥接
- `crates/rocode-mcp`：MCP 客户端与注册
- `crates/rocode-lsp`：LSP 支持
- `crates/rocode-storage`：SQLite 存储

## 开发与验证

```bash
cargo fmt
cargo check
cargo clippy --workspace --all-targets
```

最小验证（常用）：

```bash
cargo check -p rocode-cli
cargo check -p rocode-tui
```

## 文档导航

- 用户指南：`USER_GUIDE.md`
- 文档索引：`docs/README.md`
- CLI：`docs/rocode-cli.md`
- TUI：`docs/rocode-tui.md`
- Server：`docs/rocode-server.md`
- Tool：`docs/rocode-tool.md`
- Provider：`docs/rocode-provider.md`
- Config：`docs/rocode-config.md`

## 说明

- 当前默认命令名为 `rocode`。
