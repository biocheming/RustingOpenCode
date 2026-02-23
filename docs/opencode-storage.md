# opencode-storage

`opencode-storage` 提供 SQLite 持久化能力，封装数据库初始化与仓储访问。

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
cargo check -p opencode-storage
```
