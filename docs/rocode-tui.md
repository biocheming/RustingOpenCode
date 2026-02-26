# rocode-tui

文档基线：v2026.2.26（更新日期：2026-02-26）

`rocode-tui` 提供终端交互界面，包括首页、会话视图、输入框、侧栏、对话框、快捷键与主题系统。

## 品牌与显示

- `APP_NAME`: `RustingOpenCode`
- `APP_SHORT_NAME`: `ROCode`
- `APP_VERSION_DATE`: `v2026.2.26`
- `APP_TAGLINE`: `A Rusted OpenCode Version`

定义位置：`crates/rocode-tui/src/branding.rs`

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
- 侧栏支持分段折叠、滚动条、鼠标点击命中与进程面板（可选择/终止 Plugin/Bash 进程）
- 首页底栏增加 MCP 连接数和版本信息对齐展示（`ROCode v2026.2.26`）
- 新增 Provider 对话框数据流：优先读取 `/provider/known`，可直接提交 API Key 并刷新模型列表
- reasoning 渲染支持可折叠 Thinking 视图，避免长推理块占满会话窗口
- prompt 提交改为异步后台派发 + optimistic UI：本地消息先展示，网络请求完成后通过事件回填/回滚，减少“输入后长时间无响应”的体感延迟。
- Model Select 新增最近模型列表持久化（启动恢复、切换后保存），减少重复检索成本。
- 工具结果渲染支持 `title/metadata` 透传与 `display.*` hints；`batch/question` 有专门渲染分支，信息密度更高。
- Assistant 活跃态判断改为结合 `finish` 字段，回合结束后可更稳定停止“正在输出”状态。

## 开发建议

- UI 改动优先保证滚动稳定性与低 CPU 占用
- 鼠标事件改动需重点测试 hover + scroll 场景
- 文本渲染必须使用字符边界安全处理（避免 UTF-8 切片 panic）

## 验证

```bash
cargo check -p rocode-tui
```
