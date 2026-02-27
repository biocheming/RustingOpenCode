# rocode-permission

文档基线：v2026.2.27（更新日期：2026-02-27）

`rocode-permission` 提供统一权限决策层，约束高风险工具操作。

## 本轮状态（v2026.2.27）

- 本轮未改动权限规则引擎，`allow/deny/ask` 语义保持一致。

## 主要职责

- 规则集定义与解析
- 操作 arity/粒度分类
- 允许/拒绝/询问（ask）决策
- 与插件 hook（`PermissionAsk`）协同

## 模块结构

- `ruleset.rs`：规则结构、匹配与解析
- `arity.rs`：操作粒度和参数类别
- `engine.rs`：权限引擎主逻辑

## 使用场景

- `rocode-tool` 执行前权限判定
- `rocode-session` 会话内审批流
- TUI/Server 的权限请求与回复

## 开发建议

- 新规则先覆盖最小权限默认策略
- 权限结果应可解释（命中规则、动作、目标）
- 避免把业务逻辑硬编码进规则引擎

## 验证

```bash
cargo check -p rocode-permission
```
