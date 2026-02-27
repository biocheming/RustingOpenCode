# rocode-util

文档基线：v2026.2.27（更新日期：2026-02-27）

`rocode-util` 提供跨模块复用的工具函数与基础能力实现。

## 本轮状态（v2026.2.27）

- 新增并统一复用鲁棒 JSON 参数处理能力：`try_parse_json_object_robust`、`recover_tool_arguments_from_jsonish`。
- 该能力已被 provider/tool/session/storage 多模块复用，用于处理模型流式工具参数中的截断、转义与 JSON-ish 形态。

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
cargo check -p rocode-util
```
