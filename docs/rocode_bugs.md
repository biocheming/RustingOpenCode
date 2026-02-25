# ROCode 消息传递漏洞分析报告

> 分析日期: 2026-02-25  
> 分析范围: 消息传递链路、事件系统、MCP 传输层、流式响应处理

---

## 一、漏洞总览

| # | 漏洞 | 文件 | 严重程度 | 状态 |
|---|------|------|----------|------|
| 1 | Bus 事件丢失（固定容量） | `crates/rocode-core/src/bus.rs:38` | 🔴 高 | ❌ 未修复 |
| 2 | Tool Call ID 不一致 | `crates/rocode-provider/src/stream.rs:235` | 🔴 高 | ✅ 已修复 |
| 3 | MCP 解析静默忽略 | `crates/rocode-mcp/src/transport.rs:232` | 🔴 高 | ❌ 未修复 |
| 4 | 流式响应提前结束 | `crates/rocode-provider/src/transform.rs` | 🟡 中 | ✅ 已修复 |
| 5 | 异步任务取消丢失 | `crates/rocode-session/src/session.rs:783` | 🟡 中 | ❌ 未修复 |
| 6 | Retry 竞态条件 | `crates/rocode-provider/src/retry.rs` | 🟡 中 | ❌ 未修复 |
| 7 | SSE 断线丢失 | `crates/rocode-mcp/src/transport.rs:337` | 🟡 中 | ❌ 未修复 |

**修复进度**: 2/7 (29%)

---

## 二、漏洞详情

### 1. Bus 事件丢失风险 🔴 高危

**文件**: `crates/rocode-core/src/bus.rs:38`

**问题代码**:
```rust
pub fn new() -> Self {
    let (tx, _) = broadcast::channel(1024);  // 固定容量 1024
    Self {
        next_id: Arc::new(RwLock::new(0)),
        subscribers: Arc::new(RwLock::new(HashMap::new())),
        wildcard_subscribers: Arc::new(RwLock::new(Vec::new())),
        tx,
    }
}
```

**问题分析**:
- `broadcast::channel` 容量固定为 1024
- 当消费者处理慢于生产者时，旧消息会被覆盖
- 没有背压机制，高速消息场景下会丢消息
- TUI 渲染可能丢失实时更新

**建议修复**:
- 动态调整容量或使用无界 channel
- 添加消息丢失告警
- 考虑使用 `mpsc` 替代 `broadcast` 对于关键消息

---

### 2. Tool Call ID 不一致 🔴 高危 ✅ 已修复

**文件**: `crates/rocode-provider/src/stream.rs:235`

**之前的问题代码**:
```rust
fn flush(self) -> Option<StreamEvent> {
    let input = serde_json::from_str(&self.arguments)
        .unwrap_or_else(|_| serde_json::json!({}));  // 丢失原始数据
    // ...
}
```

**修复后代码**:
```rust
fn flush(self) -> Option<StreamEvent> {
    let input = serde_json::from_str(&self.arguments)
        .unwrap_or_else(|_| serde_json::Value::String(self.arguments.clone()));  // 保留原始数据
    // ...
}
```

**修复评价**: ✅ 优秀
- 不再默默丢弃原始数据
- 保留为字符串便于调试
- 测试用例同步更新

---

### 3. MCP 传输层解析失败静默忽略 🔴 高危

**文件**: `crates/rocode-mcp/src/transport.rs:232-233`

**问题代码**:
```rust
// HttpTransport::send()
if let Ok(message) = JsonRpcMessage::from_str(data) {
    let _ = self.response_tx.send(message);  // 失败时静默丢弃！
}
```

**SSE 问题代码** (`transport.rs:328-330`):
```rust
match JsonRpcMessage::from_str(&data) {
    Ok(msg) => {
        if tx.send(msg).is_err() {
            break;  // 直接退出，不返回错误
        }
    }
    Err(e) => {
        tracing::warn!("SSE: failed to parse message: {}", e);  // 只打日志
    }
}
```

**问题分析**:
- JSON 解析失败时只 `warn!` 日志，不返回错误
- `response_tx.send()` 失败时用 `let _ =` 忽略
- 没有重试或确认机制
- MCP 工具响应可能静默丢失

**建议修复**:
```rust
// 方案1: 返回错误
let message = JsonRpcMessage::from_str(data)
    .map_err(|e| McpClientError::ProtocolError(format!("Parse error: {}", e)))?;
self.response_tx.send(message)
    .map_err(|e| McpClientError::TransportError("Channel closed".into()))?;

// 方案2: 记录并重试
if let Err(e) = self.response_tx.send(message) {
    tracing::error!("Failed to send message, retrying: {}", e);
    // 重试逻辑
}
```

---

### 4. 流式响应提前结束 🟡 中危 ✅ 已修复

**文件**: `crates/rocode-provider/src/stream.rs:454-459`

**问题代码**:
```rust
"content_block_stop" => {
    // content_block_stop only marks the end of a single content block
    // (text, tool_use, thinking, etc.), NOT the end of the entire message.
    return Some(StreamEvent::TextEnd);  // 可能误导调用方
}
```

**已修复**: 
- 添加了详细注释说明
- 在 `transform.rs` 中改进了 `normalize_messages_for_caching`
- 新增 `dedup_messages` 防止重复消息
- 新增 `ensure_noop_tool_if_needed` 解决 LiteLLM 兼容性

---

### 5. 异步任务取消后消息丢失 🟡 中危

**文件**: `crates/rocode-session/src/session.rs:779-788`

**问题代码**:
```rust
fn publish_event(&self, def: &'static BusEventDef, properties: serde_json::Value) {
    if let Some(ref bus) = self.bus {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let bus = bus.clone();
            handle.spawn(async move {  // fire-and-forget
                bus.publish(def, properties).await;
            });
        }
    }
}
```

**问题分析**:
- 使用 `spawn` 异步发布事件，没有等待完成
- 如果 runtime 关闭，任务可能未执行就被取消
- 会话结束时最后的事件可能丢失

**建议修复**:
```rust
// 方案1: 同步发布
pub async fn publish_event(&self, def: &'static BusEventDef, properties: serde_json::Value) {
    if let Some(ref bus) = self.bus {
        bus.publish(def, properties).await;
    }
}

// 方案2: 使用 tokio::spawn 但等待完成
fn publish_event(&self, def: &'static BusEventDef, properties: serde_json::Value) -> JoinHandle<()> {
    let bus = self.bus.clone();
    tokio::spawn(async move {
        if let Some(b) = bus {
            b.publish(def, properties).await;
        }
    })
}
```

---

### 6. Retry 机制的竞态条件 🟡 中危

**文件**: `crates/rocode-provider/src/retry.rs:145-178`

**问题代码**:
```rust
pub async fn with_retry<F, Fut, T, E>(config: &RetryConfig, mut f: F) -> Result<T, E>
{
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt >= config.max_attempts { return Err(e); }
                // 延迟期间状态可能改变
                tokio::time::sleep(...).await;
            }
        }
    }
}
```

**问题分析**:
- 重试期间会话状态可能已被其他任务修改
- 没有乐观锁或版本检查
- 重试成功后可能覆盖其他变更

**建议修复**:
- 添加版本号/时间戳检查
- 使用 CAS (Compare-And-Swap) 模式
- 在重试前验证状态一致性

---

### 7. SSE 连接断开后缓冲区丢失 🟡 中危

**文件**: `crates/rocode-mcp/src/transport.rs:336-339`

**问题代码**:
```rust
let handle = tokio::spawn(async move {
    while let Some(event) = es.next().await {
        match event {
            // ...
            Err(e) => {
                tracing::error!("SSE error: {}", e);
                break;  // 直接退出，缓冲区消息丢失
            }
        }
    }
});
```

**问题分析**:
- SSE 错误时直接 break，未处理的消息丢失
- 没有自动重连机制
- 网络抖动时消息丢失

**建议修复**:
```rust
// 添加重连逻辑
async fn connect_with_retry(&self, max_retries: u32) -> Result<(), McpClientError> {
    let mut retries = 0;
    loop {
        match self.connect().await {
            Ok(_) => return Ok(()),
            Err(e) if retries < max_retries => {
                retries += 1;
                let delay = 2u64.pow(retries) * 1000; // 指数退避
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            Err(e) => return Err(e),
        }
    }
}
```

---

## 三、已完成的修复

### 3.1 Tool Call 不完整参数处理 ✅

**文件**: `crates/rocode-provider/src/stream.rs:235-236`

**修复内容**: 不完整 JSON 保留为字符串而非空对象

---

### 3.2 消息去重 ✅

**文件**: `crates/rocode-provider/src/transform.rs`

**新增功能**:
```rust
pub fn dedup_messages(messages: &mut Vec<Message>) {
    messages.dedup_by(|b, a| {
        if std::mem::discriminant(&a.role) != std::mem::discriminant(&b.role) {
            return false;
        }
        match (&a.content, &b.content) {
            (Content::Text(t1), Content::Text(t2)) => t1 == t2,
            _ => false,
        }
    });
}
```

---

### 3.3 LiteLLM 兼容性 ✅

**文件**: `crates/rocode-provider/src/transform.rs`

**新增功能**:
```rust
pub fn ensure_noop_tool_if_needed(
    tools: &mut Option<Vec<crate::ToolDefinition>>,
    messages: &[Message],
) {
    // 当消息历史包含 tool_use/tool_result 但当前请求无工具时
    // 注入 _noop 占位工具以兼容 LiteLLM 等代理
}
```

---

### 3.4 Invalid Tool 路由 ✅

**文件**: `crates/rocode-agent/src/executor.rs`

**新增功能**: 自动将 `InvalidArguments` 错误路由到 `invalid` 工具

---

### 3.5 Multi-step Agent Loop ✅

**文件**: `crates/rocode-server/src/routes.rs`

**新增功能**: 支持最多 100 步的工具调用循环

---

### 3.6 Tool 状态追踪 ✅

**文件**: `crates/rocode-session/src/message.rs`

**新增功能**:
```rust
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed,
    Error,
}
```

---

## 四、修复优先级建议

### P0 - 立即修复
1. **Bus 事件丢失** - 影响所有 UI 更新
2. **MCP 静默忽略** - 影响工具调用可靠性

### P1 - 尽快修复
3. **SSE 断线重连** - 影响长连接稳定性
4. **异步任务取消** - 影响会话结束时的状态

### P2 - 计划修复
5. **Retry 竞态条件** - 边界情况，影响有限

---

## 五、测试建议

### 5.1 Bus 压力测试
```rust
#[tokio::test]
async fn bus_high_load_test() {
    let bus = Arc::new(Bus::new());
    let mut rx = bus.subscribe_channel();
    
    // 发送超过容量的消息
    for i in 0..2000 {
        bus.publish(&TEST_EVENT, serde_json::json!({"count": i})).await;
    }
    
    // 验证消息丢失情况
}
```

### 5.2 MCP 解析错误测试
```rust
#[tokio::test]
async fn mcp_invalid_json_handling() {
    let transport = HttpTransport::new(...);
    // 发送无效 JSON，验证错误处理
}
```

### 5.3 SSE 重连测试
```rust
#[tokio::test]
async fn sse_reconnect_test() {
    // 模拟网络中断，验证重连
}
```

---

## 六、参考文献

- [Tokio Broadcast Channel 文档](https://docs.rs/tokio/latest/tokio/sync/broadcast/index.html)
- [SSE 规范](https://html.spec.whatwg.org/multipage/server-sent-events.html)
- [MCP 协议规范](https://modelcontextprotocol.io/)
