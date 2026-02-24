# rocode-cli

`rocode-cli` 提供工作区统一可执行入口（二进制名：`rocode`）。

## 命令定位

- 启动 TUI
- 启动/附加服务
- 执行单次任务（run）
- 调用会话、模型、MCP、调试等子命令

## 当前顶层子命令

以 `rocode --help`（2026-02-23）为准：

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

## 开发建议

- 子命令行为变更后，先更新 `--help` 再更新文档
- 尽量保持 CLI 参数与服务端/配置字段一致命名

## 验证

```bash
cargo check -p rocode-cli
./target/debug/rocode --help
```
