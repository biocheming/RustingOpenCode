# rocode-storage

文档基线：v2026.2.27（更新日期：2026-02-27）

`rocode-storage` 提供 SQLite 持久化能力，封装数据库初始化与仓储访问。

## 当前分支变化（v2026.2.27）

- 数据库初始化新增 `PRAGMA journal_mode=WAL` 与 `PRAGMA synchronous=NORMAL`（失败仅告警），提升并发读写吞吐。
- `SessionRepository` 新增 `upsert()` 与事务化 `flush_with_messages()`：单事务完成 session upsert、message upsert、stale message 对账删除。
- `sync_sessions_to_storage()` 路径已统一复用事务 flush 逻辑，避免“删全量再逐条重建”的高收尾开销。
- `messages` 表新增 `finish` 列，并通过迁移脚本兼容旧库；MessageRepository 已完成读写全链路支持。
- 新增对历史 malformed tool_call 入参的读取侧兼容：优先鲁棒解析并尝试 JSON-ish 恢复，降低旧会话回放失败率。

## 主要职责

- 建立数据库连接与迁移
- 提供会话、消息、todo 仓储实现
- 统一存储层错误模型

## 模块结构

- `database.rs`：数据库初始化、连接管理
- `schema.rs`：表结构与迁移定义
- `repository.rs`：Session/Message/Todo 仓储

## 关键导出

- `Database`
- `DatabaseError`
- `SessionRepository`
- `MessageRepository`
- `TodoRepository`

## 开发建议

- Schema 变更要保证迁移前后兼容
- 仓储接口应保持事务边界清晰
- 读写热点场景需要关注索引与查询成本

## 验证

```bash
cargo check -p rocode-storage
```
