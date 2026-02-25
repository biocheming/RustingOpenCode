use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

impl AgentMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            tool_result: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            tool_result: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_result: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_result: None,
            tool_calls,
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        let content = content.into();
        Self {
            role: MessageRole::Tool,
            content: content.clone(),
            tool_result: Some(ToolResult {
                tool_call_id: tool_call_id.into(),
                name: name.into(),
                content,
                is_error,
            }),
            tool_calls: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<AgentMessage>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn with_system_prompt(prompt: impl Into<String>) -> Self {
        let mut conv = Self::new();
        conv.messages.push(AgentMessage::system(prompt));
        conv
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(AgentMessage::user(content));
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push(AgentMessage::assistant(content));
    }

    pub fn add_assistant_message_with_tools(
        &mut self,
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
    ) {
        self.messages
            .push(AgentMessage::assistant_with_tools(content, tool_calls));
    }

    pub fn add_tool_result(
        &mut self,
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) {
        self.messages.push(AgentMessage::tool_result(
            tool_call_id,
            name,
            content,
            is_error,
        ));
    }

    pub fn to_provider_messages(&self) -> Vec<rocode_provider::Message> {
        self.messages
            .iter()
            .map(|m| match &m.role {
                MessageRole::System => rocode_provider::Message::system(&m.content),
                MessageRole::User => rocode_provider::Message::user(&m.content),
                MessageRole::Assistant => {
                    if m.tool_calls.is_empty() {
                        rocode_provider::Message::assistant(&m.content)
                    } else {
                        let mut parts = Vec::new();
                        if !m.content.is_empty() {
                            parts.push(rocode_provider::ContentPart {
                                content_type: "text".to_string(),
                                text: Some(m.content.clone()),
                                image_url: None,
                                tool_use: None,
                                tool_result: None,
                                cache_control: None,
                                filename: None,
                                media_type: None,
                                provider_options: None,
                            });
                        }
                        for call in &m.tool_calls {
                            parts.push(rocode_provider::ContentPart {
                                content_type: "tool_use".to_string(),
                                text: None,
                                image_url: None,
                                tool_use: Some(rocode_provider::ToolUse {
                                    id: call.id.clone(),
                                    name: call.name.clone(),
                                    input: call.arguments.clone(),
                                }),
                                tool_result: None,
                                cache_control: None,
                                filename: None,
                                media_type: None,
                                provider_options: None,
                            });
                        }
                        rocode_provider::Message {
                            role: rocode_provider::Role::Assistant,
                            content: rocode_provider::Content::Parts(parts),
                            cache_control: None,
                            provider_options: None,
                        }
                    }
                }
                MessageRole::Tool => {
                    let tool_result =
                        m.tool_result
                            .as_ref()
                            .map(|result| rocode_provider::ToolResult {
                                tool_use_id: result.tool_call_id.clone(),
                                content: result.content.clone(),
                                is_error: Some(result.is_error),
                            });

                    rocode_provider::Message {
                        role: rocode_provider::Role::Tool,
                        content: if let Some(result) = tool_result {
                            rocode_provider::Content::Parts(vec![rocode_provider::ContentPart {
                                content_type: "tool_result".to_string(),
                                text: None,
                                image_url: None,
                                tool_use: None,
                                tool_result: Some(result),
                                cache_control: None,
                                filename: None,
                                media_type: None,
                                provider_options: None,
                            }])
                        } else {
                            rocode_provider::Content::Text(m.content.clone())
                        },
                        cache_control: None,
                        provider_options: None,
                    }
                }
            })
            .collect()
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_with_tool_calls_serializes_tool_use_parts() {
        let mut conversation = Conversation::new();
        conversation.add_assistant_message_with_tools(
            "",
            vec![ToolCall {
                id: "tool-call-0".to_string(),
                name: "ls".to_string(),
                arguments: serde_json::json!({"path":"."}),
            }],
        );

        let provider_messages = conversation.to_provider_messages();
        assert_eq!(provider_messages.len(), 1);
        let message = &provider_messages[0];
        match &message.content {
            rocode_provider::Content::Parts(parts) => {
                assert!(parts.iter().any(|part| {
                    part.content_type == "tool_use"
                        && part
                            .tool_use
                            .as_ref()
                            .map(|tool| {
                                tool.name == "ls"
                                    && tool.id == "tool-call-0"
                                    && tool.input == serde_json::json!({"path":"."})
                            })
                            .unwrap_or(false)
                }));
            }
            rocode_provider::Content::Text(_) => {
                panic!("assistant message with tool calls must serialize as parts");
            }
        }
    }
}
