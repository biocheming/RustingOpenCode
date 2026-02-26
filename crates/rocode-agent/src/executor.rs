use futures::stream::BoxStream;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{AgentInfo, Conversation, ToolCall};
use rocode_plugin::{HookContext, HookEvent};
use rocode_provider::{ChatRequest, Provider, ProviderRegistry, StreamEvent};
use rocode_tool::{ToolContext, ToolError, ToolRegistry};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("Tool error: {0}")]
    ToolError(String),

    #[error("Max steps exceeded")]
    MaxStepsExceeded,

    #[error("No provider available")]
    NoProvider,

    #[error("Invalid response")]
    InvalidResponse,
}

pub struct AgentExecutor {
    agent: AgentInfo,
    conversation: Conversation,
    providers: Arc<ProviderRegistry>,
    tools: Arc<ToolRegistry>,
    disabled_tools: HashSet<String>,
    subsessions: Arc<Mutex<HashMap<String, SubsessionState>>>,
    max_steps: u32,
}

#[derive(Debug, Clone)]
struct SubsessionState {
    agent: AgentInfo,
    conversation: Conversation,
    disabled_tools: HashSet<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedSubsessionState {
    pub agent: AgentInfo,
    pub conversation: Conversation,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
}

impl AgentExecutor {
    pub fn new(
        agent: AgentInfo,
        providers: Arc<ProviderRegistry>,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        let max_steps = agent.max_steps.unwrap_or(100);
        let conversation = Conversation::new();

        Self {
            agent,
            conversation,
            providers,
            tools,
            disabled_tools: HashSet::new(),
            subsessions: Arc::new(Mutex::new(HashMap::new())),
            max_steps,
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.conversation = Conversation::with_system_prompt(prompt);
        self
    }

    pub fn with_disabled_tools<I>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        self.disabled_tools = tools.into_iter().collect();
        self
    }

    pub fn with_persisted_subsessions(
        mut self,
        states: HashMap<String, PersistedSubsessionState>,
    ) -> Self {
        let subsessions = states
            .into_iter()
            .map(|(id, state)| {
                (
                    id,
                    SubsessionState {
                        agent: state.agent,
                        conversation: state.conversation,
                        disabled_tools: state.disabled_tools.into_iter().collect(),
                    },
                )
            })
            .collect();
        self.subsessions = Arc::new(Mutex::new(subsessions));
        self
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn conversation_mut(&mut self) -> &mut Conversation {
        &mut self.conversation
    }

    pub async fn export_subsessions(&self) -> HashMap<String, PersistedSubsessionState> {
        self.subsessions
            .lock()
            .await
            .iter()
            .map(|(id, state)| {
                (
                    id.clone(),
                    PersistedSubsessionState {
                        agent: state.agent.clone(),
                        conversation: state.conversation.clone(),
                        disabled_tools: state.disabled_tools.iter().cloned().collect(),
                    },
                )
            })
            .collect()
    }

    pub async fn execute(&mut self, user_message: impl Into<String>) -> Result<String, AgentError> {
        self.conversation.add_user_message(user_message);

        // Plugin hook: session.start
        rocode_plugin::trigger(
            HookContext::new(HookEvent::SessionStart)
                .with_data("agent", serde_json::json!(&self.agent.name))
                .with_data("max_steps", serde_json::json!(self.max_steps)),
        )
        .await;

        let mut steps = 0;
        let mut final_response = String::new();

        while steps < self.max_steps {
            steps += 1;

            let provider = self.get_provider()?;
            let model_id = self.get_model_id(&provider);

            // Plugin hook: chat.system.transform — let plugins modify system prompt per step
            rocode_plugin::trigger(
                HookContext::new(HookEvent::ChatSystemTransform)
                    .with_data("agent", serde_json::json!(&self.agent.name))
                    .with_data("model_id", serde_json::json!(&model_id))
                    .with_data("step", serde_json::json!(steps)),
            )
            .await;

            let tool_defs = self.resolve_tool_definitions().await;
            let request = ChatRequest::new(model_id, self.conversation.to_provider_messages())
                .with_tools(tool_defs);

            let stream = provider
                .chat_stream(request)
                .await
                .map_err(|e| AgentError::ProviderError(e.to_string()))?;

            let (response, tool_calls) = self.process_stream(stream).await?;

            if tool_calls.is_empty() {
                final_response = response;
                break;
            }

            self.conversation
                .add_assistant_message_with_tools(&response, tool_calls.clone());

            for tool_call in tool_calls {
                let effective_tool_call = self.repair_tool_call(tool_call).await;
                let mut result = self.execute_tool(&effective_tool_call).await;

                // Reroute InvalidArguments to the invalid tool for structured feedback
                if effective_tool_call.name != "invalid"
                    && matches!(&result, Err(ToolError::InvalidArguments(_)))
                {
                    let validation_error = result
                        .as_ref()
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_default();
                    let invalid_call = ToolCall {
                        id: effective_tool_call.id.clone(),
                        name: "invalid".to_string(),
                        arguments: serde_json::json!({
                            "tool": effective_tool_call.name,
                            "error": validation_error,
                        }),
                    };
                    result = self.execute_tool(&invalid_call).await;
                }

                let (content, is_error) = match result {
                    Ok(tool_result) => (tool_result.output, false),
                    Err(e) => (e.to_string(), true),
                };

                self.conversation.add_tool_result(
                    &effective_tool_call.id,
                    &effective_tool_call.name,
                    content,
                    is_error,
                );
            }
        }

        // Plugin hook: session.end
        rocode_plugin::trigger(
            HookContext::new(HookEvent::SessionEnd)
                .with_data("agent", serde_json::json!(&self.agent.name))
                .with_data("steps", serde_json::json!(steps)),
        )
        .await;

        if steps >= self.max_steps {
            return Err(AgentError::MaxStepsExceeded);
        }

        Ok(final_response)
    }

    async fn execute_subsession(
        &mut self,
        user_message: impl Into<String>,
    ) -> Result<String, AgentError> {
        self.conversation.add_user_message(user_message);

        let mut steps = 0;
        let mut final_response = String::new();

        while steps < self.max_steps {
            steps += 1;

            let provider = self.get_provider()?;
            let model_id = self.get_model_id(&provider);
            let tool_defs = self.resolve_tool_definitions().await;
            let request = ChatRequest::new(model_id, self.conversation.to_provider_messages())
                .with_tools(tool_defs);

            let stream = provider
                .chat_stream(request)
                .await
                .map_err(|e| AgentError::ProviderError(e.to_string()))?;

            let (response, tool_calls) = self.process_stream(stream).await?;

            if tool_calls.is_empty() {
                final_response = response;
                break;
            }

            self.conversation
                .add_assistant_message_with_tools(&response, tool_calls.clone());

            for tool_call in tool_calls {
                let effective_tool_call = self.repair_tool_call(tool_call).await;
                let mut result = self
                    .execute_tool_without_subsessions(&effective_tool_call)
                    .await;

                if effective_tool_call.name != "invalid"
                    && matches!(&result, Err(ToolError::InvalidArguments(_)))
                {
                    let validation_error = result
                        .as_ref()
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_default();
                    let invalid_call = ToolCall {
                        id: effective_tool_call.id.clone(),
                        name: "invalid".to_string(),
                        arguments: serde_json::json!({
                            "tool": effective_tool_call.name,
                            "error": validation_error,
                        }),
                    };
                    result = self.execute_tool_without_subsessions(&invalid_call).await;
                }

                let (content, is_error) = match result {
                    Ok(output) => (output, false),
                    Err(e) => (e.to_string(), true),
                };

                self.conversation.add_tool_result(
                    &effective_tool_call.id,
                    &effective_tool_call.name,
                    content,
                    is_error,
                );
            }
        }

        if steps >= self.max_steps {
            return Err(AgentError::MaxStepsExceeded);
        }

        Ok(final_response)
    }

    pub async fn execute_streaming(
        &mut self,
        user_message: String,
    ) -> Result<BoxStream<'static, Result<StreamEvent, AgentError>>, AgentError> {
        self.conversation.add_user_message(user_message);
        let mut steps = 0u32;
        let mut emitted: Vec<Result<StreamEvent, AgentError>> = Vec::new();

        while steps < self.max_steps {
            steps += 1;

            let provider = self.get_provider()?;
            let model_id = self.get_model_id(&provider);
            let tool_defs = self.resolve_tool_definitions().await;
            let request = ChatRequest::new(model_id, self.conversation.to_provider_messages())
                .with_tools(tool_defs);

            let mut stream = provider
                .chat_stream(request)
                .await
                .map_err(|e| AgentError::ProviderError(e.to_string()))?;

            let mut response = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();

            while let Some(event) = stream.next().await {
                match event {
                    Ok(StreamEvent::TextDelta(text)) => {
                        response.push_str(&text);
                        emitted.push(Ok(StreamEvent::TextDelta(text)));
                    }
                    Ok(StreamEvent::ToolCallStart { id, name }) => {
                        emitted.push(Ok(StreamEvent::ToolCallStart { id, name }));
                    }
                    Ok(StreamEvent::ToolCallDelta { id, input }) => {
                        emitted.push(Ok(StreamEvent::ToolCallDelta { id, input }));
                    }
                    Ok(StreamEvent::ToolCallEnd { id, name, input }) => {
                        if name.trim().is_empty() {
                            tracing::warn!(
                                tool_call_id = %id,
                                "ignoring ToolCallEnd with empty tool name"
                            );
                            continue;
                        }
                        emitted.push(Ok(StreamEvent::ToolCallEnd {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        }));
                        tool_calls.push(ToolCall {
                            id,
                            name,
                            arguments: input,
                        });
                    }
                    Ok(StreamEvent::ReasoningStart { id }) => {
                        emitted.push(Ok(StreamEvent::ReasoningStart { id }));
                    }
                    Ok(StreamEvent::ReasoningDelta { id, text }) => {
                        emitted.push(Ok(StreamEvent::ReasoningDelta { id, text }));
                    }
                    Ok(StreamEvent::ReasoningEnd { id }) => {
                        emitted.push(Ok(StreamEvent::ReasoningEnd { id }));
                    }
                    Ok(StreamEvent::Done) => break,
                    Ok(StreamEvent::Error(e)) => {
                        return Err(AgentError::ProviderError(e));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        return Err(AgentError::ProviderError(e.to_string()));
                    }
                }
            }

            if tool_calls.is_empty() {
                self.conversation.add_assistant_message(&response);
                emitted.push(Ok(StreamEvent::Done));
                return Ok(futures::stream::iter(emitted).boxed());
            }

            self.conversation
                .add_assistant_message_with_tools(&response, tool_calls.clone());

            for tool_call in tool_calls {
                let effective_tool_call = self.repair_tool_call(tool_call).await;
                let mut execution = self.execute_tool(&effective_tool_call).await;

                if effective_tool_call.name != "invalid"
                    && matches!(&execution, Err(ToolError::InvalidArguments(_)))
                {
                    let validation_error = execution
                        .as_ref()
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_default();
                    let invalid_call = ToolCall {
                        id: effective_tool_call.id.clone(),
                        name: "invalid".to_string(),
                        arguments: serde_json::json!({
                            "tool": effective_tool_call.name,
                            "error": validation_error,
                        }),
                    };
                    execution = self.execute_tool(&invalid_call).await;
                }

                match execution {
                    Ok(tool_result) => {
                        self.conversation.add_tool_result(
                            &effective_tool_call.id,
                            &effective_tool_call.name,
                            tool_result.output.clone(),
                            false,
                        );
                        emitted.push(Ok(StreamEvent::ToolResult {
                            tool_call_id: effective_tool_call.id.clone(),
                            tool_name: effective_tool_call.name.clone(),
                            input: Some(effective_tool_call.arguments.clone()),
                            output: rocode_provider::ToolResultOutput {
                                output: tool_result.output,
                                title: tool_result.title,
                                metadata: tool_result.metadata,
                                attachments: None,
                            },
                        }));
                    }
                    Err(error) => {
                        self.conversation.add_tool_result(
                            &effective_tool_call.id,
                            &effective_tool_call.name,
                            error.to_string(),
                            true,
                        );
                        emitted.push(Ok(StreamEvent::ToolError {
                            tool_call_id: effective_tool_call.id.clone(),
                            tool_name: effective_tool_call.name.clone(),
                            input: Some(effective_tool_call.arguments.clone()),
                            error: error.to_string(),
                            kind: Some(rocode_provider::ToolErrorKind::ExecutionError),
                        }));
                    }
                }
            }
        }

        Err(AgentError::MaxStepsExceeded)
    }

    fn get_provider(&self) -> Result<Arc<dyn Provider>, AgentError> {
        if let Some(ref model_ref) = self.agent.model {
            self.providers
                .get(&model_ref.provider_id)
                .ok_or(AgentError::NoProvider)
        } else {
            let providers = self.providers.list();
            if providers.is_empty() {
                return Err(AgentError::NoProvider);
            }
            Ok(providers.into_iter().next().unwrap())
        }
    }

    fn get_model_id(&self, provider: &Arc<dyn Provider>) -> String {
        if let Some(ref model_ref) = self.agent.model {
            model_ref.model_id.clone()
        } else {
            let models = provider.models();
            models.first().map(|m| m.id.clone()).unwrap_or_default()
        }
    }

    async fn process_stream(
        &mut self,
        mut stream: rocode_provider::StreamResult,
    ) -> Result<(String, Vec<ToolCall>), AgentError> {
        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::TextDelta(text)) => {
                    content.push_str(&text);
                }
                Ok(StreamEvent::ToolCallEnd { id, name, input }) => {
                    if name.trim().is_empty() {
                        tracing::warn!(tool_call_id = %id, "ignoring ToolCallEnd with empty tool name");
                        continue;
                    }
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments: input,
                    });
                }
                Ok(StreamEvent::Done) => break,
                Ok(StreamEvent::Error(e)) => {
                    return Err(AgentError::ProviderError(e));
                }
                Err(e) => {
                    return Err(AgentError::ProviderError(e.to_string()));
                }
                _ => {}
            }
        }

        Ok((content, tool_calls))
    }

    async fn execute_tool(
        &self,
        tool_call: &ToolCall,
    ) -> Result<rocode_tool::ToolResult, ToolError> {
        if self.disabled_tools.contains(&tool_call.name) {
            return Err(ToolError::PermissionDenied(format!(
                "Tool '{}' is disabled for this subagent session",
                tool_call.name
            )));
        }
        self.ensure_tool_allowed(&tool_call.name)?;

        let directory = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let current_model = self.current_model_string();
        let base_ctx = ToolContext::new("default".to_string(), "default".to_string(), directory)
            .with_agent(self.agent.name.clone())
            .with_get_last_model({
                let current_model = current_model.clone();
                move |_session_id| {
                    let current_model = current_model.clone();
                    async move { Ok(current_model) }
                }
            });
        let ctx = self.with_subsession_callbacks(base_ctx);

        self.tools
            .execute(&tool_call.name, tool_call.arguments.clone(), ctx)
            .await
    }

    async fn execute_tool_without_subsessions(
        &self,
        tool_call: &ToolCall,
    ) -> Result<String, ToolError> {
        if self.disabled_tools.contains(&tool_call.name) {
            return Err(ToolError::PermissionDenied(format!(
                "Tool '{}' is disabled for this subagent session",
                tool_call.name
            )));
        }
        self.ensure_tool_allowed(&tool_call.name)?;

        let directory = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let current_model = self.current_model_string();
        let base_ctx = ToolContext::new("default".to_string(), "default".to_string(), directory)
            .with_agent(self.agent.name.clone())
            .with_get_last_model({
                let current_model = current_model.clone();
                move |_session_id| {
                    let current_model = current_model.clone();
                    async move { Ok(current_model) }
                }
            });
        let ctx = self.with_subsession_callbacks(base_ctx);

        self.tools
            .execute(&tool_call.name, tool_call.arguments.clone(), ctx)
            .await
            .map(|r| r.output)
    }

    fn with_subsession_callbacks(&self, ctx: ToolContext) -> ToolContext {
        let subsessions = self.subsessions.clone();
        let providers = self.providers.clone();
        let tools = self.tools.clone();

        let ctx = ctx.with_get_agent_info(|name| async move {
            let cwd = std::env::current_dir().unwrap_or_default();
            let registry = crate::AgentRegistry::from_project_dir(&cwd);
            Ok(registry.get(&name).map(|info| rocode_tool::TaskAgentInfo {
                name: info.name.clone(),
                model: info.model.as_ref().map(|m| rocode_tool::TaskAgentModel {
                    provider_id: m.provider_id.clone(),
                    model_id: m.model_id.clone(),
                }),
                can_use_task: info.is_tool_allowed("task"),
            }))
        });

        ctx.with_create_subsession({
            let subsessions = subsessions.clone();
            move |agent_name, _title, model, disabled_tools| {
                let subsessions = subsessions.clone();
                async move {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    let registry = crate::AgentRegistry::from_project_dir(&cwd);
                    let mut agent = registry.get(&agent_name).cloned().ok_or_else(|| {
                        ToolError::InvalidArguments(format!(
                            "Unknown agent type: {} is not a valid agent type",
                            agent_name
                        ))
                    })?;

                    if let Some((provider_id, model_id)) = parse_model_string(model.as_deref()) {
                        agent = agent.with_model(model_id, provider_id);
                    }

                    let conversation = if let Some(system_prompt) = &agent.system_prompt {
                        Conversation::with_system_prompt(system_prompt.clone())
                    } else {
                        Conversation::new()
                    };

                    let session_id =
                        format!("task_{}_{}", agent_name, uuid::Uuid::new_v4().simple());
                    let mut store = subsessions.lock().await;
                    store.insert(
                        session_id.clone(),
                        SubsessionState {
                            agent,
                            conversation,
                            disabled_tools: disabled_tools.into_iter().collect(),
                        },
                    );
                    Ok(session_id)
                }
            }
        })
        .with_prompt_subsession({
            let subsessions = subsessions.clone();
            let providers = providers.clone();
            let tools = tools.clone();
            move |session_id, prompt| {
                let subsessions = subsessions.clone();
                let providers = providers.clone();
                let tools = tools.clone();
                async move {
                    let state = {
                        let store = subsessions.lock().await;
                        store.get(&session_id).cloned()
                    }
                    .ok_or_else(|| {
                        ToolError::ExecutionError(format!(
                            "Unknown subagent session: {}. Start without task_id first.",
                            session_id
                        ))
                    })?;

                    let mut executor =
                        AgentExecutor::new(state.agent, providers.clone(), tools.clone())
                            .with_disabled_tools(state.disabled_tools.iter().cloned());
                    executor.conversation = state.conversation;

                    let output = executor.execute_subsession(prompt).await.map_err(|e| {
                        ToolError::ExecutionError(format!("Subagent execution failed: {}", e))
                    })?;

                    let mut store = subsessions.lock().await;
                    if let Some(state) = store.get_mut(&session_id) {
                        state.conversation = executor.conversation.clone();
                    }

                    Ok(output)
                }
            }
        })
    }

    fn current_model_string(&self) -> Option<String> {
        if let Some(model) = self.agent.model.as_ref() {
            return Some(format!("{}:{}", model.provider_id, model.model_id));
        }

        let provider = self.get_provider().ok()?;
        let model_id = self.get_model_id(&provider);
        if model_id.is_empty() {
            return None;
        }
        Some(format!("{}:{}", provider.id(), model_id))
    }

    async fn repair_tool_call(&self, tool_call: ToolCall) -> ToolCall {
        let available_tools = self.tools.list_ids().await;
        let repaired_name = repair_tool_call_name(&tool_call.name, &available_tools)
            .unwrap_or_else(|| tool_call.name.clone());

        if repaired_name == tool_call.name {
            return tool_call;
        }

        let arguments = if repaired_name == "invalid" && tool_call.name != "invalid" {
            serde_json::json!({
                "tool": tool_call.name.clone(),
                "error": format!("Unknown tool requested by model: {}", tool_call.name),
            })
        } else {
            tool_call.arguments
        };

        ToolCall {
            id: tool_call.id,
            name: repaired_name,
            arguments,
        }
    }

    async fn resolve_tool_definitions(&self) -> Vec<rocode_provider::ToolDefinition> {
        self.tools
            .list_schemas()
            .await
            .into_iter()
            .filter(|s| !self.disabled_tools.contains(&s.name))
            .map(|s| rocode_provider::ToolDefinition {
                name: s.name,
                description: Some(s.description),
                parameters: s.parameters,
            })
            .collect()
    }

    fn ensure_tool_allowed(&self, tool_name: &str) -> Result<(), ToolError> {
        match self.agent.tool_permission_decision(tool_name) {
            crate::PermissionDecision::Allow => Ok(()),
            crate::PermissionDecision::Ask => Err(ToolError::PermissionDenied(format!(
                "Tool '{}' requires explicit approval for agent '{}'",
                tool_name, self.agent.name
            ))),
            crate::PermissionDecision::Deny => Err(ToolError::PermissionDenied(format!(
                "Tool '{}' is denied by agent '{}' permission rules",
                tool_name, self.agent.name
            ))),
        }
    }
}

fn repair_tool_call_name(name: &str, available_tools: &[String]) -> Option<String> {
    if available_tools.iter().any(|tool| tool == name) {
        return None;
    }

    let lower = name.to_ascii_lowercase();
    if lower != name && available_tools.iter().any(|tool| tool == &lower) {
        return Some(lower);
    }

    if available_tools.iter().any(|tool| tool == "invalid") {
        return Some("invalid".to_string());
    }

    None
}

fn parse_model_string(raw: Option<&str>) -> Option<(String, String)> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }

    let (provider, model) = raw.split_once(':').or_else(|| raw.split_once('/'))?;

    if provider.is_empty() || model.is_empty() {
        return None;
    }

    Some((provider.to_string(), model.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rocode_permission::{PermissionAction, PermissionRule};

    fn build_executor(agent: AgentInfo) -> AgentExecutor {
        AgentExecutor::new(
            agent,
            Arc::new(ProviderRegistry::new()),
            Arc::new(ToolRegistry::new()),
        )
    }

    #[tokio::test]
    async fn persisted_subsessions_roundtrip() {
        let mut conversation = Conversation::with_system_prompt("subagent prompt");
        conversation.add_user_message("inspect project");
        conversation.add_assistant_message("working on it");

        let mut persisted = HashMap::new();
        persisted.insert(
            "task_explore_1".to_string(),
            PersistedSubsessionState {
                agent: AgentInfo::explore().with_model("gpt-4.1-mini", "openai"),
                conversation: conversation.clone(),
                disabled_tools: vec!["write".to_string(), "edit".to_string()],
            },
        );

        let executor = build_executor(AgentInfo::general()).with_persisted_subsessions(persisted);
        let exported = executor.export_subsessions().await;
        let state = exported
            .get("task_explore_1")
            .expect("expected persisted subsession");

        assert_eq!(state.agent.name, "explore");
        assert_eq!(
            state.conversation.messages.len(),
            conversation.messages.len()
        );

        let mut disabled = state.disabled_tools.clone();
        disabled.sort();
        assert_eq!(disabled, vec!["edit".to_string(), "write".to_string()]);
    }

    #[test]
    fn executor_enforces_explore_allowlist() {
        let executor = build_executor(AgentInfo::explore());

        assert!(executor.ensure_tool_allowed("grep").is_ok());

        let denied = executor
            .ensure_tool_allowed("write")
            .expect_err("write should be denied for explore");
        assert!(
            matches!(denied, ToolError::PermissionDenied(_)),
            "expected permission denied, got: {denied}"
        );
    }

    #[test]
    fn executor_blocks_ask_permissions_without_user_approval() {
        let agent = AgentInfo::custom("review").with_permission(vec![PermissionRule {
            permission: "bash".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Ask,
        }]);
        let executor = build_executor(agent);

        let denied = executor
            .ensure_tool_allowed("bash")
            .expect_err("ask should block direct execution");
        assert!(
            matches!(denied, ToolError::PermissionDenied(_)),
            "expected permission denied, got: {denied}"
        );
    }

    #[test]
    fn repair_tool_call_name_fixes_case_when_lower_tool_exists() {
        let available = vec!["read".to_string(), "invalid".to_string()];
        let repaired = repair_tool_call_name("Read", &available);
        assert_eq!(repaired.as_deref(), Some("read"));
    }

    #[test]
    fn repair_tool_call_name_falls_back_to_invalid_tool() {
        let available = vec!["read".to_string(), "invalid".to_string()];
        let repaired = repair_tool_call_name("missing_tool", &available);
        assert_eq!(repaired.as_deref(), Some("invalid"));
    }

    /// Build a mock stream from a sequence of StreamEvents.
    fn mock_stream(events: Vec<StreamEvent>) -> rocode_provider::StreamResult {
        let stream = futures::stream::iter(
            events
                .into_iter()
                .map(|e| Ok::<_, rocode_provider::ProviderError>(e)),
        );
        Box::pin(stream)
    }

    #[tokio::test]
    async fn process_stream_uses_tool_call_end_as_authoritative() {
        let mut executor = build_executor(AgentInfo::general());
        let stream = mock_stream(vec![
            StreamEvent::ToolCallStart {
                id: "tool-call-0".into(),
                name: "read".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "tool-call-0".into(),
                input: r#"{"partial":true"#.into(),
            },
            StreamEvent::ToolCallEnd {
                id: "tool-call-0".into(),
                name: "read".into(),
                input: serde_json::json!({"file_path": "/tmp/test"}),
            },
            StreamEvent::Done,
        ]);

        let (_, tool_calls) = executor.process_stream(stream).await.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "read");
        assert_eq!(
            tool_calls[0].arguments,
            serde_json::json!({"file_path": "/tmp/test"})
        );
    }

    #[tokio::test]
    async fn process_stream_ignores_partial_tool_call_without_end() {
        let mut executor = build_executor(AgentInfo::general());
        let stream = mock_stream(vec![
            StreamEvent::ToolCallStart {
                id: "tool-call-0".into(),
                name: "bash".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "tool-call-0".into(),
                input: r#"{"command":"ls"}"#.into(),
            },
        ]);

        let (_, tool_calls) = executor.process_stream(stream).await.unwrap();
        assert!(tool_calls.is_empty());
    }

    #[tokio::test]
    async fn process_stream_handles_multiple_tool_call_end_events() {
        let mut executor = build_executor(AgentInfo::general());
        let stream = mock_stream(vec![
            StreamEvent::ToolCallEnd {
                id: "tool-call-0".into(),
                name: "read".into(),
                input: serde_json::json!({"file_path": "/tmp/a"}),
            },
            StreamEvent::ToolCallEnd {
                id: "tool-call-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
            StreamEvent::Done,
        ]);

        let (_, tool_calls) = executor.process_stream(stream).await.unwrap();
        assert_eq!(tool_calls.len(), 2);

        let read_tc = tool_calls.iter().find(|t| t.name == "read").unwrap();
        assert_eq!(
            read_tc.arguments,
            serde_json::json!({"file_path": "/tmp/a"})
        );

        let bash_tc = tool_calls.iter().find(|t| t.name == "bash").unwrap();
        assert_eq!(bash_tc.arguments, serde_json::json!({"command": "ls"}));
    }

    #[tokio::test]
    async fn process_stream_ignores_tool_call_end_with_empty_name() {
        let mut executor = build_executor(AgentInfo::general());
        let stream = mock_stream(vec![
            StreamEvent::ToolCallEnd {
                id: "tool-call-empty".into(),
                name: "   ".into(),
                input: serde_json::json!({"command": "ls"}),
            },
            StreamEvent::ToolCallEnd {
                id: "tool-call-1".into(),
                name: "ls".into(),
                input: serde_json::json!({"path": "."}),
            },
            StreamEvent::Done,
        ]);

        let (_, tool_calls) = executor.process_stream(stream).await.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "tool-call-1");
        assert_eq!(tool_calls[0].name, "ls");
        assert_eq!(tool_calls[0].arguments, serde_json::json!({"path": "."}));
    }
}
