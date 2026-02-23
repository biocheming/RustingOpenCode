# opencode-util

`opencode-util` 提供跨模块复用的工具函数与基础能力实现。

## 主要职责

- 文件系统便捷接口（`filesystem`）
- 日志初始化与结构化日志封装（`logging`）
- 通用工具集合（`util`）

## 模块结构

- `filesystem.rs`：文件读写、路径相关帮助方法
- `logging.rs`：tracing 初始化、日志级别与输出
- `util.rs`：token/timeout/git/lock/wildcard 等工具

## 使用建议

- 优先复用 util 能力，避免重复实现
- 与业务强耦合的 helper 不要放入此 crate
- 日志初始化建议由 CLI 统一调用

## 验证

```bash
cargo check -p opencode-util
```
