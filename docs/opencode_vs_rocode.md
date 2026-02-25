  opencode vs rocode 核心差异分析报告

  一、致命级问题 (CRITICAL)

  1. ✅ Agent 循环不传递 Tools 给 LLM
  已修复：executor.rs 的 execute/execute_subsession/execute_streaming 三个方法现在通过 resolve_tool_definitions() + .with_tools() 传递工具定义。

  2. ✅ 空消息处理错误
  已修复：normalize_messages_for_caching 改为 retain() 过滤空 assistant 消息，不再用空格替换。

  3. ✅ Interleaved Thinking 内容丢失
  已修复：apply_interleaved_thinking 不再删除 reasoning parts；Anthropic provider 新增 Thinking variant 将 reasoning 转为 thinking 内容块。

  二、严重级问题 (HIGH)

  4. ✅ MCP 工具未转换为可执行格式
  已修复：新建 rocode-session/src/mcp_bridge.rs，McpBridgeTool 实现 Tool trait 委托 McpClient::call_tool()，register_mcp_tools() 注册到主 ToolRegistry。

  5. ✅ max_tokens 硬编码
  已修复：anthropic.rs 的 fallback 从 4096 改为 model.max_output_tokens.min(32000)。

  6. ✅ 工具修复机制不完整
  已修复：executor 的三个循环现在在 InvalidArguments 时重新路由到 invalid 工具，与 session prompt 行为一致。

  7. ✅ 消息去重缺失
  已修复：新增 dedup_messages() 函数，去除连续重复消息。

  三、中等级问题 (MEDIUM)

  8. 架构差异 — 巨型单文件 (未修复 — 结构性重构)

  opencode CLI: 48+ 个 TypeScript 文件，职责清晰分离。
  rocode CLI: main.rs 一个文件 5543 行。

  9. 插件系统通信开销 (未修复 — 架构性差异)

  opencode: 插件作为 ES 模块直接 import，in-process 调用。
  rocode: 插件通过子进程 + JSON-RPC 2.0 通信。

  10. ✅ LiteLLM 代理兼容性缺失
  已修复：新增 ensure_noop_tool_if_needed()，当消息含 tool_use 但无 tools 时注入 _noop 占位工具。

  11. ✅ MCP 通知处理 (原报告有误 — 已实现)
  rocode 已有 handle_notification 处理 notifications/tools/list_changed，refresh_tools_if_needed 自动重载。

  12. Compaction 实现差异 (未修复 — 需要 LLM 驱动摘要)

  opencode: LLM 驱动的摘要生成。
  rocode: 简单文本截取。

  四、低级问题 (LOW)

  13. 系统提示组装差异 (未修复)

  opencode: 插件可修改系统提示，2-part 结构缓存优化。
  rocode: ChatSystemTransform hook 只传元数据。

  14. ✅ Provider Options 深度合并
  已修复：merge_deep_into 改为真正递归合并嵌套对象。

  ---
  修复统计：14 个问题中 10 个已修复，1 个原报告有误（已实现），剩余 3 个为架构性/结构性问题。
