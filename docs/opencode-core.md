# opencode-core

`opencode-core` 是工作区最底层的通用基础库，提供全局事件总线与 ID 生成能力。

## 主要职责

- 提供异步事件总线（`bus`）
- 提供统一 ID 工具（`id`）
- 作为上层 crate 的轻依赖基础

## 模块结构

- `bus.rs`：事件发布/订阅基础设施
- `id.rs`：ID 生成、解析、格式化
- `lib.rs`：统一导出

## 依赖关系

- 上游依赖：无业务依赖
- 下游使用：几乎所有业务 crate（session、tool、provider、server 等）

## 开发建议

- 避免把业务逻辑放入 core
- 新增能力时优先保证无副作用、低耦合
- 任何变更都应考虑全 workspace 影响面

## 验证

```bash
cargo check -p opencode-core
```
