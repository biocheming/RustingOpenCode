# opencode-tool

`opencode-tool` 提供工具调用体系，包括工具定义、注册中心、执行上下文和内置工具实现。

## 主要职责

- 定义统一 `Tool` 接口
- 维护工具注册表（`registry`）
- 提供内置工具（读写编辑、shell、搜索、patch、todo 等）
- 与权限系统、插件 hooks、LSP/MCP 协作

## 内置模块（部分）

- 文件类：`read`、`write`、`edit`、`multiedit`、`ls`
- 搜索类：`grep_tool`、`glob_tool`、`codesearch`
- 执行类：`bash`、`batch`、`apply_patch`
- 任务类：`plan`、`task`、`todo`、`question`
- 网络类：`webfetch`、`websearch`
- 支持类：`registry`、`tool`、`truncation`

## 特性开关

- `lsp` feature：启用 `opencode-lsp` 与 `lsp-types` 集成

## 开发建议

- 新增工具优先实现幂等和可观测日志
- 所有副作用工具都应配合权限系统检查
- 工具输出需考虑 TUI 展示截断策略

## 验证

```bash
cargo check -p opencode-tool
cargo check -p opencode-tool --features lsp
```
