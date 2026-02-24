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
                MessageRole::Assistant => rocode_provider::Message::assistant(&m.content),
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
