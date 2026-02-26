use std::collections::HashSet;
use std::sync::Arc;

use rocode_provider::{ChatRequest, Content, Message, Provider, ToolDefinition};

use super::{AgentParams, ModelRef};

#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub fn tool_definitions_from_schemas(schemas: Vec<ToolSchema>) -> Vec<ToolDefinition> {
    schemas
        .into_iter()
        .map(|s| ToolDefinition {
            name: s.name,
            description: Some(s.description),
            parameters: s.parameters,
        })
        .collect()
}

pub struct SubtaskExecutor {
    pub agent_name: String,
    pub prompt: String,
    pub description: Option<String>,
    pub model: Option<ModelRef>,
    pub agent_params: AgentParams,
}

impl SubtaskExecutor {
    pub fn new(agent_name: &str, prompt: &str) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            prompt: prompt.to_string(),
            description: None,
            model: None,
            agent_params: AgentParams::default(),
        }
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn with_model(mut self, model: ModelRef) -> Self {
        self.model = Some(model);
        self
    }

    pub async fn execute(
        &self,
        provider: Arc<dyn Provider>,
        tool_registry: &rocode_tool::ToolRegistry,
        ctx: &rocode_tool::ToolContext,
    ) -> anyhow::Result<String> {
        let model = self.model.as_ref().cloned().unwrap_or(ModelRef {
            provider_id: "default".to_string(),
            model_id: "default".to_string(),
        });
        let model_ref = format!("{}:{}", model.provider_id, model.model_id);
        let title = self
            .description
            .clone()
            .unwrap_or_else(|| "Subtask".to_string());

        let subsession_id = ctx
            .do_create_subsession(
                self.agent_name.clone(),
                Some(title.clone()),
                Some(model_ref),
                vec!["todowrite".to_string(), "todoread".to_string()],
            )
            .await
            .unwrap_or_else(|_| format!("task_{}_{}", self.agent_name, uuid::Uuid::new_v4()));

        if let Ok(output) = ctx
            .do_prompt_subsession(subsession_id.clone(), self.prompt.clone())
            .await
        {
            return Ok(format!(
                "task_id: {} (for resuming to continue this task if needed)\n\n<task_result>\n{}\n</task_result>",
                subsession_id, output
            ));
        }

        let output = self.execute_inline(provider, tool_registry, &[]).await?;
        Ok(format!(
            "task_id: {} (for resuming to continue this task if needed)\n\n<task_result>\n{}\n</task_result>",
            subsession_id, output
        ))
    }

    pub async fn execute_inline(
        &self,
        provider: Arc<dyn Provider>,
        tool_registry: &rocode_tool::ToolRegistry,
        disabled_tools: &[String],
    ) -> anyhow::Result<String> {
        let model = self.model.as_ref().cloned().unwrap_or(ModelRef {
            provider_id: "default".to_string(),
            model_id: "default".to_string(),
        });
        let disabled: HashSet<&str> = disabled_tools.iter().map(|s| s.as_str()).collect();
        let tools = tool_registry.list_schemas().await;
        let tool_defs: Vec<ToolDefinition> = tools
            .into_iter()
            .filter(|s| !disabled.contains(s.name.as_str()))
            .map(|s| ToolDefinition {
                name: s.name,
                description: Some(s.description),
                parameters: s.parameters,
            })
            .collect();

        let messages = vec![Message::user(&self.prompt)];

        let request = ChatRequest {
            model: model.model_id,
            messages,
            max_tokens: Some(self.agent_params.max_tokens.unwrap_or(8192)),
            temperature: self.agent_params.temperature,
            system: None,
            tools: Some(tool_defs),
            stream: Some(false),
            top_p: self.agent_params.top_p,
            variant: None,
            provider_options: None,
        };

        let response = provider.chat(request).await?;

        let output = response
            .choices
            .first()
            .and_then(|c| match &c.message.content {
                Content::Text(text) => Some(text.clone()),
                Content::Parts(parts) => parts.first().and_then(|p| p.text.clone()),
            })
            .unwrap_or_default();

        Ok(output)
    }
}
