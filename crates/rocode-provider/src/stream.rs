use crate::provider::ProviderError;
use futures::{stream, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
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

/// Returns true when the input is a complete and parseable JSON value.
pub fn is_parsable_json(s: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(s).is_ok()
}

#[derive(Debug, Clone)]
struct ToolCallAssembler {
    id: String,
    name: String,
    arguments: String,
    finished: bool,
}

impl ToolCallAssembler {
    fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            arguments: String::new(),
            finished: false,
        }
    }

    fn append(&mut self, delta: &str) {
        if !self.finished {
            self.arguments.push_str(delta);
        }
    }

    fn try_emit(&mut self) -> Option<StreamEvent> {
        if self.finished || self.arguments.is_empty() {
            return None;
        }

        let input: serde_json::Value = serde_json::from_str(&self.arguments).ok()?;
        self.finished = true;
        Some(StreamEvent::ToolCallEnd {
            id: self.id.clone(),
            name: self.name.clone(),
            input,
        })
    }

    fn flush(self) -> Option<StreamEvent> {
        if self.arguments.is_empty() || self.finished {
            return None;
        }

        let input = serde_json::from_str(&self.arguments)
            .unwrap_or_else(|_| serde_json::Value::String(self.arguments.clone()));
        Some(StreamEvent::ToolCallEnd {
            id: self.id,
            name: self.name,
            input,
        })
    }
}

fn flush_tool_call_assemblers(
    assemblers: &mut HashMap<String, ToolCallAssembler>,
    out: &mut VecDeque<Result<StreamEvent, ProviderError>>,
) {
    let mut pending: Vec<ToolCallAssembler> = assemblers.drain().map(|(_, asm)| asm).collect();
    pending.sort_by(|a, b| a.id.cmp(&b.id));

    for assembler in pending {
        if let Some(event) = assembler.flush() {
            out.push_back(Ok(event));
        }
    }
}

/// Wraps a stream and assembles `ToolCallStart`/`ToolCallDelta` fragments into
/// `ToolCallEnd` events. Existing `ToolCallEnd` events are passed through.
pub fn assemble_tool_calls(inner: StreamResult) -> StreamResult {
    let state = (
        inner,
        HashMap::<String, ToolCallAssembler>::new(),
        VecDeque::<Result<StreamEvent, ProviderError>>::new(),
        false,
    );

    Box::pin(stream::unfold(
        state,
        |(mut inner, mut assemblers, mut pending, mut eof)| async move {
            loop {
                if let Some(item) = pending.pop_front() {
                    return Some((item, (inner, assemblers, pending, eof)));
                }

                if eof {
                    return None;
                }

                match inner.next().await {
                    Some(Ok(event)) => match event {
                        StreamEvent::ToolCallStart { id, name } => {
                            assemblers.insert(
                                id.clone(),
                                ToolCallAssembler::new(id.clone(), name.clone()),
                            );
                            pending.push_back(Ok(StreamEvent::ToolCallStart { id, name }));
                        }
                        StreamEvent::ToolCallDelta { id, input } => {
                            if let Some(assembler) = assemblers.get_mut(&id) {
                                assembler.append(&input);
                                pending.push_back(Ok(StreamEvent::ToolCallDelta {
                                    id: id.clone(),
                                    input,
                                }));
                                if let Some(end_event) = assembler.try_emit() {
                                    pending.push_back(Ok(end_event));
                                }
                            } else {
                                pending.push_back(Ok(StreamEvent::ToolCallDelta { id, input }));
                            }
                        }
                        StreamEvent::ToolCallEnd { id, name, input } => {
                            assemblers.remove(&id);
                            pending.push_back(Ok(StreamEvent::ToolCallEnd { id, name, input }));
                        }
                        StreamEvent::Done => {
                            flush_tool_call_assemblers(&mut assemblers, &mut pending);
                            pending.push_back(Ok(StreamEvent::Done));
                        }
                        other => pending.push_back(Ok(other)),
                    },
                    Some(Err(err)) => pending.push_back(Err(err)),
                    None => {
                        flush_tool_call_assemblers(&mut assemblers, &mut pending);
                        eof = true;
                    }
                }
            }
        },
    ))
}

pub fn parse_openai_sse(data: &str) -> Vec<StreamEvent> {
    if data == "[DONE]" {
        return vec![StreamEvent::Done];
    }

    let event: OpenAISSEvent = match serde_json::from_str(data) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut events = Vec::new();
    let usage = event.usage.as_ref().map(|u| StreamUsage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        ..Default::default()
    });

    for choice in event.choices {
        if let Some(delta) = &choice.delta {
            if let Some(content) = &delta.content {
                if !content.is_empty() {
                    events.push(StreamEvent::TextDelta(content.clone()));
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
                            events.push(StreamEvent::ToolCallStart {
                                id: openai_tool_call_id(tc),
                                name: func.name.clone().unwrap_or_default(),
                            });
                        }
                        if has_args {
                            events.push(StreamEvent::ToolCallDelta {
                                id: openai_tool_call_id(tc),
                                input: func.arguments.clone().unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }

        if let Some(reason) = &choice.finish_reason {
            match reason.as_str() {
                "stop" => {
                    events.push(StreamEvent::FinishStep {
                        finish_reason: Some("stop".to_string()),
                        usage: usage.clone().unwrap_or_default(),
                        provider_metadata: None,
                    });
                    events.push(StreamEvent::Done);
                }
                "tool_calls" => {
                    events.push(StreamEvent::FinishStep {
                        finish_reason: Some("tool-calls".to_string()),
                        usage: usage.clone().unwrap_or_default(),
                        provider_metadata: None,
                    });
                    events.push(StreamEvent::Done);
                }
                _ => {}
            }
        }
    }

    if let Some(usage) = event.usage {
        events.push(StreamEvent::Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
        });
    }

    events
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
    use futures::StreamExt;

    fn mock_stream(events: Vec<StreamEvent>) -> StreamResult {
        Box::pin(futures::stream::iter(
            events
                .into_iter()
                .map(|event| Ok::<_, ProviderError>(event)),
        ))
    }

    async fn collect_events(stream: StreamResult) -> Vec<StreamEvent> {
        stream
            .map(|item| item.expect("expected Ok stream event"))
            .collect::<Vec<_>>()
            .await
    }

    #[test]
    fn is_parsable_json_checks_complete_json() {
        assert!(is_parsable_json(r#"{"key":"value"}"#));
        assert!(!is_parsable_json(r#"{"key":"#));
        assert!(!is_parsable_json(""));
    }

    #[test]
    fn tool_call_assembler_emits_when_json_is_complete() {
        let mut assembler = ToolCallAssembler::new("tool-call-0".into(), "read".into());
        assembler.append("{\"path\":\"");
        assert!(assembler.try_emit().is_none());

        assembler.append("/tmp/a\"}");
        let event = assembler.try_emit().expect("should emit ToolCallEnd");
        match event {
            StreamEvent::ToolCallEnd { id, name, input } => {
                assert_eq!(id, "tool-call-0");
                assert_eq!(name, "read");
                assert_eq!(input, serde_json::json!({"path": "/tmp/a"}));
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[tokio::test]
    async fn assemble_tool_calls_emits_mid_stream_tool_call_end() {
        let stream = mock_stream(vec![
            StreamEvent::ToolCallStart {
                id: "tool-call-0".into(),
                name: "read".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "tool-call-0".into(),
                input: "{\"path\":\"".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "tool-call-0".into(),
                input: "/tmp/a\"}".into(),
            },
            StreamEvent::TextDelta("after-tool-call".into()),
            StreamEvent::Done,
        ]);

        let events = collect_events(assemble_tool_calls(stream)).await;
        assert!(matches!(events[0], StreamEvent::ToolCallStart { .. }));
        assert!(matches!(events[1], StreamEvent::ToolCallDelta { .. }));
        assert!(matches!(events[2], StreamEvent::ToolCallDelta { .. }));
        assert!(matches!(events[3], StreamEvent::ToolCallEnd { .. }));
        assert!(matches!(events[4], StreamEvent::TextDelta(_)));
        assert!(matches!(events[5], StreamEvent::Done));
    }

    #[tokio::test]
    async fn assemble_tool_calls_flushes_unfinished_tool_call_on_done() {
        let stream = mock_stream(vec![
            StreamEvent::ToolCallStart {
                id: "tool-call-0".into(),
                name: "read".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "tool-call-0".into(),
                input: r#"{"path":"incomplete""#.into(),
            },
            StreamEvent::Done,
        ]);

        let events = collect_events(assemble_tool_calls(stream)).await;
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::ToolCallEnd {
                id,
                name,
                input
            } if id == "tool-call-0"
                && name == "read"
                && input == &serde_json::Value::String(r#"{"path":"incomplete""#.to_string())
        )));
    }

    #[tokio::test]
    async fn assemble_tool_calls_passthrough_existing_tool_call_end() {
        let stream = mock_stream(vec![
            StreamEvent::ToolCallEnd {
                id: "tool-call-9".into(),
                name: "read".into(),
                input: serde_json::json!({"path": "/tmp/z"}),
            },
            StreamEvent::Done,
        ]);

        let events = collect_events(assemble_tool_calls(stream)).await;
        let end_count = events
            .iter()
            .filter(|event| matches!(event, StreamEvent::ToolCallEnd { .. }))
            .count();
        assert_eq!(
            end_count, 1,
            "existing ToolCallEnd should not be duplicated"
        );
    }

    #[test]
    fn parse_openai_sse_uses_fallback_id_for_tool_start() {
        let data =
            r#"{"choices":[{"delta":{"tool_calls":[{"index":2,"function":{"name":"bash"}}]}}]}"#;
        let events = parse_openai_sse(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
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
        let events = parse_openai_sse(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ToolCallDelta { id, input } => {
                assert_eq!(id, "tool-call-2");
                assert_eq!(input, "{\"x\":1}");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn parse_openai_sse_ends_on_tool_calls_finish_reason() {
        let data = r#"{"choices":[{"finish_reason":"tool_calls"}]}"#;
        let events = parse_openai_sse(data);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            StreamEvent::FinishStep {
                finish_reason: Some(ref reason),
                ..
            } if reason == "tool-calls"
        ));
        assert!(
            matches!(events[1], StreamEvent::Done),
            "tool_calls finish_reason should emit Done"
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
        let events = parse_openai_sse(data);
        assert!(events.is_empty(), "empty tool name should be ignored");
    }

    #[test]
    fn parse_openai_sse_skips_null_tool_name_emits_args() {
        // When name is absent but arguments are present, emit ToolCallDelta.
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"path\":\".\"}"}}]}}]}"#;
        let events = parse_openai_sse(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
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
        let events = parse_openai_sse(data);
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "tool-call-0", "should use index, not explicit id");
                assert_eq!(name, "read");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn parse_openai_sse_emits_both_start_and_delta_in_same_event() {
        // When both name and arguments arrive in the same SSE event,
        // both ToolCallStart and ToolCallDelta should be emitted.
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_xyz","function":{"name":"read","arguments":"{\"file_path\":\"/tmp/test\"}"}}]}}]}"#;
        let events = parse_openai_sse(data);
        assert_eq!(events.len(), 2, "should emit both start and delta");
        match &events[0] {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "tool-call-0");
                assert_eq!(name, "read");
            }
            other => panic!("expected ToolCallStart, got: {:?}", other),
        }
        match &events[1] {
            StreamEvent::ToolCallDelta { id, input } => {
                assert_eq!(id, "tool-call-0");
                assert_eq!(input, r#"{"file_path":"/tmp/test"}"#);
            }
            other => panic!("expected ToolCallDelta, got: {:?}", other),
        }
    }
}
