# USER GUIDE - RustingOpenCode (ROCode)

本手册面向日常使用者，覆盖启动、常用命令、配置与故障排查。  
品牌名为 `RustingOpenCode`（简称 `ROCode`），当前 CLI 命令仍为 `opencode`。

## 1. 快速启动

如果你从源码运行：

```bash
cd opencode-rust-rewrite
cargo run -p opencode-cli -- --help
```

默认进入 TUI：

```bash
cargo run -p opencode-cli --
```

等价于：

```bash
cargo run -p opencode-cli -- tui
```

单次非交互运行：

```bash
cargo run -p opencode-cli -- run "请总结这个仓库当前风险"
```

启动 HTTP 服务：

```bash
cargo run -p opencode-cli -- serve --port 3000 --hostname 127.0.0.1
```

## 2. 常用命令

### 2.1 会话管理

```bash
opencode session list
opencode session list --format json
opencode session show <SESSION_ID>
opencode session delete <SESSION_ID>
```

### 2.2 模型与配置

```bash
opencode models
opencode models --refresh --verbose
opencode config
```

### 2.3 认证管理

```bash
opencode auth list
opencode auth login --help
opencode auth logout --help
```

说明：`auth login/logout` 的具体参数请以 `--help` 输出为准。

### 2.4 MCP 管理

```bash
opencode mcp list
opencode mcp connect <NAME>
opencode mcp disconnect <NAME>
opencode mcp add --help
opencode mcp auth --help
```

如果本地服务不在默认地址，可加：

```bash
opencode mcp --server http://127.0.0.1:3000 list
```

### 2.5 调试命令

```bash
opencode debug paths
opencode debug config
opencode debug skill
opencode debug agent
```

## 3. TUI 与 Run 常用参数

查看完整参数：

```bash
opencode tui --help
opencode run --help
```

高频参数（两者都常用）：

- `-m, --model <MODEL>`：指定模型
- `-c, --continue`：继续最近会话
- `-s, --session <SESSION>`：继续指定会话
- `--fork`：分叉会话
- `--agent <AGENT>`：指定 agent（默认 `build`）
- `--port <PORT>` / `--hostname <HOSTNAME>`：服务地址参数

`run` 额外常用：

- `--format default|json`
- `-f, --file <FILE>`
- `--thinking`

## 4. 配置文件位置

程序会按优先级合并多份配置（向上查找）：

- `opencode.jsonc`
- `opencode.json`
- `.opencode/opencode.jsonc`
- `.opencode/opencode.json`

全局配置默认位置：

- `~/.config/opencode/opencode.jsonc`（或 `.json`）

建议：先使用项目级最小配置，再逐步增加 provider/mcp/agent/lsp。

## 5. 推荐工作流

### 5.1 本地交互开发

1. `cargo run -p opencode-cli --`
2. 在 TUI 中执行任务
3. 用 `opencode session list/show` 回看历史

### 5.2 脚本或集成场景

1. `opencode serve --port 3000`
2. 用 `opencode run ... --format json` 或服务 API 集成
3. 用 `opencode stats` 追踪 token/cost

## 6. 故障排查

### 6.1 端口冲突

- 换端口：`opencode serve --port 3001`

### 6.2 模型不可用

1. `opencode auth list`
2. `opencode models --refresh`
3. `opencode config` 检查 provider 配置是否生效

### 6.3 配置疑难

1. `opencode debug paths` 查看配置搜索路径
2. `opencode debug config` 查看最终合并结果

### 6.4 MCP 连接异常

1. `opencode mcp list`
2. `opencode mcp debug <NAME>`
3. `opencode mcp connect <NAME>`

## 7. 文档索引

- 项目总览：`README.md`
- 文档总索引：`docs/README.md`
- CLI 文档：`docs/opencode-cli.md`
- TUI 文档：`docs/opencode-tui.md`
- 配置文档：`docs/opencode-config.md`
