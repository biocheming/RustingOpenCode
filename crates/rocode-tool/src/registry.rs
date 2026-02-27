use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{Tool, ToolContext, ToolError, ToolResult};
use rocode_plugin::{HookContext, HookEvent};

/// Tools that should not appear in suggestion lists when a tool is not found.
const FILTERED_FROM_SUGGESTIONS: &[&str] = &["invalid", "patch", "batch"];

fn looks_like_jsonish_payload(s: &str) -> bool {
    let trimmed = s.trim_start();
    trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with("\"{")
        || trimmed.starts_with("\"[")
        || s.contains("\":")
        || s.contains("\\\":")
        || s.contains("\"file_path\"")
        || s.contains("\\\"file_path\\\"")
        || s.contains("\"filePath\"")
        || s.contains("\\\"filePath\\\"")
        || s.contains("\"content\"")
        || s.contains("\\\"content\\\"")
}

fn parse_jsonish_string_field(input: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\"", field);
    let field_idx = input.find(&needle)?;
    let after_field = &input[field_idx + needle.len()..];
    let colon_idx = after_field.find(':')?;
    let mut chars = after_field[colon_idx + 1..].chars().peekable();

    while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
        chars.next();
    }
    if !matches!(chars.next(), Some('"')) {
        return None;
    }

    let mut out = String::new();
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if escaped {
            match ch {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{08}'),
                'f' => out.push('\u{0c}'),
                'u' => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        match chars.peek().copied() {
                            Some(c) if c.is_ascii_hexdigit() => {
                                hex.push(c);
                                chars.next();
                            }
                            _ => break,
                        }
                    }
                    if hex.len() == 4 {
                        if let Ok(code) = u32::from_str_radix(&hex, 16) {
                            if let Some(decoded) = char::from_u32(code) {
                                out.push(decoded);
                            }
                        }
                    } else {
                        out.push('u');
                        out.push_str(&hex);
                    }
                }
                other => out.push(other),
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(out),
            other => out.push(other),
        }
    }

    // Unterminated JSON string: return best-effort content.
    Some(out)
}

fn recover_write_args_from_jsonish(input: &str) -> Option<serde_json::Value> {
    fn normalize_single_escaped_quotes(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();
        let mut prev: Option<char> = None;

        while let Some(ch) = chars.next() {
            if ch == '\\' && matches!(chars.peek(), Some('"')) && prev != Some('\\') {
                out.push('"');
                chars.next();
                prev = Some('"');
                continue;
            }
            out.push(ch);
            prev = Some(ch);
        }
        out
    }

    fn recover_once(input: &str) -> Option<serde_json::Value> {
        let file_path = parse_jsonish_string_field(input, "file_path")
            .or_else(|| parse_jsonish_string_field(input, "filePath"))?;
        let content = parse_jsonish_string_field(input, "content").unwrap_or_default();
        Some(serde_json::json!({
            "file_path": file_path,
            "content": content
        }))
    }

    if let Some(recovered) = recover_once(input) {
        return Some(recovered);
    }

    // If arguments were wrapped as a JSON string, unwrap one layer and retry.
    if let Ok(inner) = serde_json::from_str::<String>(input) {
        if let Some(recovered) = recover_once(&inner) {
            return Some(recovered);
        }
    }

    // Some malformed payloads preserve escaped quotes without a valid outer JSON
    // wrapper (e.g. {\"file_path\":\"...\" ...). Best-effort normalize and retry.
    if input.contains("\\\"") {
        let de_escaped_quotes = normalize_single_escaped_quotes(input);
        if let Some(recovered) = recover_once(&de_escaped_quotes) {
            return Some(recovered);
        }
    }

    None
}

pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
}

fn rewrite_invalid_arguments(tool_id: &str, err: ToolError) -> ToolError {
    match err {
        ToolError::InvalidArguments(msg) => {
            if msg.contains("Please rewrite the input so it satisfies the expected schema.") {
                ToolError::InvalidArguments(msg)
            } else {
                ToolError::InvalidArguments(format!(
                    "The {} tool was called with invalid arguments: {}.\nPlease rewrite the input so it satisfies the expected schema.",
                    tool_id, msg
                ))
            }
        }
        other => other,
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register<T: Tool + 'static>(&self, tool: T) {
        let mut tools = self.tools.write().await;
        tools.insert(tool.id().to_string(), Arc::new(tool));
    }

    pub async fn get(&self, id: &str) -> Option<Arc<dyn Tool>> {
        let tools = self.tools.read().await;
        tools.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<Arc<dyn Tool>> {
        let tools = self.tools.read().await;
        tools.values().cloned().collect()
    }

    /// Returns all registered tool IDs.
    pub async fn list_ids(&self) -> Vec<String> {
        let tools = self.tools.read().await;
        tools.keys().cloned().collect()
    }

    /// Given a tool name that was not found, returns a list of available tool names
    /// filtered to exclude tools in `FILTERED_FROM_SUGGESTIONS`.
    pub async fn suggest_tools(&self, _requested: &str) -> Vec<String> {
        let tools = self.tools.read().await;
        let mut names: Vec<String> = tools
            .keys()
            .filter(|name| !FILTERED_FROM_SUGGESTIONS.contains(&name.as_str()))
            .cloned()
            .collect();
        names.sort();
        names
    }

    pub async fn list_schemas(&self) -> Vec<ToolSchema> {
        let tools = self.tools.read().await;
        let mut schemas: Vec<ToolSchema> = tools
            .values()
            .map(|t| ToolSchema {
                name: t.id().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect();

        // Trigger tool.definition hook for each schema so plugins can transform them
        for schema in &mut schemas {
            let hook_outputs = rocode_plugin::trigger_collect(
                HookContext::new(HookEvent::ToolDefinition)
                    .with_data("tool_id", serde_json::json!(&schema.name))
                    .with_data("description", serde_json::json!(&schema.description))
                    .with_data("parameters", schema.parameters.clone()),
            )
            .await;
            for output in hook_outputs {
                if let Some(payload) = output.payload.as_ref() {
                    apply_tool_definition_payload(schema, payload);
                }
            }
        }

        schemas
    }

    pub async fn execute(
        &self,
        tool_id: &str,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let tool = match self.get(tool_id).await {
            Some(t) => t,
            None => {
                let suggestions = self.suggest_tools(tool_id).await;
                return Err(ToolError::InvalidArguments(format!(
                    "Tool '{}' not found in registry. Available tools: {}",
                    tool_id,
                    suggestions.join(", ")
                )));
            }
        };

        let mut args = args;

        // Normalize: if args is a JSON string that contains a valid object,
        // parse it into an actual object. This happens when the stream assembler
        // fails to parse tool call arguments during streaming and wraps the raw
        // text as Value::String.
        if let Some(s) = args.as_str().map(|s| s.to_owned()) {
            if let Some(parsed @ serde_json::Value::Object(_)) =
                rocode_util::json::try_parse_json_object_robust(&s)
            {
                tracing::info!(
                    tool = %tool_id,
                    "recovered tool arguments via robust JSON parser"
                );
                args = parsed;
            } else {
                if tool_id == "write" {
                    if let Some(parsed) = recover_write_args_from_jsonish(&s) {
                        tracing::info!(
                            tool = %tool_id,
                            "recovered write arguments from JSON-ish payload"
                        );
                        args = parsed;
                    }
                }
                // If still a string, try key=value fallback.
                if args.is_string() && !looks_like_jsonish_payload(&s) {
                    // Some models (e.g. Qwen via LiteLLM) may send arguments in
                    // non-JSON formats like "key=value" or "key: value". Try to
                    // construct a JSON object from simple key=value pairs.
                    let mut obj = serde_json::Map::new();
                    for line in s.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        if let Some((key, value)) = line.split_once('=') {
                            let key = key.trim().to_string();
                            let value = value.trim();
                            // Try to parse value as JSON, otherwise treat as string
                            let json_value = serde_json::from_str(value)
                                .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
                            obj.insert(key, json_value);
                        }
                    }
                    if !obj.is_empty() {
                        tracing::info!(
                            tool = %tool_id,
                            "normalized non-JSON tool arguments from key=value format"
                        );
                        args = serde_json::Value::Object(obj);
                    }
                }
            }
        }

        // If args is still an empty object, log a warning for diagnostics.
        if args.is_object() && args.as_object().map_or(false, |o| o.is_empty()) {
            tracing::warn!(
                tool = %tool_id,
                "tool called with empty arguments object"
            );
        }

        // Plugin hook: tool.execute.before
        tracing::debug!(
            tool = %tool_id,
            "[plugin-seq] tool.execute.before"
        );
        let mut before_hook_ctx = HookContext::new(HookEvent::ToolExecuteBefore)
            .with_session(&ctx.session_id)
            .with_data("tool", serde_json::json!(tool_id))
            .with_data("args", args.clone());
        if let Some(call_id) = &ctx.call_id {
            before_hook_ctx = before_hook_ctx.with_data("call_id", serde_json::json!(call_id));
        }
        let before_outputs = rocode_plugin::trigger_collect(before_hook_ctx).await;
        for output in before_outputs {
            if let Some(payload) = output.payload.as_ref() {
                apply_tool_before_payload(&mut args, payload);
            }
        }

        tool.validate(&args)
            .map_err(|e| rewrite_invalid_arguments(tool_id, e))?;
        let mut result = tool.execute(args.clone(), ctx.clone()).await;
        if let Err(e) = &result {
            // Log the exact args when a tool fails, to diagnose argument parsing issues.
            tracing::error!(
                tool = %tool_id,
                error = %e,
                args_type = %match &args {
                    serde_json::Value::Object(o) => format!("object(keys={})", o.keys().cloned().collect::<Vec<_>>().join(",")),
                    serde_json::Value::String(s) => format!("string(len={},preview={})", s.len(), &s[..s.len().min(200)]),
                    serde_json::Value::Null => "null".to_string(),
                    serde_json::Value::Array(_) => "array".to_string(),
                    serde_json::Value::Bool(_) => "bool".to_string(),
                    serde_json::Value::Number(_) => "number".to_string(),
                },
                args_json = %serde_json::to_string(&args).unwrap_or_else(|_| "??".to_string()),
                "tool execution failed"
            );
        }
        if let Err(e) = result {
            result = Err(rewrite_invalid_arguments(tool_id, e));
        }

        // Plugin hook: tool.execute.after
        tracing::debug!(
            tool = %tool_id,
            "[plugin-seq] tool.execute.after"
        );
        let mut hook_ctx = HookContext::new(HookEvent::ToolExecuteAfter)
            .with_session(&ctx.session_id)
            .with_data("tool", serde_json::json!(tool_id))
            .with_data("args", args);
        if let Some(call_id) = &ctx.call_id {
            hook_ctx = hook_ctx.with_data("call_id", serde_json::json!(call_id));
        }

        hook_ctx = match &result {
            Ok(r) => hook_ctx
                .with_data("title", serde_json::json!(&r.title))
                .with_data("output", serde_json::json!(&r.output))
                .with_data("metadata", serde_json::json!(&r.metadata))
                .with_data("error", serde_json::json!(false)),
            Err(e) => hook_ctx
                .with_data("output", serde_json::json!(e.to_string()))
                .with_data("error", serde_json::json!(true)),
        };

        let after_outputs = rocode_plugin::trigger_collect(hook_ctx).await;
        if let Ok(tool_result) = &mut result {
            for output in after_outputs {
                if let Some(payload) = output.payload.as_ref() {
                    apply_tool_after_payload(tool_result, payload);
                }
            }
        }

        result
    }
}

fn hook_payload_object(
    payload: &serde_json::Value,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    payload
        .get("output")
        .and_then(|value| value.as_object())
        .or_else(|| payload.as_object())
        .or_else(|| payload.get("data").and_then(|value| value.as_object()))
}

fn apply_tool_definition_payload(schema: &mut ToolSchema, payload: &serde_json::Value) {
    let Some(object) = hook_payload_object(payload) else {
        return;
    };
    if let Some(description) = object.get("description").and_then(|value| value.as_str()) {
        schema.description = description.to_string();
    }
    if let Some(parameters) = object.get("parameters") {
        schema.parameters = parameters.clone();
    }
}

fn apply_tool_before_payload(args: &mut serde_json::Value, payload: &serde_json::Value) {
    let Some(object) = hook_payload_object(payload) else {
        return;
    };
    if let Some(next_args) = object.get("args") {
        *args = next_args.clone();
    }
}

fn apply_tool_after_payload(result: &mut ToolResult, payload: &serde_json::Value) {
    let Some(object) = hook_payload_object(payload) else {
        return;
    };
    if let Some(title) = object.get("title").and_then(|value| value.as_str()) {
        result.title = title.to_string();
    }
    if let Some(output) = object.get("output") {
        if let Some(output_str) = output.as_str() {
            result.output = output_str.to_string();
        } else if !output.is_null() {
            result.output = output.to_string();
        }
    }
    if let Some(metadata) = object.get("metadata").and_then(|value| value.as_object()) {
        result.metadata = metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub async fn create_default_registry() -> ToolRegistry {
    let registry = ToolRegistry::new();

    registry.register(crate::read::ReadTool::new()).await;
    registry.register(crate::write::WriteTool::new()).await;
    registry.register(crate::edit::EditTool::new()).await;
    registry.register(crate::bash::BashTool::new()).await;
    registry.register(crate::glob_tool::GlobTool::new()).await;
    registry.register(crate::grep_tool::GrepTool::new()).await;
    registry.register(crate::ls::LsTool::new()).await;
    registry.register(crate::task::TaskTool::new()).await;
    registry
        .register(crate::question::QuestionTool::new())
        .await;
    registry
        .register(crate::webfetch::WebFetchTool::new())
        .await;
    registry
        .register(crate::websearch::WebSearchTool::new())
        .await;
    registry.register(crate::todo::TodoReadTool).await;
    registry.register(crate::todo::TodoWriteTool).await;
    registry.register(crate::multiedit::MultiEditTool).await;
    registry.register(crate::apply_patch::ApplyPatchTool).await;
    registry.register(crate::skill::SkillTool).await;
    registry.register(crate::lsp_tool::LspTool).await;
    registry.register(crate::batch::BatchTool).await;
    registry.register(crate::codesearch::CodeSearchTool).await;
    registry.register(crate::plan::PlanEnterTool).await;
    registry.register(crate::plan::PlanExitTool).await;
    registry.register(crate::invalid::InvalidTool).await;

    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    struct CaptureTool {
        captured: Arc<Mutex<Option<serde_json::Value>>>,
        id: &'static str,
    }

    #[async_trait]
    impl Tool for CaptureTool {
        fn id(&self) -> &str {
            self.id
        }

        fn description(&self) -> &str {
            "Captures args for testing"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            *self.captured.lock().expect("lock should succeed") = Some(args.clone());
            let file_path = args
                .get("file_path")
                .or_else(|| args.get("filePath"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Ok(ToolResult::simple("ok", file_path))
        }
    }

    async fn setup_capture_registry() -> (ToolRegistry, Arc<Mutex<Option<serde_json::Value>>>) {
        let registry = ToolRegistry::new();
        let captured = Arc::new(Mutex::new(None));
        registry
            .register(CaptureTool {
                captured: captured.clone(),
                id: "capture",
            })
            .await;
        (registry, captured)
    }

    fn test_tool_context() -> ToolContext {
        ToolContext::new(
            "ses_test".to_string(),
            "msg_test".to_string(),
            ".".to_string(),
        )
    }

    #[tokio::test]
    async fn execute_recovers_stringified_json_object_arguments() {
        let (registry, captured) = setup_capture_registry().await;
        let inner = r#"{"file_path":"/tmp/a.html","content":"hello"}"#;
        let outer = serde_json::to_string(inner).expect("stringify should succeed");

        let result = registry
            .execute(
                "capture",
                serde_json::Value::String(outer),
                test_tool_context(),
            )
            .await
            .expect("tool should execute");

        assert_eq!(result.output, "/tmp/a.html");
        let captured_args = captured
            .lock()
            .expect("lock should succeed")
            .clone()
            .expect("args should be captured");
        assert!(captured_args.is_object());
        assert_eq!(captured_args["file_path"], "/tmp/a.html");
    }

    #[tokio::test]
    async fn execute_recovers_literal_control_characters_in_json_string_arguments() {
        let (registry, captured) = setup_capture_registry().await;
        let args = serde_json::Value::String(
            "{\"file_path\":\"/tmp/b.html\",\"content\":\"line1\nline2\"}".to_string(),
        );

        let result = registry
            .execute("capture", args, test_tool_context())
            .await
            .expect("tool should execute");

        assert_eq!(result.output, "/tmp/b.html");
        let captured_args = captured
            .lock()
            .expect("lock should succeed")
            .clone()
            .expect("args should be captured");
        assert_eq!(captured_args["file_path"], "/tmp/b.html");
        assert_eq!(captured_args["content"], "line1\nline2");
    }

    #[tokio::test]
    async fn execute_keeps_key_value_fallback_for_non_json_strings() {
        let (registry, captured) = setup_capture_registry().await;
        let args = serde_json::Value::String("file_path=/tmp/c.html\ncontent=hello".to_string());

        let result = registry
            .execute("capture", args, test_tool_context())
            .await
            .expect("tool should execute");

        assert_eq!(result.output, "/tmp/c.html");
        let captured_args = captured
            .lock()
            .expect("lock should succeed")
            .clone()
            .expect("args should be captured");
        assert_eq!(captured_args["file_path"], "/tmp/c.html");
        assert_eq!(captured_args["content"], "hello");
    }

    #[tokio::test]
    async fn execute_recovers_write_args_from_unterminated_jsonish_payload() {
        let registry = ToolRegistry::new();
        let captured = Arc::new(Mutex::new(None));
        registry
            .register(CaptureTool {
                captured: captured.clone(),
                id: "write",
            })
            .await;

        let malformed = serde_json::Value::String(
            "{\"file_path\":\"/tmp/d.html\",\"content\":\"<div class=\\\"x\\\">hello\nworld"
                .to_string(),
        );

        let result = registry
            .execute("write", malformed, test_tool_context())
            .await
            .expect("tool should execute");

        assert_eq!(result.output, "/tmp/d.html");
        let captured_args = captured
            .lock()
            .expect("lock should succeed")
            .clone()
            .expect("args should be captured");
        assert_eq!(captured_args["file_path"], "/tmp/d.html");
        assert_eq!(captured_args["content"], "<div class=\"x\">hello\nworld");
    }

    #[tokio::test]
    async fn execute_recovers_write_args_from_escaped_jsonish_payload() {
        let registry = ToolRegistry::new();
        let captured = Arc::new(Mutex::new(None));
        registry
            .register(CaptureTool {
                captured: captured.clone(),
                id: "write",
            })
            .await;

        let malformed = serde_json::Value::String(
            "{\\\"file_path\\\":\\\"/tmp/e.html\\\",\\\"content\\\":\\\"<div class=\\\\\\\"x\\\\\\\">hello\\nworld".to_string(),
        );

        let result = registry
            .execute("write", malformed, test_tool_context())
            .await
            .expect("tool should execute");

        assert_eq!(result.output, "/tmp/e.html");
        let captured_args = captured
            .lock()
            .expect("lock should succeed")
            .clone()
            .expect("args should be captured");
        assert_eq!(captured_args["file_path"], "/tmp/e.html");
        let content = captured_args["content"]
            .as_str()
            .expect("content should be string");
        assert!(content.contains("<div class="));
        assert!(content.contains("hello"));
    }
}
