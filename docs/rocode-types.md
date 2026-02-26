# rocode-types

文档基线：v2026.2.26（更新日期：2026-02-26）

`rocode-types` 定义跨 crate 共享的数据结构，减少重复定义和序列化分歧。

## 当前分支变化（v2026.2.26）

- `SessionMessage` 新增 `finish: Option<String>`，用于跨 crate 传递统一的回合结束原因。
- 该字段对序列化为可选输出（`skip_serializing_if`），兼容旧数据与旧客户端。

## 主要职责

- 统一消息模型
- 统一会话模型
- 统一 todo 模型

## 模块结构

- `message.rs`：消息体、消息片段等类型
- `session.rs`：会话与元信息类型
- `todo.rs`：任务清单类型
- `lib.rs`：统一导出

## 设计原则

- 数据结构应尽量稳定
- 对外结构调整优先兼容旧字段
- 涉及 JSON 序列化时保持字段语义一致

## 典型使用场景

- `rocode-session` 在会话流转中直接使用
- `rocode-storage` 以这些类型为持久化边界
- `rocode-server` 在 API 层对外输出

## 验证

```bash
cargo check -p rocode-types
```
