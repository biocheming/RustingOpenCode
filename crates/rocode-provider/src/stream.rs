use crate::provider::ProviderError;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    /// Stream has started.
    Start,
    /// Incremental text content.
    TextDelta(String),
    /// Start of a text block.
    TextStart,
    /// End of a text block.
    TextEnd,
    /// Start of a reasoning/thinking block.
    ReasoningStart {
        id: String,
    },
    /// Incremental reasoning text.
    ReasoningDelta {
        id: String,
        text: String,
    },
    /// End of a reasoning/thinking block.
    ReasoningEnd {
        id: String,
    },
    /// Start of tool input streaming (tool-input-start in TS).
    ToolInputStart {
        id: String,
        tool_name: String,
    },
    /// Incremental tool input JSON (tool-input-delta in TS).
    ToolInputDelta {
        id: String,
        delta: String,
    },
    /// End of tool input streaming (tool-input-end in TS).
    ToolInputEnd {
        id: String,
    },
    /// Full tool call event (after input is fully assembled).
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallDelta {
        id: String,
        input: String,
    },
    ToolCallEnd {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result received.
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        input: Option<serde_json::Value>,
        output: ToolResultOutput,
    },
    /// Tool error received.
    ToolError {
        tool_call_id: String,
        tool_name: String,
        input: Option<serde_json::Value>,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<ToolErrorKind>,
    },
    /// Start of a processing step (maps to start-step in TS).
    StartStep,
    /// End of a processing step with usage info (maps to finish-step in TS).
    FinishStep {
        finish_reason: Option<String>,
        usage: StreamUsage,
        provider_metadata: Option<serde_json::Value>,
    },
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// Stream finished (maps to "finish" in TS).
    Finish,
    Done,
    Error(String),
}

/// Type-safe tool error category for streaming tool failures.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorKind {
    PermissionDenied,
    QuestionRejected,
    ExecutionError,
}

/// Output from a tool result event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultOutput {
    pub output: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<serde_json::Value>>,
}

/// Usage information from a step completion.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

pub type StreamResult = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAISSEvent {
    #[serde(default)]
    pub choices: Vec<OpenAIChoice>,
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChoice {
    #[serde(default)]
    pub delta: Option<OpenAIDelta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIDelta {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    pub reasoning_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIToolCall {
    #[serde(default)]
    pub index: u32,
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<OpenAIFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAIFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

fn openai_tool_call_id(tc: &OpenAIToolCall) -> String {
    // Always use index-based ID for consistency across stream chunks.
    // The first chunk may carry an explicit `id` (e.g. "call_xxx") while
    // subsequent delta chunks only have `index`, causing ID mismatches
    // that result in orphaned tool-call entries with empty names.
    format!("tool-call-{}", tc.index)
}

fn anthropic_tool_call_id(index: Option<u32>, explicit_id: Option<&str>) -> String {
    if let Some(index) = index {
        return format!("tool-call-{}", index);
    }
    explicit_id.unwrap_or_default().to_string()
}

pub fn parse_openai_sse(data: &str) -> Option<StreamEvent> {
    if data == "[DONE]" {
        return Some(StreamEvent::Done);
    }

    let event: OpenAISSEvent = serde_json::from_str(data).ok()?;

    for choice in event.choices {
        if let Some(delta) = &choice.delta {
            if let Some(content) = &delta.content {
                if !content.is_empty() {
                    return Some(StreamEvent::TextDelta(content.clone()));
                }
            }

            if let Some(tool_calls) = &delta.tool_calls {
                for tc in tool_calls {
                    if let Some(func) = &tc.function {
                        // Ollama-compatible models may send empty tool names;
                        // treat those as absent so we don't create ghost entries.
                        let has_name = func.name.as_deref().is_some_and(|n| !n.is_empty());
                        let has_args = func.arguments.as_deref().is_some_and(|a| !a.is_empty());

                        if has_name {
                            return Some(StreamEvent::ToolCallStart {
                                id: openai_tool_call_id(tc),
                                name: func.name.clone().unwrap_or_default(),
                            });
                        }
                        if has_args {
                            return Some(StreamEvent::ToolCallDelta {
                                id: openai_tool_call_id(tc),
                                input: func.arguments.clone().unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }

        if let Some(reason) = &choice.finish_reason {
            if reason == "stop" {
                return Some(StreamEvent::Done);
            }
        }
    }

    if let Some(usage) = event.usage {
        return Some(StreamEvent::Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
        });
    }

    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub index: Option<u32>,
    pub delta: Option<AnthropicDelta>,
    pub content_block: Option<AnthropicContentBlock>,
    pub message: Option<AnthropicMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicDelta {
    #[serde(rename = "type")]
    pub delta_type: Option<String>,
    pub text: Option<String>,
    pub partial_json: Option<String>,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub id: Option<String>,
    pub name: Option<String>,
    pub input: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub usage: Option<AnthropicUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub fn parse_anthropic_sse(data: &str) -> Option<StreamEvent> {
    let event: AnthropicEvent = serde_json::from_str(data).ok()?;

    match event.event_type.as_str() {
        "content_block_delta" => {
            if let Some(delta) = event.delta {
                if let Some(text) = delta.text {
                    return Some(StreamEvent::TextDelta(text));
                }
                if let Some(json) = delta.partial_json {
                    return Some(StreamEvent::ToolCallDelta {
                        id: anthropic_tool_call_id(event.index, None),
                        input: json,
                    });
                }
            }
        }
        "content_block_start" => {
            if let Some(block) = event.content_block {
                if block.block_type == "tool_use" {
                    return Some(StreamEvent::ToolCallStart {
                        id: anthropic_tool_call_id(event.index, block.id.as_deref()),
                        name: block.name.unwrap_or_default(),
                    });
                }
            }
        }
        "content_block_stop" => {
            // content_block_stop only marks the end of a single content block
            // (text, tool_use, thinking, etc.), NOT the end of the entire message.
            // The stream continues with more blocks. The real end signal is
            // "message_stop" or a stop_reason in "message_delta".
            return Some(StreamEvent::TextEnd);
        }
        "message_stop" => {
            return Some(StreamEvent::Done);
        }
        "message_delta" => {
            if let Some(delta) = event.delta {
                if delta.stop_reason.is_some() {
                    return Some(StreamEvent::Done);
                }
            }
        }
        "message_start" => {
            if let Some(msg) = event.message {
                if let Some(usage) = msg.usage {
                    return Some(StreamEvent::Usage {
                        prompt_tokens: usage.input_tokens,
                        completion_tokens: usage.output_tokens,
                    });
                }
            }
        }
        _ => {}
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_openai_sse_uses_fallback_id_for_tool_start() {
        let data =
            r#"{"choices":[{"delta":{"tool_calls":[{"index":2,"function":{"name":"bash"}}]}}]}"#;
        let event = parse_openai_sse(data).expect("event should parse");
        match event {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "tool-call-2");
                assert_eq!(name, "bash");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn parse_openai_sse_uses_fallback_id_for_tool_delta() {
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":2,"function":{"arguments":"{\"x\":1}"}}]}}]}"#;
        let event = parse_openai_sse(data).expect("event should parse");
        match event {
            StreamEvent::ToolCallDelta { id, input } => {
                assert_eq!(id, "tool-call-2");
                assert_eq!(input, "{\"x\":1}");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn parse_openai_sse_does_not_end_on_tool_calls_finish_reason() {
        let data = r#"{"choices":[{"finish_reason":"tool_calls"}]}"#;
        let event = parse_openai_sse(data);
        assert!(
            event.is_none(),
            "tool_calls finish_reason should not end stream"
        );
    }

    #[test]
    fn parse_anthropic_sse_uses_index_for_tool_start_id() {
        let data = r#"{"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"toolu_abc","name":"bash","input":{}}}"#;
        let event = parse_anthropic_sse(data).expect("event should parse");
        match event {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "tool-call-3");
                assert_eq!(name, "bash");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn parse_anthropic_sse_uses_index_for_tool_delta_id() {
        let data = r#"{"type":"content_block_delta","index":3,"delta":{"partial_json":"{\"cmd\":\"ls\"}"}}"#;
        let event = parse_anthropic_sse(data).expect("event should parse");
        match event {
            StreamEvent::ToolCallDelta { id, input } => {
                assert_eq!(id, "tool-call-3");
                assert_eq!(input, "{\"cmd\":\"ls\"}");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn parse_openai_sse_skips_empty_tool_name() {
        // Ollama models sometimes send an empty tool name string.
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":""}}]}}]}"#;
        let event = parse_openai_sse(data);
        assert!(event.is_none(), "empty tool name should be ignored");
    }

    #[test]
    fn parse_openai_sse_skips_null_tool_name_emits_args() {
        // When name is absent but arguments are present, emit ToolCallDelta.
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"path\":\".\"}"}}]}}]}"#;
        let event = parse_openai_sse(data).expect("event should parse");
        match event {
            StreamEvent::ToolCallDelta { id, input } => {
                assert_eq!(id, "tool-call-1");
                assert_eq!(input, r#"{"path":"."}"#);
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn parse_openai_sse_ignores_explicit_id_for_consistent_tool_call_ids() {
        // When the first chunk has an explicit id like "call_xxx", we still
        // use the index-based ID so that subsequent delta chunks (which lack
        // the explicit id) produce matching IDs.
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc123","function":{"name":"read"}}]}}]}"#;
        let event = parse_openai_sse(data).expect("event should parse");
        match event {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "tool-call-0", "should use index, not explicit id");
                assert_eq!(name, "read");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }
}
