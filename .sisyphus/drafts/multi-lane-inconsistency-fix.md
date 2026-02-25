# 多链路不一致问题诊断与修复方案

## 问题诊断（用户分析验证）

### ✅ 已修复的功能（报告过期内容）

| 功能 | 位置 | 状态 |
|------|------|------|
| Invalid 结构化载荷 | `prompt.rs:1822` | ✅ 已实现 |
| 缺字段预检 | `prompt.rs:1834` | ✅ 已实现 |
| 空工具名过滤 | `prompt.rs:1163`, `executor.rs:327` | ✅ 已实现 |

---

### ❌ 真正的根本原因：多链路不一致

Rust 版本有 **3 条独立的执行链路**，行为尚未统一：

```
┌─────────────────────────────────────────────────────────────────┐
│                    3 条执行链路                                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. session/prompt 链路 (SessionPrompt::loop_inner)             │
│     - 位置：crates/rocode-session/src/prompt.rs:1100+          │
│     - 特点：✅ 预检 ✅ 回退 ✅ 结构化错误                        │
│     - 状态：最完善                                              │
│                                                                 │
│  2. agent/executor 链路 (AgentExecutor::execute)                │
│     - 位置：crates/rocode-agent/src/executor.rs:240+           │
│     - 特点：❌ 无预检 ❌ 无回退 ❌ 直接执行                      │
│     - 状态：最落后                                              │
│                                                                 │
│  3. llm 链路 (LlmProcessor)                                     │
│     - 位置：crates/rocode-session/src/llm.rs:1328              │
│     - 特点：⚠️ 只修工具名 ❌ 无结构化载荷                        │
│     - 状态：中等                                                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## 链路详细对比

### 1. session/prompt 链路 ✅（最完善）

**位置**: `crates/rocode-session/src/prompt.rs:1970-2150`

**完整流程**:
```rust
// 1. 预检 - missing_required_fields (line 1834)
let missing = Self::missing_required_fields(&tool.parameters(), &effective_input);
if !missing.is_empty() {
    preflight_error = Some(...);
}

// 2. 执行
let execution = if let Some(err) = preflight_error {
    Err(ToolError::InvalidArguments(err))
} else {
    tool_registry.execute(...).await
};

// 3. 回退到 invalid 工具 (line 2102-2126)
if matches!(&execution, Err(ToolError::InvalidArguments(_))) {
    effective_tool_name = "invalid".to_string();
    effective_input = Self::invalid_tool_payload(&tool_name, &validation_error, &effective_input);
    execution = tool_registry.execute(&effective_tool_name, ...).await;
}
```

**特点**:
- ✅ 预检缺失字段
- ✅ 结构化错误载荷 (`invalid_tool_payload`)
- ✅ 自动回退到 invalid 工具

---

### 2. agent/executor 链路 ❌（最落后）

**位置**: `crates/rocode-agent/src/executor.rs:240-260`

**当前流程**:
```rust
// 1. 直接 repair（只修工具名）
let effective_tool_call = self.repair_tool_call(tool_call).await;

// 2. 直接执行，无预检
let result = self.execute_tool_without_subsessions(&effective_tool_call).await;

// 3. 直接返回错误，无回退
let (content, is_error) = match result {
    Ok(output) => (output, false),
    Err(e) => (e.to_string(), true),  // ← 简单字符串化
};

// 4. 添加到对话
self.conversation.add_tool_result(..., content, is_error);
```

**缺失功能**:
- ❌ 无预检（missing_required_fields）
- ❌ 无结构化错误载荷
- ❌ 无 invalid 工具回退
- ❌ 简单字符串化错误

**repair_tool_call 实现** (line 514-537):
```rust
async fn repair_tool_call(&self, tool_call: ToolCall) -> ToolCall {
    let available_tools = self.tools.list_ids().await;
    let repaired_name = repair_tool_call_name(&tool_call.name, &available_tools)
        .unwrap_or_else(|| tool_call.name.clone());
    
    // ⚠️ 只修复工具名，不构造结构化载荷
    let arguments = if repaired_name == "invalid" && tool_call.name != "invalid" {
        serde_json::json!({
            "tool": tool_call.name.clone(),
            "error": format!("Unknown tool requested by model: {}", tool_call.name),
        })
    } else {
        tool_call.arguments  // ← 保持原样，即使是空对象 {}
    };
    
    ToolCall { id, name: repaired_name, arguments }
}
```

---

### 3. llm 链路 ⚠️（中等）

**位置**: `crates/rocode-session/src/llm.rs:1328-1348`

**当前实现**:
```rust
pub fn repair_tool_call(name: &str, tools: &HashMap<String, ToolDefinition>) -> String {
    // Exact match
    if tools.contains_key(name) {
        return name.to_string();
    }
    
    // Try lowercase
    let lower = name.to_lowercase();
    if lower != name && tools.contains_key(&lower) {
        return lower;
    }
    
    // No match - return "invalid"
    "invalid".to_string()  // ← 只返回名称，没有载荷
}
```

**缺失功能**:
- ❌ 只修复工具名
- ❌ 不构造结构化载荷
- ❌ 不处理参数验证

---

## 其他问题

### 4. Provider 解析层问题 ⚠️

**位置**: `crates/rocode-provider/src/stream.rs:350`

**当前逻辑**:
```rust
let has_name = func.name.as_deref().is_some_and(|n| !n.is_empty());
let has_args = func.arguments.as_deref().is_some_and(|a| !a.is_empty());

if has_name {
    events.push(StreamEvent::ToolCallStart { ... });
}
if has_args {
    events.push(StreamEvent::ToolCallDelta { ... });
}
```

**问题**:
- ⚠️ 只检查非空，不检查有效性
- ⚠️ `⚙` 等符号名会通过
- ⚠️ 不完整 JSON 会传递

---

### 5. 系统提示缺失 ❌

**位置**: `crates/rocode-session/src/prompt_templates/anthropic.txt:83`

**当前内容**:
> "Do not guess or make up arguments for tools that require specific inputs."

**问题**:
- ❌ 只强调"不要猜参数"
- ❌ 没有"参数错误时必须重试"
- ❌ 没有"如何解读 invalid 工具输出"

---

## 修复方案（统一 3 条链路）

### P0 - 统一 agent/executor 链路（最关键）

**目标**: 让 executor 链路拥有和 prompt 链路同等的预检/回退能力

**修改文件**: `crates/rocode-agent/src/executor.rs`

**步骤**:

#### 1. 添加预检函数（复用 prompt.rs）

```rust
// 在 executor.rs 中添加（或从 prompt.rs 提取为公共函数）
fn missing_required_fields(schema: &Value, args: &Value) -> Vec<String> {
    let required: Vec<String> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    
    if required.is_empty() {
        return Vec::new();
    }
    
    let obj = match args.as_object() {
        Some(map) => map,
        None => return required,
    };
    
    required.into_iter().filter(|field| !obj.contains_key(field)).collect()
}
```

#### 2. 修改 execute_tool 添加预检和回退

```rust
async fn execute_tool(&self, tool_call: &ToolCall) -> Result<String, ToolError> {
    // ... 现有代码 ...
    
    // 1. 预检
    let mut preflight_error: Option<String> = None;
    if let Some(tool) = self.tools.get(&tool_call.name).await {
        let missing = Self::missing_required_fields(&tool.parameters(), &tool_call.arguments);
        if !missing.is_empty() {
            preflight_error = Some(format!(
                "Invalid arguments: missing required field(s): {}",
                missing.join(", ")
            ));
        }
    }
    
    // 2. 执行或返回预检错误
    let execution = if let Some(err) = preflight_error {
        Err(ToolError::InvalidArguments(err))
    } else {
        self.tools.execute(&tool_call.name, tool_call.arguments.clone(), ctx).await
    };
    
    // 3. 回退到 invalid 工具
    if let Err(ToolError::InvalidArguments(validation_error)) = &execution {
        let available_tools = self.tools.list_ids().await;
        if available_tools.contains(&"invalid".to_string()) {
            let invalid_input = serde_json::json!({
                "tool": tool_call.name.clone(),
                "error": validation_error.to_string(),
                "receivedArgs": tool_call.arguments.clone()
            });
            
            return self.tools.execute("invalid", invalid_input, ctx).await
                .map(|r| r.output);
        }
    }
    
    execution.map(|r| r.output)
}
```

---

### P1 - 增强 llm 链路

**目标**: 让 llm 链路的 repair 返回结构化载荷

**修改文件**: `crates/rocode-session/src/llm.rs`

**修改**:

```rust
// 当前只返回 String，改为返回完整 ToolCall
pub fn repair_tool_call(
    name: &str, 
    input: &Value,
    tools: &HashMap<String, ToolDefinition>
) -> (String, Value) {  // ← 返回 (name, input)
    // Exact match
    if tools.contains_key(name) {
        return (name.to_string(), input.clone());
    }
    
    // Try lowercase
    let lower = name.to_lowercase();
    if lower != name && tools.contains_key(&lower) {
        return (lower, input.clone());
    }
    
    // No match - return "invalid" with structured payload
    let invalid_input = serde_json::json!({
        "tool": name,
        "error": format!("Unknown tool requested by model: {}", name),
        "receivedArgs": input
    });
    
    ("invalid".to_string(), invalid_input)
}
```

---

### P2 - 增强 Provider 解析层

**目标**: 过滤无效工具名（符号、emoji 等）

**修改文件**: `crates/rocode-provider/src/stream.rs`

**修改**:

```rust
// 添加工具名有效性检查
fn is_valid_tool_name(name: &str) -> bool {
    // 不能为空
    if name.trim().is_empty() {
        return false;
    }
    
    // 必须包含至少一个字母数字
    if !name.chars().any(|c| c.is_alphanumeric()) {
        return false;  // ← 过滤 ⚙ 等纯符号
    }
    
    // 可选：检查是否包含非法字符
    // ...
    
    true
}

// 在 parse_openai_sse 中使用
if has_name && is_valid_tool_name(&func.name.clone().unwrap_or_default()) {
    events.push(StreamEvent::ToolCallStart { ... });
}
```

---

### P3 - 增强系统提示

**目标**: 明确指导模型重试

**修改文件**: `crates/rocode-session/src/prompt_templates/anthropic.txt`

**添加章节**:

```
## Tool Error Handling

When a tool returns an error:
1. **Analyze the error message** - Understand what went wrong
2. **Fix the parameters** - If the error indicates missing or invalid arguments, correct them
3. **Retry the tool call** - Call the tool again with the fixed parameters
4. **Do not give up** - Try at least 2-3 times before considering alternatives

Example conversation:
User: "Read the file"
Assistant: [calls read tool with empty args {}]
Tool Result: "Invalid arguments: missing required field(s): filePath. Please rewrite the input so it satisfies the expected schema."
Assistant: [understands the error, calls read again with correct filePath="/path/to/file"]
Tool Result: [file content]

**Important**: When you see "Invalid arguments" or "missing required field(s)", you MUST fix the parameters and retry. Do not stop or ask the user for help unless you've tried multiple times.
```

---

## 实施计划

| 优先级 | 任务 | 预计工时 | 负责人 |
|--------|------|----------|--------|
| P0 | 统一 agent/executor 链路（预检 + 回退） | 2-3 小时 | |
| P1 | 增强 llm 链路（结构化载荷） | 1 小时 | |
| P2 | 增强 Provider 解析层（过滤符号名） | 30 分钟 | |
| P3 | 增强系统提示（重试指导） | 30 分钟 | |

**总预计工时**: 4-5 小时

---

## 验证方案

修复后需要验证：

1. **空参数测试**: 模型发送 `{}` 调用 read 工具 → 应该触发预检 → 回退到 invalid → 模型重试
2. **符号名测试**: 模型发送 `⚙` 工具名 → 应该被 Provider 过滤
3. **缺失字段测试**: 模型调用 read 缺少 filePath → 应该触发预检 → 回退到 invalid → 模型重试
4. **系统提示测试**: 模型收到 invalid 错误后 → 应该会重试至少 2 次

---

## 总结

**根本原因**: 3 条执行链路行为不一致，agent/executor 链路缺少预检和回退机制

**解决方案**: 统一 3 条链路，都实现：
1. ✅ 预检（missing_required_fields）
2. ✅ 结构化错误载荷
3. ✅ invalid 工具回退
4. ✅ 系统提示重试指导

**预期效果**: 模型发送空参数或错误参数时，会自动触发预检 → 回退 → 重试循环，而不是直接失败
