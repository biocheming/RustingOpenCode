# opencode-tui

`opencode-tui` 提供终端交互界面，包括首页、会话视图、输入框、侧栏、对话框、快捷键与主题系统。

## 品牌与显示

- `APP_NAME`: `RustingOpenCode`
- `APP_SHORT_NAME`: `ROCode`
- `APP_VERSION_DATE`: `2026.02.23`
- `APP_TAGLINE`: `A Rusted OpenCode Version`

定义位置：`crates/opencode-tui/src/branding.rs`

## 主要职责

- 渲染会话消息与工具结果
- 管理输入、补全、命令面板和对话框
- 连接服务端 API 与本地事件循环
- 提供主题、布局和交互状态管理

## 关键模块

- `app/`：主事件循环与状态同步
- `components/`：home/session/prompt/sidebar/dialog
- `context/`：应用状态、键位、缓存
- `api.rs`：与本地 server 的通信客户端
- `file_index.rs`：`@path` 补全索引（nucleo matcher）
- `components/markdown/`：代码块渲染与 syntect 高亮

## 已落地增强（当前分支）

- 浮层侧栏与显式开关（含 `☰` 按钮）
- Braille/KnightRider 可切换 spinner
- 更细致的消息块布局与状态行
- syntect 代码高亮与路径感知补全

## 开发建议

- UI 改动优先保证滚动稳定性与低 CPU 占用
- 鼠标事件改动需重点测试 hover + scroll 场景
- 文本渲染必须使用字符边界安全处理（避免 UTF-8 切片 panic）

## 验证

```bash
cargo check -p opencode-tui
```
