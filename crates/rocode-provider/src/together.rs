use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;

use crate::{
    ChatRequest, ChatResponse, ModelInfo, Provider, ProviderError, StreamEvent, StreamResult,
};

const TOGETHER_API_URL: &str = "https://api.together.xyz/v1/chat/completions";

#[derive(Debug)]
pub struct TogetherProvider {
    client: Client,
    api_key: String,
    models: Vec<ModelInfo>,
}

impl TogetherProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.into(),
            models: vec![
                ModelInfo {
                    id: "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
                    name: "Llama 3.3 70B (Together)".to_string(),
                    provider: "together".to_string(),
                    context_window: 131072,
                    max_output_tokens: 8192,
                    supports_vision: false,
                    supports_tools: true,
                    cost_per_million_input: 0.88,
                    cost_per_million_output: 0.88,
                },
                ModelInfo {
                    id: "meta-llama/Llama-3.2-90B-Vision-Instruct-Turbo".to_string(),
                    name: "Llama 3.2 90B Vision (Together)".to_string(),
                    provider: "together".to_string(),
                    context_window: 131072,
                    max_output_tokens: 8192,
                    supports_vision: true,
                    supports_tools: true,
                    cost_per_million_input: 0.88,
                    cost_per_million_output: 0.88,
                },
                ModelInfo {
                    id: "mistralai/Mixtral-8x7B-Instruct-v0.1".to_string(),
                    name: "Mixtral 8x7B (Together)".to_string(),
                    provider: "together".to_string(),
                    context_window: 32768,
                    max_output_tokens: 4096,
                    supports_vision: false,
                    supports_tools: false,
                    cost_per_million_input: 0.6,
                    cost_per_million_output: 0.6,
                },
                ModelInfo {
                    id: "Qwen/Qwen2.5-72B-Instruct-Turbo".to_string(),
                    name: "Qwen 2.5 72B (Together)".to_string(),
                    provider: "together".to_string(),
                    context_window: 32768,
                    max_output_tokens: 8192,
                    supports_vision: false,
                    supports_tools: true,
                    cost_per_million_input: 0.88,
                    cost_per_million_output: 0.88,
                },
                ModelInfo {
                    id: "deepseek-ai/DeepSeek-V3".to_string(),
                    name: "DeepSeek V3 (Together)".to_string(),
                    provider: "together".to_string(),
                    context_window: 131072,
                    max_output_tokens: 8192,
                    supports_vision: false,
                    supports_tools: true,
                    cost_per_million_input: 1.25,
                    cost_per_million_output: 1.25,
                },
            ],
        }
    }
}

#[async_trait]
impl Provider for TogetherProvider {
    fn id(&self) -> &str {
        "together"
    }
    fn name(&self) -> &str {
        "Together AI"
    }
    fn models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }
    fn get_model(&self, id: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.id == id)
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let response = self
            .client
            .post(TOGETHER_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| ProviderError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!("{}: {}", status, body)));
        }

        response
            .json()
            .await
            .map_err(|e| ProviderError::ApiError(e.to_string()))
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<StreamResult, ProviderError> {
        let mut stream_request = request;
        stream_request.stream = Some(true);

        let response = self
            .client
            .post(TOGETHER_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&stream_request)
            .send()
            .await
            .map_err(|e| ProviderError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError(format!("{}: {}", status, body)));
        }

        let stream = response.bytes_stream().flat_map(|chunk_result| {
            let events: Vec<Result<StreamEvent, ProviderError>> = match chunk_result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut events = Vec::new();
                    for line in text.lines() {
                        if line.starts_with("data: ") {
                            let data = &line[6..];
                            events
                                .extend(crate::stream::parse_openai_sse(data).into_iter().map(Ok));
                        }
                    }
                    events
                }
                Err(e) => vec![Err(ProviderError::StreamError(e.to_string()))],
            };
            futures::stream::iter(events)
        });

        Ok(crate::stream::assemble_tool_calls(Box::pin(stream)))
    }
}
