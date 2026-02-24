# rocode-config

`rocode-config` 负责配置文件发现、加载、解析与合并，是运行时行为的配置入口。

## 主要职责

- 搜索配置文件（项目级与全局）
- 解析 JSON/JSONC（兼容注释）
- 将配置映射为强类型结构
- 提供 well-known 路径与默认值

## 模块结构

- `loader.rs`：配置加载、路径查找、合并流程
- `schema.rs`：配置结构定义
- `wellknown.rs`：常见目录/文件路径常量

## 配置路径（常见）

- 项目：`opencode.jsonc` / `opencode.json`
- 项目扩展：`.opencode/opencode.jsonc` / `.opencode/opencode.json`
- 全局：`~/.config/opencode/opencode.jsonc`（或 `.json`）

## 使用建议

- 配置新增字段时同时补默认值策略
- 合并行为需保持“可预期”
- 涉及 provider/mcp/agent 的字段变更需联动文档

## 验证

```bash
cargo check -p rocode-config
```
