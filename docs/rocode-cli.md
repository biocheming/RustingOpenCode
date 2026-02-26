# rocode-cli

文档基线：v2026.2.26（更新日期：2026-02-26）

`rocode-cli` 提供工作区统一可执行入口（二进制名：`rocode`）。

## 命令定位

- 启动 TUI
- 启动/附加服务
- 执行单次任务（run）
- 调用会话、模型、MCP、调试等子命令

## 当前分支变化（v2026.2.26）

- CLI 入口已按子命令拆分到独立模块（`agent_cmd.rs`、`auth.rs`、`db.rs`、`mcp_cmd.rs`、`run.rs`、`server.rs`、`session_cmd.rs`、`tui.rs` 等），`main.rs` 只负责参数分发。
- 启动时会初始化文件日志，默认写入 `~/.local/share/rocode/log/rocode.log`（若无法获取用户目录则回退到 `/tmp/rocode/log/rocode.log`）。
- `tui`/`attach` 路径统一通过环境变量桥接（如 `OPENCODE_TUI_BASE_URL`、`OPENCODE_TUI_SESSION`、`OPENCODE_TUI_MODEL`），便于 TUI 与本地 server 解耦。
- `version` 子命令输出来自 `CARGO_PKG_VERSION`，用于与工作区版本号保持一致。
- 本轮命令集未新增子命令，重点是跟随会话/服务端能力升级并统一版本为 `2026.2.26`。

## 当前顶层子命令

以 `rocode --help`（2026-02-26）为准：

- `tui`
- `attach`
- `run`
- `serve`
- `web`
- `acp`
- `models`
- `session`
- `stats`
- `db`
- `config`
- `auth`
- `agent`
- `debug`
- `mcp`
- `export`
- `import`
- `github`
- `pr`
- `upgrade`
- `uninstall`
- `generate`
- `version`

## 关键参数（高频）

### `rocode tui`

- `-m, --model <MODEL>`
- `-c, --continue`
- `-s, --session <SESSION>`
- `--fork`
- `--agent <AGENT>`（默认 `build`）
- `--port <PORT>`、`--hostname <HOSTNAME>`

### `rocode run`

- `MESSAGE...`
- `--command <COMMAND>`
- `-f, --file <FILE>`
- `--format <default|json>`
- `--thinking`
- `--agent <AGENT>` / `--model <MODEL>`

## 源码入口

- `crates/rocode-cli/src/main.rs`
- `crates/rocode-cli/src/cli.rs`
- `crates/rocode-cli/src/tui.rs`

## 开发建议

- 子命令行为变更后，先更新 `--help` 再更新文档
- 尽量保持 CLI 参数与服务端/配置字段一致命名

## 验证

```bash
cargo check -p rocode-cli
./target/debug/rocode --help
```
