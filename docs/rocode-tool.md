# rocode-tool

文档基线：v2026.2.26（更新日期：2026-02-26）

`rocode-tool` 提供工具调用体系，包括工具定义、注册中心、执行上下文和内置工具实现。

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

- `lsp` feature：启用 `rocode-lsp` 与 `lsp-types` 集成

## 当前分支变化（v2026.2.26）

- `read` 工具描述改为外置 `read.txt`，并增强入参兼容（`file_path/filePath/filepath/path`）；对目录、图片、PDF 提供结构化读取输出（含 data URL）。
- `registry` 增加工具参数规范化与错误重写：字符串参数会尝试解析为 JSON/`key=value`，`InvalidArguments` 统一附带“请重写为合法 schema”提示。
- `invalid` 工具兼容 TS 与旧版字段命名（如 `toolName/errorMessage`），用于承接上游工具参数错误。
- `bash` 工具引入 `shell.env` hook 环境注入、AST 级权限提取、进程注册（`process_registry`）与超时/取消终止树处理。
- `write` 工具保留 `diff` 元数据并发布 `file.edited`、`file_watcher.updated` 事件，便于 TUI/LSP 同步。
- `read` 在二进制读取场景不再把 base64 直接塞进文本输出，改为通过 `metadata.attachments`/`attachment` 透传，降低 provider 请求体体积。
- `batch` 会聚合子工具附件并剔除重复的大附件 metadata，默认输出从“全量 JSON 回显”改为摘要文案，降低上下文噪音与 body 大小。
- `question` 工具新增 `display.summary` 与 `display.fields` 元数据，供 TUI 以结构化问答方式渲染。

## 开发建议

- 新增工具优先实现幂等和可观测日志
- 所有副作用工具都应配合权限系统检查
- 工具输出需考虑 TUI 展示截断策略

## 验证

```bash
cargo check -p rocode-tool
cargo check -p rocode-tool --features lsp
```
