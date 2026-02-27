pub mod compaction_helpers;
mod file_parts;
pub(crate) mod hooks;
mod message_building;
pub mod shell;
pub mod subtask;
mod tool_calls;
mod tool_execution;
pub mod tools_and_output;

pub use compaction_helpers::{should_compact, trigger_compaction};
pub(crate) use hooks::{
    apply_chat_message_hook_outputs, apply_chat_messages_hook_outputs, session_message_hook_payload,
};
#[cfg(test)]
pub(crate) use shell::resolve_shell_invocation;
pub use shell::{resolve_command_template, shell_exec, CommandInput, ShellInput};
pub use subtask::{tool_definitions_from_schemas, SubtaskExecutor, ToolSchema};
pub use tools_and_output::{
    create_structured_output_tool, extract_structured_output, generate_session_title,
    generate_session_title_llm, insert_reminders, max_steps_for_agent, merge_tool_definitions,
    resolve_tools, resolve_tools_with_mcp, resolve_tools_with_mcp_registry,
    structured_output_system_prompt, was_plan_agent, ResolvedTool, StructuredOutputConfig,
};

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use futures::StreamExt;
use rocode_plugin::{HookContext, HookEvent};
use rocode_provider::transform::{apply_caching, ProviderType};
use rocode_provider::{ChatRequest, Provider, StreamEvent, ToolDefinition};

use crate::compaction::{run_compaction, CompactionResult};
use crate::message_v2::ModelRef as V2ModelRef;
use crate::{MessageRole, PartType, Session, SessionMessage, SessionStateManager};

const MAX_STEPS: u32 = 100;

#[derive(Debug, Clone)]
pub struct PromptInput {
    pub session_id: String,
    pub message_id: Option<String>,
    pub model: Option<ModelRef>,
    pub agent: Option<String>,
    pub no_reply: bool,
    pub system: Option<String>,
    pub variant: Option<String>,
    pub parts: Vec<PartInput>,
    pub tools: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone)]
pub struct ModelRef {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PartInput {
    Text {
        text: String,
    },
    File {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
    },
    Agent {
        name: String,
    },
    Subtask {
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        agent: String,
    },
}

impl TryFrom<serde_json::Value> for PartInput {
    type Error = String;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|e| format!("Invalid PartInput: {}", e))
    }
}

impl PartInput {
    /// Parse a JSON array of parts into a Vec<PartInput>, skipping invalid entries.
    pub fn parse_array(value: &serde_json::Value) -> Vec<PartInput> {
        match value.as_array() {
            Some(arr) => arr
                .iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect(),
            None => Vec::new(),
        }
    }
}

struct PromptState {
    cancel_token: CancellationToken,
}

#[derive(Debug, Clone)]
struct PendingSubtask {
    part_index: usize,
    subtask_id: String,
    agent: String,
    prompt: String,
    description: String,
}

#[derive(Debug, Clone)]
struct StreamToolState {
    name: String,
    raw_input: String,
    input: serde_json::Value,
    status: crate::ToolCallStatus,
    resolved_by_provider: bool,
    state: crate::ToolState,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub(super) struct PersistedSubsession {
    agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory: Option<String>,
    #[serde(default)]
    disabled_tools: Vec<String>,
    #[serde(default)]
    history: Vec<PersistedSubsessionTurn>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct PersistedSubsessionTurn {
    prompt: String,
    output: String,
}

/// LLM parameters derived from agent configuration.
#[derive(Debug, Clone, Default)]
pub struct AgentParams {
    pub max_tokens: Option<u64>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
}

pub type SessionUpdateHook = Arc<dyn Fn(&Session) + Send + Sync + 'static>;
pub type AskQuestionHook = Arc<
    dyn Fn(
            String,
            Vec<rocode_tool::QuestionDef>,
        )
            -> Pin<Box<dyn Future<Output = Result<Vec<Vec<String>>, rocode_tool::ToolError>> + Send>>
        + Send
        + Sync
        + 'static,
>;

pub struct SessionPrompt {
    state: Arc<Mutex<HashMap<String, PromptState>>>,
    session_state: Arc<RwLock<SessionStateManager>>,
    mcp_clients: Option<Arc<rocode_mcp::McpClientRegistry>>,
    lsp_registry: Option<Arc<rocode_lsp::LspClientRegistry>>,
}

impl SessionPrompt {
    fn text_from_prompt_parts(parts: &[PartInput]) -> String {
        parts
            .iter()
            .filter_map(|p| match p {
                PartInput::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn truncate_debug_text(value: &str, max_chars: usize) -> String {
        if value.chars().count() <= max_chars {
            return value.to_string();
        }
        let mut out = value.chars().take(max_chars).collect::<String>();
        out.push_str("...[truncated]");
        out
    }

    fn annotate_latest_user_message(
        session: &mut Session,
        input: &PromptInput,
        system_prompt: Option<&str>,
    ) {
        let Some(user_msg) = session
            .messages
            .iter_mut()
            .rfind(|m| matches!(m.role, MessageRole::User))
        else {
            return;
        };

        if let Some(agent) = input.agent.as_deref() {
            user_msg
                .metadata
                .insert("resolved_agent".to_string(), serde_json::json!(agent));
        }

        if let Some(system) = system_prompt {
            user_msg.metadata.insert(
                "resolved_system_prompt".to_string(),
                serde_json::json!(Self::truncate_debug_text(system, 8000)),
            );
            user_msg.metadata.insert(
                "resolved_system_prompt_applied".to_string(),
                serde_json::json!(true),
            );
        } else if input.agent.is_some() {
            user_msg.metadata.insert(
                "resolved_system_prompt_applied".to_string(),
                serde_json::json!(false),
            );
        }

        let user_prompt = Self::text_from_prompt_parts(&input.parts);
        if !user_prompt.is_empty() {
            user_msg.metadata.insert(
                "resolved_user_prompt".to_string(),
                serde_json::json!(Self::truncate_debug_text(&user_prompt, 8000)),
            );
        }
    }

    fn has_tool_result_after(session: &Session, message_index: usize, tool_call_id: &str) -> bool {
        session.messages.iter().skip(message_index + 1).any(|msg| {
            msg.parts.iter().any(|part| {
                matches!(
                    &part.part_type,
                    PartType::ToolResult { tool_call_id: id, .. } if id == tool_call_id
                )
            })
        })
    }

    pub fn new(session_state: Arc<RwLock<SessionStateManager>>) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            session_state,
            mcp_clients: None,
            lsp_registry: None,
        }
    }

    pub fn with_mcp_clients(mut self, clients: Arc<rocode_mcp::McpClientRegistry>) -> Self {
        self.mcp_clients = Some(clients);
        self
    }

    pub fn with_lsp_registry(mut self, registry: Arc<rocode_lsp::LspClientRegistry>) -> Self {
        self.lsp_registry = Some(registry);
        self
    }

    pub async fn assert_not_busy(&self, session_id: &str) -> anyhow::Result<()> {
        let state = self.state.lock().await;
        if state.contains_key(session_id) {
            return Err(anyhow::anyhow!("Session {} is busy", session_id));
        }
        Ok(())
    }

    pub async fn create_user_message(
        &self,
        input: &PromptInput,
        session: &mut Session,
    ) -> anyhow::Result<()> {
        // Collect text parts for the primary message
        let text = input
            .parts
            .iter()
            .filter_map(|p| match p {
                PartInput::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let has_non_text = input
            .parts
            .iter()
            .any(|p| !matches!(p, PartInput::Text { .. }));

        if text.is_empty() && !has_non_text {
            return Err(anyhow::anyhow!("No content in prompt"));
        }

        let project_root = session.directory.clone();

        // Create the user message with text (or empty if only non-text parts)
        let msg = if text.is_empty() {
            session.add_user_message(" ")
        } else {
            session.add_user_message(&text)
        };

        // Add non-text parts to the message
        for part in &input.parts {
            match part {
                PartInput::Text { .. } => {} // already handled above
                PartInput::File {
                    url,
                    filename,
                    mime,
                } => {
                    self.add_file_part(
                        msg,
                        url,
                        filename.as_deref(),
                        mime.as_deref(),
                        &project_root,
                    )
                    .await;
                }
                PartInput::Agent { name } => {
                    msg.add_agent(name.clone());
                    // Add synthetic text instructing the LLM to invoke the agent
                    msg.add_text(format!(
                        "Use the above message and context to generate a prompt and call the task tool with subagent: {}",
                        name
                    ));
                }
                PartInput::Subtask {
                    prompt,
                    description,
                    agent,
                } => {
                    let subtask_id = format!("sub_{}", uuid::Uuid::new_v4());
                    let description = description.clone().unwrap_or_else(|| prompt.clone());
                    msg.add_subtask(subtask_id.clone(), description.clone());
                    let mut pending = msg
                        .metadata
                        .get("pending_subtasks")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    pending.push(serde_json::json!({
                        "id": subtask_id,
                        "agent": agent,
                        "prompt": prompt,
                        "description": description,
                    }));
                    msg.metadata.insert(
                        "pending_subtasks".to_string(),
                        serde_json::Value::Array(pending),
                    );
                }
            }
        }

        Ok(())
    }

    // --- file_parts methods moved to file_parts.rs ---

    async fn start(&self, session_id: &str) -> Option<CancellationToken> {
        let state = self.state.lock().await;
        if state.contains_key(session_id) {
            return None;
        }
        drop(state);

        let token = CancellationToken::new();
        let mut state = self.state.lock().await;
        state.insert(
            session_id.to_string(),
            PromptState {
                cancel_token: token.clone(),
            },
        );
        Some(token)
    }

    async fn resume(&self, session_id: &str) -> Option<CancellationToken> {
        let state = self.state.lock().await;
        state.get(session_id).map(|s| s.cancel_token.clone())
    }

    pub async fn is_running(&self, session_id: &str) -> bool {
        let state = self.state.lock().await;
        state.contains_key(session_id)
    }

    async fn finish_run(&self, session_id: &str) {
        let mut state = self.state.lock().await;
        state.remove(session_id);
        drop(state);

        let mut session_state = self.session_state.write().await;
        session_state.set_idle(session_id);
    }

    pub async fn cancel(&self, session_id: &str) {
        let mut state = self.state.lock().await;
        if let Some(prompt_state) = state.remove(session_id) {
            prompt_state.cancel_token.cancel();
        }

        let mut session_state = self.session_state.write().await;
        session_state.set_idle(session_id);
    }

    pub async fn prompt(
        &self,
        input: PromptInput,
        session: &mut Session,
        provider: Arc<dyn Provider>,
        system_prompt: Option<String>,
        tools: Vec<ToolDefinition>,
        agent_params: AgentParams,
    ) -> anyhow::Result<()> {
        self.prompt_with_update_hook(
            input,
            session,
            provider,
            system_prompt,
            tools,
            agent_params,
            None,
            None,
            None,
        )
        .await
    }

    pub async fn prompt_with_update_hook(
        &self,
        input: PromptInput,
        session: &mut Session,
        provider: Arc<dyn Provider>,
        system_prompt: Option<String>,
        tools: Vec<ToolDefinition>,
        agent_params: AgentParams,
        update_hook: Option<SessionUpdateHook>,
        agent_lookup: Option<Arc<dyn Fn(&str) -> Option<rocode_tool::TaskAgentInfo> + Send + Sync>>,
        ask_question_hook: Option<AskQuestionHook>,
    ) -> anyhow::Result<()> {
        self.assert_not_busy(&input.session_id).await?;

        let cancel_token = self.start(&input.session_id).await;
        let token = match cancel_token {
            Some(t) => t,
            None => return Err(anyhow::anyhow!("Session already running")),
        };

        // Keep model/provider resolution aligned for both hook payloads and prompt loop.
        let model_id = input
            .model
            .as_ref()
            .map(|m| m.model_id.clone())
            .unwrap_or_else(|| "default".to_string());
        let provider_id = input
            .model
            .as_ref()
            .map(|m| m.provider_id.clone())
            .unwrap_or_else(|| "anthropic".to_string());

        self.create_user_message(&input, session).await?;
        Self::annotate_latest_user_message(session, &input, system_prompt.as_deref());

        session.touch();
        Self::emit_session_update(update_hook.as_ref(), session);

        if input.no_reply {
            self.finish_run(&input.session_id).await;
            return Ok(());
        }

        {
            let mut session_state = self.session_state.write().await;
            session_state.set_busy(&input.session_id);
        }

        let session_id = input.session_id.clone();

        let result = Self::loop_inner(
            session_id.clone(),
            token,
            provider,
            model_id,
            provider_id,
            session,
            input.agent.as_deref(),
            system_prompt,
            tools,
            &agent_params,
            update_hook,
            agent_lookup,
            ask_question_hook,
        )
        .await;

        self.finish_run(&session_id).await;

        if let Err(e) = result {
            tracing::error!("Prompt loop error for session {}: {}", session_id, e);
            return Err(e);
        }

        Ok(())
    }

    pub async fn resume_session(
        &self,
        session_id: &str,
        session: &mut Session,
        provider: Arc<dyn Provider>,
        system_prompt: Option<String>,
        tools: Vec<ToolDefinition>,
        agent_params: AgentParams,
    ) -> anyhow::Result<()> {
        let token = self.resume(session_id).await;

        let token = match token {
            Some(t) => t,
            None => {
                return Err(anyhow::anyhow!(
                    "Session {} is not running, cannot resume",
                    session_id
                ));
            }
        };

        let model = session.messages.iter().rev().find_map(|m| match m.role {
            MessageRole::User => session
                .metadata
                .get("model_provider")
                .and_then(|p| p.as_str())
                .zip(session.metadata.get("model_id").and_then(|i| i.as_str()))
                .map(|(provider_id, model_id)| ModelRef {
                    provider_id: provider_id.to_string(),
                    model_id: model_id.to_string(),
                }),
            _ => None,
        });

        let model_id = model
            .as_ref()
            .map(|m| m.model_id.clone())
            .unwrap_or_else(|| "default".to_string());
        let provider_id = model
            .as_ref()
            .map(|m| m.provider_id.clone())
            .unwrap_or_else(|| "anthropic".to_string());

        let session_id = session_id.to_string();
        let resume_agent = session
            .metadata
            .get("agent")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        {
            let mut session_state = self.session_state.write().await;
            session_state.set_busy(&session_id);
        }

        let result = Self::loop_inner(
            session_id.clone(),
            token,
            provider,
            model_id,
            provider_id,
            session,
            resume_agent.as_deref(),
            system_prompt,
            tools,
            &agent_params,
            None,
            None,
            None,
        )
        .await;

        self.finish_run(&session_id).await;

        if let Err(e) = result {
            tracing::error!("Resume prompt loop error for session {}: {}", session_id, e);
            return Err(e);
        }

        Ok(())
    }

    async fn loop_inner(
        session_id: String,
        token: CancellationToken,
        provider: Arc<dyn Provider>,
        model_id: String,
        provider_id: String,
        session: &mut Session,
        agent_name: Option<&str>,
        system_prompt: Option<String>,
        tools: Vec<ToolDefinition>,
        agent_params: &AgentParams,
        update_hook: Option<SessionUpdateHook>,
        agent_lookup: Option<Arc<dyn Fn(&str) -> Option<rocode_tool::TaskAgentInfo> + Send + Sync>>,
        ask_question_hook: Option<AskQuestionHook>,
    ) -> anyhow::Result<()> {
        let mut step = 0u32;
        let provider_type = ProviderType::from_provider_id(&provider_id);
        let mut post_first_step_ran = false;

        loop {
            if token.is_cancelled() {
                tracing::info!("Prompt loop cancelled for session {}", session_id);
                break;
            }

            let mut filtered_messages = Self::filter_compacted_messages(&session.messages);

            let last_user_idx = filtered_messages
                .iter()
                .rposition(|m| matches!(m.role, MessageRole::User));

            let last_assistant_idx = filtered_messages
                .iter()
                .rposition(|m| matches!(m.role, MessageRole::Assistant));

            let last_user_idx = match last_user_idx {
                Some(idx) => idx,
                None => return Err(anyhow::anyhow!("No user message found")),
            };

            if Self::process_pending_subtasks(
                session,
                provider.clone(),
                &model_id,
                &provider_id,
                agent_lookup.clone(),
                ask_question_hook.clone(),
            )
            .await?
            {
                tracing::info!("Processed pending subtask parts for session {}", session_id);
                continue;
            }

            // Early exit: if the last assistant message has a terminal finish
            // reason (not "tool-calls"/"tool_calls"/"unknown") and it came after
            // the last user message, the conversation turn is complete.
            // Mirrors TS prompt.ts:318-325.
            // Uses index position instead of ID comparison because user IDs
            // (uuid v4) and assistant IDs (different generator) have no
            // guaranteed lexicographic ordering.
            if let Some(assistant_idx) = last_assistant_idx {
                let assistant = &filtered_messages[assistant_idx];
                let is_terminal = assistant
                    .finish
                    .as_deref()
                    .is_some_and(|f| !matches!(f, "tool-calls" | "tool_calls" | "unknown"));

                if is_terminal && last_user_idx < assistant_idx {
                    tracing::info!(
                        finish = ?assistant.finish,
                        "Prompt loop complete for session {}", session_id
                    );
                    break;
                }
            }

            step += 1;
            if step > MAX_STEPS {
                tracing::warn!("Max steps reached for session {}", session_id);
                break;
            }

            if Self::should_compact(
                &filtered_messages,
                provider.as_ref(),
                &model_id,
                agent_params.max_tokens,
            ) {
                tracing::info!(
                    "Context overflow detected, triggering compaction for session {}",
                    session_id
                );

                // Use LLM-driven compaction via CompactionEngine::process().
                // Build provider messages from the filtered session messages.
                let parent_id = filtered_messages
                    .last()
                    .map(|m| m.id.clone())
                    .unwrap_or_default();
                let compaction_messages =
                    Self::build_chat_messages(&filtered_messages, None).unwrap_or_default();
                let model_ref = V2ModelRef {
                    provider_id: provider_id.clone(),
                    model_id: model_id.clone(),
                };

                match run_compaction::<crate::compaction::NoopSessionOps>(
                    &session_id,
                    &parent_id,
                    compaction_messages,
                    model_ref,
                    provider.clone(),
                    CancellationToken::new(),
                    true, // auto-triggered
                    None,
                    None, // no SessionOps — we persist via Session directly below
                )
                .await
                {
                    Ok(CompactionResult::Continue) => {
                        tracing::info!(
                            "LLM compaction complete for session {}, continuing",
                            session_id
                        );
                    }
                    Ok(CompactionResult::Stop) => {
                        tracing::warn!("LLM compaction returned stop for session {}, falling back to simple compaction", session_id);
                        if let Some(summary) = Self::trigger_compaction(session, &filtered_messages)
                        {
                            tracing::info!("Fallback compaction (from stop) complete: {}", summary);
                        }
                    }
                    Err(e) => {
                        // Fallback to simple text truncation if LLM compaction fails.
                        tracing::warn!(
                            "LLM compaction failed for session {}: {}, falling back to simple compaction",
                            session_id,
                            e
                        );
                        if let Some(summary) = Self::trigger_compaction(session, &filtered_messages)
                        {
                            tracing::info!("Fallback compaction complete: {}", summary);
                        }
                    }
                }
            }

            tracing::info!(
                step = step,
                session_id = %session_id,
                message_count = filtered_messages.len(),
                "[plugin-seq] prompt loop step start"
            );

            tracing::info!("Prompt loop step {} for session {}", step, session_id);

            // Plugin hook: chat.messages.transform — let plugins modify messages before sending
            let hook_messages = serde_json::Value::Array(
                filtered_messages
                    .iter()
                    .map(session_message_hook_payload)
                    .collect(),
            );
            let message_hook_outputs = rocode_plugin::trigger_collect(
                HookContext::new(HookEvent::ChatMessagesTransform)
                    .with_session(&session_id)
                    .with_data("message_count", serde_json::json!(filtered_messages.len()))
                    .with_data("messages", hook_messages),
            )
            .await;
            apply_chat_messages_hook_outputs(&mut filtered_messages, message_hook_outputs);

            let mut prompt_messages = filtered_messages;
            if let Some(agent) = agent_name {
                let was_plan = was_plan_agent(&prompt_messages);
                prompt_messages = insert_reminders(&prompt_messages, agent, was_plan);
            }

            let mut chat_messages =
                Self::build_chat_messages(&prompt_messages, system_prompt.as_deref())?;

            apply_caching(&mut chat_messages, provider_type);
            let resolved_tools =
                merge_tool_definitions(tools.clone(), Self::mcp_tools_from_session(session));

            let request = ChatRequest {
                model: model_id.clone(),
                messages: chat_messages,
                max_tokens: Some(agent_params.max_tokens.unwrap_or(8192)),
                temperature: agent_params.temperature,
                system: None,
                tools: if resolved_tools.is_empty() {
                    None
                } else {
                    Some(resolved_tools.clone())
                },
                stream: Some(true),
                top_p: agent_params.top_p,
                variant: None,
                provider_options: None,
            };

            // Stream the response (matching TS streamText approach).
            let mut stream = match provider.chat_stream(request).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Provider error for session {}: {}", session_id, e);
                    return Err(anyhow::anyhow!("{}", e));
                }
            };

            // Create assistant message placeholder before consuming the stream so
            // callers can observe incremental output updates.
            let assistant_index = session.messages.len();
            let assistant_message_id =
                rocode_core::id::create(rocode_core::id::Prefix::Message, true, None);
            let mut assistant_metadata = HashMap::new();
            assistant_metadata.insert(
                "model_provider".to_string(),
                serde_json::json!(&provider_id),
            );
            assistant_metadata.insert("model_id".to_string(), serde_json::json!(&model_id));
            if let Some(agent) = agent_name {
                assistant_metadata.insert("agent".to_string(), serde_json::json!(agent));
                assistant_metadata.insert("mode".to_string(), serde_json::json!(agent));
            }
            session.messages.push(SessionMessage {
                id: assistant_message_id,
                session_id: session_id.clone(),
                role: MessageRole::Assistant,
                parts: Vec::new(),
                created_at: chrono::Utc::now(),
                metadata: assistant_metadata,
                usage: None,
                finish: None,
            });
            session.touch();
            Self::emit_session_update(update_hook.as_ref(), session);

            // Consume stream events to build the assistant message incrementally.
            let tool_registry = Arc::new(rocode_tool::create_default_registry().await);
            let mut tool_calls: HashMap<String, StreamToolState> = HashMap::new();
            let mut stream_tool_results: Vec<(
                String,
                String,
                bool,
                Option<String>,
                Option<HashMap<String, serde_json::Value>>,
                Option<Vec<serde_json::Value>>,
            )> = Vec::new();
            let mut finish_reason: Option<String> = None;
            let mut prompt_tokens: u64 = 0;
            let mut completion_tokens: u64 = 0;
            let mut reasoning_tokens: u64 = 0;
            let mut cache_read_tokens: u64 = 0;
            let mut cache_write_tokens: u64 = 0;
            let mut executed_local_tools_this_step = false;
            let mut last_emit = Instant::now() - Duration::from_millis(50);

            while let Some(event_result) = stream.next().await {
                if token.is_cancelled() {
                    tracing::info!("Stream cancelled for session {}", session_id);
                    break;
                }
                match event_result {
                    Ok(StreamEvent::TextDelta(text)) => {
                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            Self::append_delta_part(assistant, false, &text);
                        }
                        session.touch();
                        Self::maybe_emit_session_update(
                            update_hook.as_ref(),
                            session,
                            &mut last_emit,
                            false,
                        );
                    }
                    Ok(StreamEvent::TextStart) | Ok(StreamEvent::TextEnd) => {}
                    Ok(StreamEvent::ReasoningStart { .. }) => {}
                    Ok(StreamEvent::ReasoningDelta { text, .. }) => {
                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            Self::append_delta_part(assistant, true, &text);
                        }
                        session.touch();
                        Self::maybe_emit_session_update(
                            update_hook.as_ref(),
                            session,
                            &mut last_emit,
                            false,
                        );
                    }
                    Ok(StreamEvent::ReasoningEnd { .. }) => {}
                    Ok(StreamEvent::ToolCallStart { id, name }) => {
                        tracing::info!(
                            tool_call_id = %id,
                            tool_name = %name,
                            "[DIAG] ToolCallStart received"
                        );
                        if name.trim().is_empty() {
                            tracing::warn!(tool_call_id = %id, "ignoring ToolCallStart with empty tool name");
                            continue;
                        }
                        let entry =
                            tool_calls
                                .entry(id.clone())
                                .or_insert_with(|| StreamToolState {
                                    name: String::new(),
                                    raw_input: String::new(),
                                    input: serde_json::json!({}),
                                    status: crate::ToolCallStatus::Pending,
                                    resolved_by_provider: false,
                                    state: crate::ToolState::Pending {
                                        input: serde_json::json!({}),
                                        raw: String::new(),
                                    },
                                });
                        if entry.name.is_empty() {
                            entry.name = name.clone();
                        }
                        entry.status = crate::ToolCallStatus::Pending;
                        entry.state = crate::ToolState::Pending {
                            input: entry.input.clone(),
                            raw: entry.raw_input.clone(),
                        };

                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            Self::upsert_tool_call_part(
                                assistant,
                                &id,
                                Some(&name),
                                Some(entry.input.clone()),
                                Some(entry.raw_input.clone()),
                                Some(crate::ToolCallStatus::Pending),
                                Some(entry.state.clone()),
                            );
                        }
                    }
                    Ok(StreamEvent::ToolCallDelta { id, input }) => {
                        tracing::info!(
                            tool_call_id = %id,
                            delta_len = input.len(),
                            delta_preview = %input.chars().take(200).collect::<String>(),
                            "[DIAG] ToolCallDelta received"
                        );
                        let entry =
                            tool_calls
                                .entry(id.clone())
                                .or_insert_with(|| StreamToolState {
                                    name: String::new(),
                                    raw_input: String::new(),
                                    input: serde_json::json!({}),
                                    status: crate::ToolCallStatus::Pending,
                                    resolved_by_provider: false,
                                    state: crate::ToolState::Pending {
                                        input: serde_json::json!({}),
                                        raw: String::new(),
                                    },
                                });
                        entry.raw_input.push_str(&input);
                        if rocode_provider::is_parsable_json(&entry.raw_input) {
                            if let Ok(parsed) = serde_json::from_str(&entry.raw_input) {
                                entry.input = parsed;
                            }
                        }
                        entry.state = crate::ToolState::Pending {
                            input: entry.input.clone(),
                            raw: entry.raw_input.clone(),
                        };
                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            Self::upsert_tool_call_part(
                                assistant,
                                &id,
                                None,
                                Some(entry.input.clone()),
                                Some(entry.raw_input.clone()),
                                Some(crate::ToolCallStatus::Pending),
                                Some(entry.state.clone()),
                            );
                        }
                    }
                    Ok(StreamEvent::ToolCallEnd { id, name, input }) => {
                        if name.trim().is_empty() {
                            tracing::warn!(tool_call_id = %id, "ignoring ToolCallEnd with empty tool name");
                            continue;
                        }
                        // Avoid double-encoding: if the provider returned
                        // Value::String (e.g. flush path for incomplete JSON),
                        // use the string content directly as raw_input and try
                        // to parse it into an object for entry.input.
                        let (raw_input, parsed_input) = match &input {
                            serde_json::Value::String(s) => {
                                let parsed = Self::parse_json_or_string(s);
                                (s.clone(), parsed)
                            }
                            other => (
                                serde_json::to_string(other).unwrap_or_default(),
                                other.clone(),
                            ),
                        };
                        tracing::info!(
                            tool_call_id = %id,
                            tool_name = %name,
                            input_type = %if parsed_input.is_object() { "object" } else if parsed_input.is_string() { "string" } else { "other" },
                            raw_input_len = raw_input.len(),
                            raw_input_preview = %raw_input.chars().take(200).collect::<String>(),
                                "[DIAG] ToolCallEnd received"
                        );
                        let parsed_input_for_part = parsed_input.clone();
                        let entry =
                            tool_calls
                                .entry(id.clone())
                                .or_insert_with(|| StreamToolState {
                                    name: String::new(),
                                    raw_input: String::new(),
                                    input: serde_json::json!({}),
                                    status: crate::ToolCallStatus::Pending,
                                    resolved_by_provider: false,
                                    state: crate::ToolState::Pending {
                                        input: serde_json::json!({}),
                                        raw: String::new(),
                                    },
                                });
                        entry.name = name.clone();
                        entry.raw_input = raw_input.clone();
                        entry.input = parsed_input;
                        entry.status = crate::ToolCallStatus::Running;
                        entry.state = crate::ToolState::Running {
                            input: entry.input.clone(),
                            title: None,
                            metadata: None,
                            time: crate::RunningTime {
                                start: chrono::Utc::now().timestamp_millis(),
                            },
                        };

                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            Self::upsert_tool_call_part(
                                assistant,
                                &id,
                                Some(&name),
                                Some(parsed_input_for_part),
                                Some(raw_input),
                                Some(crate::ToolCallStatus::Running),
                                Some(entry.state.clone()),
                            );
                        }
                        let tool_context = rocode_tool::ToolContext::new(
                            session_id.clone(),
                            session.messages[assistant_index].id.clone(),
                            session.directory.clone(),
                        )
                        .with_agent(String::new())
                        .with_abort(token.clone());
                        match Self::execute_tool_calls_with_hook(
                            session,
                            tool_registry.clone(),
                            tool_context,
                            provider.clone(),
                            &provider_id,
                            &model_id,
                            update_hook.as_ref(),
                            agent_lookup.clone(),
                            ask_question_hook.clone(),
                        )
                        .await
                        {
                            Ok(executed) => {
                                if executed > 0 {
                                    executed_local_tools_this_step = true;
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Immediate tool execution error for session {}: {}",
                                    session_id,
                                    e
                                );
                            }
                        }
                        session.touch();
                        Self::maybe_emit_session_update(
                            update_hook.as_ref(),
                            session,
                            &mut last_emit,
                            true,
                        );
                    }
                    Ok(StreamEvent::ToolInputStart { id, tool_name }) => {
                        if tool_name.trim().is_empty() {
                            tracing::warn!(tool_call_id = %id, "ignoring ToolInputStart with empty tool name");
                            continue;
                        }
                        let entry =
                            tool_calls
                                .entry(id.clone())
                                .or_insert_with(|| StreamToolState {
                                    name: String::new(),
                                    raw_input: String::new(),
                                    input: serde_json::json!({}),
                                    status: crate::ToolCallStatus::Pending,
                                    resolved_by_provider: false,
                                    state: crate::ToolState::Pending {
                                        input: serde_json::json!({}),
                                        raw: String::new(),
                                    },
                                });
                        if entry.name.is_empty() {
                            entry.name = tool_name.clone();
                        }
                        entry.status = crate::ToolCallStatus::Pending;
                        entry.state = crate::ToolState::Pending {
                            input: entry.input.clone(),
                            raw: entry.raw_input.clone(),
                        };
                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            Self::upsert_tool_call_part(
                                assistant,
                                &id,
                                Some(&tool_name),
                                Some(entry.input.clone()),
                                Some(entry.raw_input.clone()),
                                Some(crate::ToolCallStatus::Pending),
                                Some(entry.state.clone()),
                            );
                        }
                    }
                    Ok(StreamEvent::ToolInputDelta { id, delta }) => {
                        let entry =
                            tool_calls
                                .entry(id.clone())
                                .or_insert_with(|| StreamToolState {
                                    name: String::new(),
                                    raw_input: String::new(),
                                    input: serde_json::json!({}),
                                    status: crate::ToolCallStatus::Pending,
                                    resolved_by_provider: false,
                                    state: crate::ToolState::Pending {
                                        input: serde_json::json!({}),
                                        raw: String::new(),
                                    },
                                });
                        entry.raw_input.push_str(&delta);
                        // Update the PartType's raw field so the accumulated input
                        // is available for execution even if ToolInputEnd never fires.
                        entry.state = crate::ToolState::Pending {
                            input: entry.input.clone(),
                            raw: entry.raw_input.clone(),
                        };
                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            Self::upsert_tool_call_part(
                                assistant,
                                &id,
                                None,
                                None,
                                Some(entry.raw_input.clone()),
                                None,
                                Some(entry.state.clone()),
                            );
                        }
                    }
                    Ok(StreamEvent::ToolInputEnd { id }) => {
                        if let Some(entry) = tool_calls.get_mut(&id) {
                            if entry.name.trim().is_empty() {
                                tracing::warn!(tool_call_id = %id, "ignoring ToolInputEnd for pending tool call with empty tool name");
                                continue;
                            }
                            if !entry.raw_input.is_empty() {
                                entry.input = serde_json::from_str(&entry.raw_input)
                                    .unwrap_or_else(|_| {
                                        serde_json::Value::String(entry.raw_input.clone())
                                    });
                            }
                            entry.status = crate::ToolCallStatus::Running;
                            entry.state = crate::ToolState::Running {
                                input: entry.input.clone(),
                                title: None,
                                metadata: None,
                                time: crate::RunningTime {
                                    start: chrono::Utc::now().timestamp_millis(),
                                },
                            };
                            if let Some(assistant) = session.messages.get_mut(assistant_index) {
                                Self::upsert_tool_call_part(
                                    assistant,
                                    &id,
                                    Some(&entry.name),
                                    Some(entry.input.clone()),
                                    Some(entry.raw_input.clone()),
                                    Some(crate::ToolCallStatus::Running),
                                    Some(entry.state.clone()),
                                );
                            }
                            let tool_context = rocode_tool::ToolContext::new(
                                session_id.clone(),
                                session.messages[assistant_index].id.clone(),
                                session.directory.clone(),
                            )
                            .with_agent(String::new())
                            .with_abort(token.clone());
                            match Self::execute_tool_calls_with_hook(
                                session,
                                tool_registry.clone(),
                                tool_context,
                                provider.clone(),
                                &provider_id,
                                &model_id,
                                update_hook.as_ref(),
                                agent_lookup.clone(),
                                ask_question_hook.clone(),
                            )
                            .await
                            {
                                Ok(executed) => {
                                    if executed > 0 {
                                        executed_local_tools_this_step = true;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Immediate tool execution error for session {}: {}",
                                        session_id,
                                        e
                                    );
                                }
                            }
                            session.touch();
                            Self::maybe_emit_session_update(
                                update_hook.as_ref(),
                                session,
                                &mut last_emit,
                                true,
                            );
                        }
                    }
                    Ok(StreamEvent::FinishStep {
                        finish_reason: fr,
                        usage,
                        ..
                    }) => {
                        tracing::info!(
                            finish_reason = %fr.as_deref().unwrap_or("None"),
                            tool_calls_count = tool_calls.len(),
                            tool_calls_keys = %tool_calls.keys().cloned().collect::<Vec<_>>().join(","),
                            "[DIAG] FinishStep received"
                        );
                        finish_reason = fr.clone();
                        // Persist finish reason on the assistant message itself,
                        // mirroring TS processor.ts:250. The prompt loop's
                        // early-exit check reads this field to decide whether
                        // to continue (tool-calls) or break (stop/end_turn).
                        if let Some(assistant) = session.messages.get_mut(assistant_index) {
                            assistant.finish = fr;
                        }
                        prompt_tokens = usage.prompt_tokens;
                        completion_tokens = usage.completion_tokens;
                        reasoning_tokens = usage.reasoning_tokens;
                        cache_read_tokens = usage.cache_read_tokens;
                        cache_write_tokens = usage.cache_write_tokens;
                    }
                    Ok(StreamEvent::Usage {
                        prompt_tokens: pt,
                        completion_tokens: ct,
                    }) => {
                        prompt_tokens = pt;
                        completion_tokens = ct;
                    }
                    Ok(StreamEvent::Done | StreamEvent::Finish) => break,
                    Ok(StreamEvent::Start) => {}
                    Ok(StreamEvent::Error(msg)) => {
                        tracing::error!("Stream error for session {}: {}", session_id, msg);
                        return Err(anyhow::anyhow!("Provider error: {}", msg));
                    }
                    Ok(StreamEvent::ToolResult {
                        tool_call_id,
                        input,
                        output,
                        ..
                    }) => {
                        if Self::has_tool_result_after(session, assistant_index, &tool_call_id) {
                            continue;
                        }
                        let assistant_message_id = session
                            .messages
                            .get(assistant_index)
                            .map(|m| m.id.clone())
                            .unwrap_or_default();
                        let (attachments, state_attachments) = Self::normalize_tool_attachments(
                            output.attachments.clone(),
                            &session_id,
                            &assistant_message_id,
                        );
                        if let Some(entry) = tool_calls.get_mut(&tool_call_id) {
                            if let Some(next_input) = input.clone() {
                                entry.input = next_input;
                            }
                            entry.status = crate::ToolCallStatus::Completed;
                            entry.resolved_by_provider = true;
                            let start = match &entry.state {
                                crate::ToolState::Running { time, .. } => time.start,
                                _ => chrono::Utc::now().timestamp_millis(),
                            };
                            entry.state = crate::ToolState::Completed {
                                input: entry.input.clone(),
                                output: output.output.clone(),
                                title: output.title.clone(),
                                metadata: output.metadata.clone(),
                                time: crate::CompletedTime {
                                    start,
                                    end: chrono::Utc::now().timestamp_millis(),
                                    compacted: None,
                                },
                                attachments: state_attachments.clone(),
                            };
                            if let Some(assistant) = session.messages.get_mut(assistant_index) {
                                Self::upsert_tool_call_part(
                                    assistant,
                                    &tool_call_id,
                                    Some(&entry.name),
                                    Some(entry.input.clone()),
                                    Some(entry.raw_input.clone()),
                                    Some(crate::ToolCallStatus::Completed),
                                    Some(entry.state.clone()),
                                );
                            }
                        }
                        stream_tool_results.push((
                            tool_call_id,
                            output.output,
                            false,
                            Some(output.title),
                            Some(output.metadata),
                            attachments,
                        ));
                    }
                    Ok(StreamEvent::ToolError {
                        tool_call_id,
                        input,
                        error,
                        kind,
                        ..
                    }) => {
                        if Self::has_tool_result_after(session, assistant_index, &tool_call_id) {
                            continue;
                        }
                        if let Some(entry) = tool_calls.get_mut(&tool_call_id) {
                            if let Some(next_input) = input.clone() {
                                entry.input = next_input;
                            }
                            entry.status = crate::ToolCallStatus::Error;
                            entry.resolved_by_provider = true;
                            let start = match &entry.state {
                                crate::ToolState::Running { time, .. } => time.start,
                                _ => chrono::Utc::now().timestamp_millis(),
                            };
                            entry.state = crate::ToolState::Error {
                                input: entry.input.clone(),
                                error: error.clone(),
                                metadata: None,
                                time: crate::ErrorTime {
                                    start,
                                    end: chrono::Utc::now().timestamp_millis(),
                                },
                            };
                            if let Some(assistant) = session.messages.get_mut(assistant_index) {
                                Self::upsert_tool_call_part(
                                    assistant,
                                    &tool_call_id,
                                    Some(&entry.name),
                                    Some(entry.input.clone()),
                                    Some(entry.raw_input.clone()),
                                    Some(crate::ToolCallStatus::Error),
                                    Some(entry.state.clone()),
                                );
                            }
                        }
                        let metadata = kind.map(|k| {
                            HashMap::from([(
                                "kind".to_string(),
                                serde_json::to_value(k).unwrap_or(serde_json::Value::Null),
                            )])
                        });
                        stream_tool_results.push((
                            tool_call_id,
                            error,
                            true,
                            Some("Tool Error".to_string()),
                            metadata,
                            None,
                        ));
                    }
                    Ok(StreamEvent::StartStep) => {}
                    Err(e) => {
                        tracing::error!("Stream error for session {}: {}", session_id, e);
                        return Err(anyhow::anyhow!("{}", e));
                    }
                }
            }

            // Finalize the placeholder assistant message with usage metadata.
            if let Some(assistant_msg) = session.messages.get_mut(assistant_index) {
                if let Some(reason) = finish_reason.clone() {
                    assistant_msg
                        .metadata
                        .insert("finish_reason".to_string(), serde_json::json!(reason));
                }
                assistant_msg.metadata.insert(
                    "completed_at".to_string(),
                    serde_json::json!(chrono::Utc::now().timestamp_millis()),
                );
                assistant_msg.metadata.insert(
                    "usage".to_string(),
                    serde_json::json!({
                        "prompt_tokens": prompt_tokens,
                        "completion_tokens": completion_tokens,
                        "reasoning_tokens": reasoning_tokens,
                        "cache_read_tokens": cache_read_tokens,
                        "cache_write_tokens": cache_write_tokens,
                    }),
                );
                assistant_msg
                    .metadata
                    .insert("tokens_input".to_string(), serde_json::json!(prompt_tokens));
                assistant_msg.metadata.insert(
                    "tokens_output".to_string(),
                    serde_json::json!(completion_tokens),
                );
                assistant_msg.metadata.insert(
                    "tokens_reasoning".to_string(),
                    serde_json::json!(reasoning_tokens),
                );
                assistant_msg.metadata.insert(
                    "tokens_cache_read".to_string(),
                    serde_json::json!(cache_read_tokens),
                );
                assistant_msg.metadata.insert(
                    "tokens_cache_write".to_string(),
                    serde_json::json!(cache_write_tokens),
                );
                assistant_msg.usage = Some(crate::message::MessageUsage {
                    input_tokens: prompt_tokens,
                    output_tokens: completion_tokens,
                    reasoning_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    ..Default::default()
                });
            }

            if !stream_tool_results.is_empty() {
                let mut tool_msg = SessionMessage::tool(session_id.clone());
                for (tool_call_id, content, is_error, title, metadata, attachments) in
                    stream_tool_results
                {
                    Self::push_tool_result_part(
                        &mut tool_msg,
                        tool_call_id,
                        content,
                        is_error,
                        title,
                        metadata,
                        attachments,
                    );
                }
                session.messages.push(tool_msg);
            }

            // Promote any tool calls still in Pending state to Running.
            // Some providers (e.g. LiteLLM, non-streaming) never emit ToolCallEnd,
            // so tool calls remain Pending after the stream ends. Without this
            // promotion, `tool_call_input_for_execution` returns None and the
            // tool never executes.
            if let Some(assistant_msg) = session.messages.get_mut(assistant_index) {
                for part in &mut assistant_msg.parts {
                    if let PartType::ToolCall {
                        id,
                        name,
                        input,
                        raw,
                        status,
                        state,
                    } = &mut part.part_type
                    {
                        if matches!(status, crate::ToolCallStatus::Pending)
                            && !name.trim().is_empty()
                        {
                            // Use accumulated raw input from the stream's tool_calls
                            // HashMap, which may have data from ToolInputDelta events
                            // that wasn't written to the PartType.
                            if let Some(entry) = tool_calls.get(id.as_str()) {
                                tracing::info!(
                                    tool_call_id = %id,
                                    tool_name = %name,
                                    entry_raw_input_len = entry.raw_input.len(),
                                    entry_raw_input_preview = %entry.raw_input.chars().take(200).collect::<String>(),
                                    entry_input_type = %if entry.input.is_object() { "object" } else if entry.input.is_string() { "string" } else { "other" },
                                    "[DIAG] Pending→Running promotion: found entry in tool_calls HashMap"
                                );
                                if !entry.raw_input.is_empty() {
                                    *raw = Some(entry.raw_input.clone());
                                    // Also try to parse the raw input into a proper object
                                    if let Ok(parsed) =
                                        serde_json::from_str::<serde_json::Value>(&entry.raw_input)
                                    {
                                        *input = parsed;
                                    }
                                }
                                if !entry.input.is_null() && entry.input != serde_json::json!({}) {
                                    *input = entry.input.clone();
                                }
                            } else {
                                tracing::info!(
                                    tool_call_id = %id,
                                    tool_name = %name,
                                    hashmap_keys = %tool_calls.keys().cloned().collect::<Vec<_>>().join(","),
                                    "[DIAG] Pending→Running promotion: entry NOT found in tool_calls HashMap"
                                );
                            }
                            *status = crate::ToolCallStatus::Running;
                            *state = Some(crate::ToolState::Running {
                                input: input.clone(),
                                title: None,
                                metadata: None,
                                time: crate::RunningTime {
                                    start: chrono::Utc::now().timestamp_millis(),
                                },
                            });
                            tracing::info!(
                                tool_call_id = %id,
                                tool_name = %name,
                                input_keys = %if input.is_object() {
                                    input.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default()
                                } else {
                                    format!("non-object: {}", input.to_string().chars().take(100).collect::<String>())
                                },
                                raw_preview = %raw.as_deref().unwrap_or("None").chars().take(200).collect::<String>(),
                                "[DIAG] promoted Pending tool call to Running after stream ended"
                            );
                        }
                    }
                }
            }

            let has_tool_calls = session
                .messages
                .get(assistant_index)
                .map(Self::has_unresolved_tool_calls)
                .unwrap_or(false);

            tracing::info!(
                has_tool_calls = has_tool_calls,
                finish_reason = %finish_reason.as_deref().unwrap_or("None"),
                "[DIAG] post-stream: before tool execution check"
            );

            session.touch();
            Self::emit_session_update(update_hook.as_ref(), session);

            // Plugin hook: chat.message — triggered after assistant message is finalized.
            // TS parity: fires on assistant message, not user message.
            if let Some(assistant_msg) = session.messages.get(assistant_index).cloned() {
                let mut hook_ctx = HookContext::new(HookEvent::ChatMessage)
                    .with_session(&session_id)
                    .with_data("message_id", serde_json::json!(&assistant_msg.id))
                    .with_data("message", session_message_hook_payload(&assistant_msg))
                    .with_data("parts", serde_json::json!(&assistant_msg.parts))
                    .with_data("has_tool_calls", serde_json::json!(has_tool_calls));

                if let Some(model) = provider.get_model(&model_id) {
                    hook_ctx = hook_ctx.with_data(
                        "model",
                        serde_json::json!({
                            "id": model.id,
                            "name": model.name,
                            "provider": model.provider,
                        }),
                    );
                } else {
                    hook_ctx = hook_ctx.with_data("model_id", serde_json::json!(&model_id));
                }
                hook_ctx = hook_ctx.with_data("sessionID", serde_json::json!(&session_id));
                if let Some(agent) = agent_name {
                    hook_ctx = hook_ctx.with_data("agent", serde_json::json!(agent));
                }

                let hook_outputs = rocode_plugin::trigger_collect(hook_ctx).await;
                if let Some(current_assistant) = session.messages.get_mut(assistant_index) {
                    apply_chat_message_hook_outputs(current_assistant, hook_outputs);
                }
            }

            if has_tool_calls {
                tracing::info!("Processing tool calls for session {}", session_id);

                let tool_context = rocode_tool::ToolContext::new(
                    session_id.clone(),
                    session.messages[assistant_index].id.clone(),
                    session.directory.clone(),
                )
                .with_agent(String::new())
                .with_abort(token.clone());

                if let Err(e) = Self::execute_tool_calls_with_hook(
                    session,
                    tool_registry.clone(),
                    tool_context,
                    provider.clone(),
                    &provider_id,
                    &model_id,
                    update_hook.as_ref(),
                    agent_lookup.clone(),
                    ask_question_hook.clone(),
                )
                .await
                {
                    tracing::error!("Tool execution error for session {}: {}", session_id, e);
                }
                session.touch();
                Self::emit_session_update(update_hook.as_ref(), session);
                continue;
            }

            if executed_local_tools_this_step {
                tracing::info!(
                    "[DIAG] local tool execution completed in-stream, continuing prompt loop"
                );
                continue;
            }

            if !post_first_step_ran {
                Self::ensure_title(session, provider.clone(), &model_id).await;
                let _ = Self::summarize_session(
                    session,
                    &session_id,
                    &provider_id,
                    &model_id,
                    provider.as_ref(),
                )
                .await;
                post_first_step_ran = true;
            }

            if !matches!(
                finish_reason.as_deref(),
                Some("tool-calls") | Some("tool_calls") | Some("unknown")
            ) {
                tracing::info!(
                    "Prompt loop complete for session {} with finish: {:?}",
                    session_id,
                    finish_reason
                );
                break;
            }
            tracing::info!("[DIAG] finish_reason=tool-calls, continuing prompt loop");
        }

        // Abort handling: mark any pending tool calls as error when cancelled.
        // Mirrors TS processor.ts lines 393-409 where incomplete tool parts
        // are set to error status with "Tool execution aborted".
        if token.is_cancelled() {
            Self::abort_pending_tool_calls(session);
        }

        Self::prune_after_loop(session);
        session.touch();
        Self::emit_session_update(update_hook.as_ref(), session);

        Ok(())
    }

    fn emit_session_update(update_hook: Option<&SessionUpdateHook>, session: &Session) {
        if let Some(hook) = update_hook {
            hook(session);
        }
    }

    fn maybe_emit_session_update(
        update_hook: Option<&SessionUpdateHook>,
        session: &Session,
        last_emit: &mut Instant,
        force: bool,
    ) {
        let elapsed = last_emit.elapsed();
        if force || elapsed >= Duration::from_millis(50) {
            Self::emit_session_update(update_hook, session);
            *last_emit = Instant::now();
        }
    }

    fn collect_pending_subtasks(message: &SessionMessage) -> Vec<PendingSubtask> {
        let metadata_by_id: HashMap<String, (String, String, String)> = message
            .metadata
            .get("pending_subtasks")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item.get("id").and_then(|v| v.as_str())?.to_string();
                        let agent = item
                            .get("agent")
                            .and_then(|v| v.as_str())
                            .unwrap_or("general")
                            .to_string();
                        let prompt = item
                            .get("prompt")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let description = item
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some((id, (agent, prompt, description)))
                    })
                    .collect()
            })
            .unwrap_or_default();

        message
            .parts
            .iter()
            .enumerate()
            .filter_map(|(part_index, part)| match &part.part_type {
                PartType::Subtask {
                    id,
                    description,
                    status,
                } if status == "pending" => {
                    let (agent, prompt, meta_description) = metadata_by_id
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| (id.clone(), description.clone(), description.clone()));
                    let description = if meta_description.is_empty() {
                        description.clone()
                    } else {
                        meta_description
                    };
                    let prompt = if prompt.trim().is_empty() {
                        description.clone()
                    } else {
                        prompt
                    };
                    Some(PendingSubtask {
                        part_index,
                        subtask_id: id.clone(),
                        agent,
                        prompt,
                        description,
                    })
                }
                _ => None,
            })
            .collect()
    }

    async fn process_pending_subtasks(
        session: &mut Session,
        provider: Arc<dyn Provider>,
        model_id: &str,
        provider_id: &str,
        agent_lookup: Option<Arc<dyn Fn(&str) -> Option<rocode_tool::TaskAgentInfo> + Send + Sync>>,
        ask_question_hook: Option<AskQuestionHook>,
    ) -> anyhow::Result<bool> {
        let last_user_idx = session
            .messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::User));
        let Some(last_user_idx) = last_user_idx else {
            return Ok(false);
        };

        let pending = Self::collect_pending_subtasks(&session.messages[last_user_idx]);
        if pending.is_empty() {
            return Ok(false);
        }

        let mut results: Vec<(usize, String, bool, String, String)> = Vec::new();
        let tool_registry = Arc::new(rocode_tool::create_default_registry().await);
        let mut persisted = Self::load_persisted_subsessions(session);
        let default_model = format!("{}:{}", provider_id, model_id);
        let user_text = session.messages[last_user_idx].get_text();

        for subtask in &pending {
            let subtask_max_steps = agent_lookup
                .as_ref()
                .and_then(|lookup| lookup(&subtask.agent))
                .and_then(|info| info.steps);
            let combined_prompt = if user_text.trim().is_empty() {
                subtask.prompt.clone()
            } else {
                format!("{}\n\nSubtask: {}", user_text, subtask.prompt)
            };
            let subsession_id = format!("task_subtask_{}", subtask.subtask_id);
            persisted
                .entry(subsession_id.clone())
                .or_insert_with(|| PersistedSubsession {
                    agent: subtask.agent.clone(),
                    model: Some(default_model.clone()),
                    max_steps: subtask_max_steps,
                    directory: Some(session.directory.clone()),
                    disabled_tools: Vec::new(),
                    history: Vec::new(),
                });
            let state_snapshot =
                persisted
                    .get(&subsession_id)
                    .cloned()
                    .unwrap_or(PersistedSubsession {
                        agent: subtask.agent.clone(),
                        model: Some(default_model.clone()),
                        max_steps: subtask_max_steps,
                        directory: Some(session.directory.clone()),
                        disabled_tools: Vec::new(),
                        history: Vec::new(),
                    });

            match Self::execute_persisted_subsession_prompt(
                &state_snapshot,
                &combined_prompt,
                provider.clone(),
                tool_registry.clone(),
                &default_model,
                Some(session.directory.as_str()),
                ask_question_hook.clone(),
                Some(session.id.clone()),
            )
            .await
            {
                Ok(output) => {
                    if let Some(existing) = persisted.get_mut(&subsession_id) {
                        existing.history.push(PersistedSubsessionTurn {
                            prompt: combined_prompt,
                            output: output.clone(),
                        });
                    }
                    results.push((
                        subtask.part_index,
                        subtask.subtask_id.clone(),
                        false,
                        subtask.description.clone(),
                        output,
                    ));
                }
                Err(error) => {
                    results.push((
                        subtask.part_index,
                        subtask.subtask_id.clone(),
                        true,
                        subtask.description.clone(),
                        error.to_string(),
                    ));
                }
            }
        }

        for (part_index, subtask_id, is_error, description, output) in results {
            if let Some(part) = session.messages[last_user_idx].parts.get_mut(part_index) {
                if let PartType::Subtask { status, .. } = &mut part.part_type {
                    *status = if is_error {
                        "error".to_string()
                    } else {
                        "completed".to_string()
                    };
                }
            }

            let assistant = session.add_assistant_message();
            assistant
                .metadata
                .insert("subtask_id".to_string(), serde_json::json!(subtask_id));
            assistant.metadata.insert(
                "subtask_status".to_string(),
                serde_json::json!(if is_error { "error" } else { "completed" }),
            );
            assistant.add_text(format!(
                "Subtask `{}` {}:\n{}",
                description,
                if is_error { "failed" } else { "completed" },
                output
            ));
        }

        Self::save_persisted_subsessions(session, &persisted);

        Ok(true)
    }
}

impl Default for SessionPrompt {
    fn default() -> Self {
        Self::new(Arc::new(RwLock::new(SessionStateManager::new())))
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PromptError {
    #[error("Session is busy: {0}")]
    Busy(String),
    #[error("No user message found")]
    NoUserMessage,
    #[error("Provider error: {0}")]
    Provider(String),
    #[error("Cancelled")]
    Cancelled,
}

/// Regex that matches `@reference` patterns. We use a capturing group for the
/// preceding character instead of a lookbehind (unsupported by the `regex` crate).
/// Group 1 = preceding char (or empty at start of string), Group 2 = the reference name.
const FILE_REFERENCE_REGEX: &str = r"(?:^|([^\w`]))@(\.?[^\s`,.]*(?:\.[^\s`,.]+)*)";

pub async fn resolve_prompt_parts(
    template: &str,
    worktree: &std::path::Path,
    known_agents: &[String],
) -> Vec<PartInput> {
    let mut parts = vec![PartInput::Text {
        text: template.to_string(),
    }];

    let re = regex::Regex::new(FILE_REFERENCE_REGEX).unwrap();
    let mut seen = std::collections::HashSet::new();

    for cap in re.captures_iter(template) {
        // Group 1 is the preceding char — if it matched a word char or backtick
        // the overall pattern wouldn't match (they're excluded by [^\w`]).
        // Group 2 is the actual reference name.
        if let Some(name) = cap.get(2) {
            let name = name.as_str();
            if name.is_empty() || seen.contains(name) {
                continue;
            }
            seen.insert(name.to_string());

            let filepath = if name.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(&name[2..])
                } else {
                    continue;
                }
            } else {
                worktree.join(name)
            };

            if let Ok(metadata) = tokio::fs::metadata(&filepath).await {
                let url = format!("file://{}", filepath.display());

                if metadata.is_dir() {
                    parts.push(PartInput::File {
                        url,
                        filename: Some(name.to_string()),
                        mime: Some("application/x-directory".to_string()),
                    });
                } else {
                    parts.push(PartInput::File {
                        url,
                        filename: Some(name.to_string()),
                        mime: Some("text/plain".to_string()),
                    });
                }
            } else if known_agents.iter().any(|a| a == name) {
                // Not a file — check if it's a known agent name
                parts.push(PartInput::Agent {
                    name: name.to_string(),
                });
            }
        }
    }

    parts
}

pub fn extract_file_references(template: &str) -> Vec<String> {
    let re = regex::Regex::new(FILE_REFERENCE_REGEX).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for cap in re.captures_iter(template) {
        if let Some(name) = cap.get(2) {
            let name = name.as_str().to_string();
            if !name.is_empty() && !seen.contains(&name) {
                seen.insert(name.clone());
                result.push(name);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessagePart;
    use async_trait::async_trait;
    use futures::stream;
    use rocode_provider::{
        ChatRequest, ChatResponse, ModelInfo, ProviderError, StreamEvent, StreamResult, StreamUsage,
    };
    use rocode_tool::{Tool, ToolContext, ToolError, ToolResult};
    use std::sync::Mutex as StdMutex;

    struct StaticModelProvider {
        model: Option<ModelInfo>,
    }

    impl StaticModelProvider {
        fn with_model(model_id: &str, context_window: u64, max_output_tokens: u64) -> Self {
            Self {
                model: Some(ModelInfo {
                    id: model_id.to_string(),
                    name: "Static Model".to_string(),
                    provider: "mock".to_string(),
                    context_window,
                    max_input_tokens: None,
                    max_output_tokens,
                    supports_vision: false,
                    supports_tools: false,
                    cost_per_million_input: 0.0,
                    cost_per_million_output: 0.0,
                }),
            }
        }
    }

    #[async_trait]
    impl Provider for StaticModelProvider {
        fn id(&self) -> &str {
            "mock"
        }

        fn name(&self) -> &str {
            "Mock"
        }

        fn models(&self) -> Vec<ModelInfo> {
            self.model.clone().into_iter().collect()
        }

        fn get_model(&self, id: &str) -> Option<&ModelInfo> {
            self.model.as_ref().filter(|model| model.id == id)
        }

        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
            Err(ProviderError::InvalidRequest(
                "chat() not used in this test".to_string(),
            ))
        }

        async fn chat_stream(&self, _request: ChatRequest) -> Result<StreamResult, ProviderError> {
            Ok(Box::pin(stream::empty()))
        }
    }

    struct ScriptedStreamProvider {
        model: ModelInfo,
        events: Vec<StreamEvent>,
    }

    #[async_trait]
    impl Provider for ScriptedStreamProvider {
        fn id(&self) -> &str {
            "mock"
        }

        fn name(&self) -> &str {
            "Mock"
        }

        fn models(&self) -> Vec<ModelInfo> {
            vec![self.model.clone()]
        }

        fn get_model(&self, id: &str) -> Option<&ModelInfo> {
            if self.model.id == id {
                Some(&self.model)
            } else {
                None
            }
        }

        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
            Err(ProviderError::InvalidRequest(
                "chat() not used in this test".to_string(),
            ))
        }

        async fn chat_stream(&self, _request: ChatRequest) -> Result<StreamResult, ProviderError> {
            Ok(Box::pin(stream::iter(
                self.events
                    .clone()
                    .into_iter()
                    .map(Result::<StreamEvent, ProviderError>::Ok),
            )))
        }
    }

    struct MultiTurnScriptedProvider {
        model: ModelInfo,
        turns: Arc<StdMutex<std::collections::VecDeque<Vec<StreamEvent>>>>,
        request_count: Arc<StdMutex<usize>>,
    }

    impl MultiTurnScriptedProvider {
        fn new(model: ModelInfo, turns: Vec<Vec<StreamEvent>>) -> Self {
            Self {
                model,
                turns: Arc::new(StdMutex::new(turns.into())),
                request_count: Arc::new(StdMutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl Provider for MultiTurnScriptedProvider {
        fn id(&self) -> &str {
            "mock"
        }

        fn name(&self) -> &str {
            "Mock"
        }

        fn models(&self) -> Vec<ModelInfo> {
            vec![self.model.clone()]
        }

        fn get_model(&self, id: &str) -> Option<&ModelInfo> {
            if self.model.id == id {
                Some(&self.model)
            } else {
                None
            }
        }

        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, ProviderError> {
            Err(ProviderError::InvalidRequest(
                "chat() not used in this test".to_string(),
            ))
        }

        async fn chat_stream(&self, _request: ChatRequest) -> Result<StreamResult, ProviderError> {
            {
                let mut count = self
                    .request_count
                    .lock()
                    .expect("request_count lock should not poison");
                *count += 1;
            }

            let events = self
                .turns
                .lock()
                .expect("turns lock should not poison")
                .pop_front()
                .ok_or_else(|| {
                    ProviderError::InvalidRequest(
                        "no scripted response left for chat_stream".to_string(),
                    )
                })?;

            Ok(Box::pin(stream::iter(
                events
                    .into_iter()
                    .map(Result::<StreamEvent, ProviderError>::Ok),
            )))
        }
    }

    struct NoArgEchoTool;

    #[async_trait]
    impl Tool for NoArgEchoTool {
        fn id(&self) -> &str {
            "noarg_echo"
        }

        fn description(&self) -> &str {
            "Echoes input for tests"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::simple("NoArg Echo", args.to_string()))
        }
    }

    struct AlwaysInvalidArgsTool;

    #[async_trait]
    impl Tool for AlwaysInvalidArgsTool {
        fn id(&self) -> &str {
            "needs_path"
        }

        fn description(&self) -> &str {
            "Fails validation for tests"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "filePath": { "type": "string" }
                },
                "required": ["filePath"]
            })
        }

        fn validate(&self, _args: &serde_json::Value) -> Result<(), ToolError> {
            Err(ToolError::InvalidArguments(
                "filePath is required".to_string(),
            ))
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Err(ToolError::ExecutionError(
                "validate should prevent execute".to_string(),
            ))
        }
    }
    #[test]
    fn insert_reminders_adds_plan_prompt_for_plan_agent() {
        let messages = vec![SessionMessage::user("ses_test", "plan this")];
        let output = insert_reminders(&messages, "plan", false);
        let last = output.last().unwrap();
        let injected = last
            .parts
            .iter()
            .filter_map(|p| match &p.part_type {
                PartType::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(injected.contains("You are in PLAN mode"));
    }

    #[test]
    fn insert_reminders_adds_build_switch_after_plan() {
        let mut user = SessionMessage::user("ses_test", "execute this");
        user.metadata
            .insert("agent".to_string(), serde_json::json!("plan"));
        let output = insert_reminders(&[user], "build", true);
        let last = output.last().unwrap();
        let injected = last
            .parts
            .iter()
            .filter_map(|p| match &p.part_type {
                PartType::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(injected.contains("The user has approved your plan"));
    }

    #[tokio::test]
    async fn prompt_with_update_hook_emits_incremental_snapshots() {
        let prompt = SessionPrompt::default();
        let mut session = Session::new("proj", ".");
        let provider = Arc::new(ScriptedStreamProvider {
            model: ModelInfo {
                id: "test-model".to_string(),
                name: "Test Model".to_string(),
                provider: "mock".to_string(),
                context_window: 8192,
                max_input_tokens: None,
                max_output_tokens: 1024,
                supports_vision: false,
                supports_tools: false,
                cost_per_million_input: 0.0,
                cost_per_million_output: 0.0,
            },
            events: vec![
                StreamEvent::Start,
                StreamEvent::TextDelta("Hel".to_string()),
                StreamEvent::TextDelta("lo".to_string()),
                StreamEvent::FinishStep {
                    finish_reason: Some("stop".to_string()),
                    usage: StreamUsage {
                        prompt_tokens: 3,
                        completion_tokens: 2,
                        ..Default::default()
                    },
                    provider_metadata: None,
                },
                StreamEvent::Done,
            ],
        });

        let snapshots = Arc::new(StdMutex::new(Vec::<Session>::new()));
        let snapshot_sink = snapshots.clone();
        let hook: SessionUpdateHook = Arc::new(move |snapshot| {
            snapshot_sink
                .lock()
                .expect("snapshot lock should not poison")
                .push(snapshot.clone());
        });

        let input = PromptInput {
            session_id: session.id.clone(),
            message_id: None,
            model: Some(ModelRef {
                provider_id: "mock".to_string(),
                model_id: "test-model".to_string(),
            }),
            agent: None,
            no_reply: false,
            system: None,
            variant: None,
            parts: vec![PartInput::Text {
                text: "Say hello".to_string(),
            }],
            tools: None,
        };

        prompt
            .prompt_with_update_hook(
                input,
                &mut session,
                provider,
                None,
                Vec::new(),
                AgentParams::default(),
                Some(hook),
                None,
                None,
            )
            .await
            .expect("prompt_with_update_hook should succeed");

        let snapshots_guard = snapshots.lock().expect("snapshot lock should not poison");
        assert!(snapshots_guard.len() >= 3);
        let saw_partial = snapshots_guard.iter().any(|snap| {
            snap.messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, MessageRole::Assistant))
                .map(|m| m.get_text() == "Hel")
                .unwrap_or(false)
        });
        assert!(
            saw_partial,
            "expected at least one streamed partial assistant snapshot"
        );
        drop(snapshots_guard);

        let final_text = session
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Assistant))
            .map(SessionMessage::get_text)
            .unwrap_or_default();
        assert_eq!(final_text, "Hello");
    }

    #[tokio::test]
    async fn prompt_continues_after_tool_calls_without_finish_step_reason() {
        let prompt = SessionPrompt::default();
        let mut session = Session::new("proj", ".");
        let temp_dir = tempfile::tempdir().expect("tempdir should create");
        let file_path = temp_dir.path().join("sample.txt");
        tokio::fs::write(&file_path, "alpha\nbeta")
            .await
            .expect("file should write");
        let file_path = file_path.to_string_lossy().to_string();

        let scripted = MultiTurnScriptedProvider::new(
            ModelInfo {
                id: "test-model".to_string(),
                name: "Test Model".to_string(),
                provider: "mock".to_string(),
                context_window: 8192,
                max_input_tokens: None,
                max_output_tokens: 1024,
                supports_vision: false,
                supports_tools: true,
                cost_per_million_input: 0.0,
                cost_per_million_output: 0.0,
            },
            vec![
                vec![
                    StreamEvent::Start,
                    StreamEvent::ToolCallStart {
                        id: "tool-call-0".to_string(),
                        name: "read".to_string(),
                    },
                    StreamEvent::ToolCallEnd {
                        id: "tool-call-0".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({ "file_path": file_path }),
                    },
                    StreamEvent::Done,
                ],
                vec![
                    StreamEvent::Start,
                    StreamEvent::TextDelta("Read complete".to_string()),
                    StreamEvent::FinishStep {
                        finish_reason: Some("stop".to_string()),
                        usage: StreamUsage::default(),
                        provider_metadata: None,
                    },
                    StreamEvent::Done,
                ],
            ],
        );
        let request_count = scripted.request_count.clone();
        let provider: Arc<dyn Provider> = Arc::new(scripted);

        let input = PromptInput {
            session_id: session.id.clone(),
            message_id: None,
            model: Some(ModelRef {
                provider_id: "mock".to_string(),
                model_id: "test-model".to_string(),
            }),
            agent: None,
            no_reply: false,
            system: None,
            variant: None,
            parts: vec![PartInput::Text {
                text: "Read the file and summarize".to_string(),
            }],
            tools: None,
        };

        prompt
            .prompt_with_update_hook(
                input,
                &mut session,
                provider,
                None,
                Vec::new(),
                AgentParams::default(),
                None,
                None,
                None,
            )
            .await
            .expect("prompt_with_update_hook should succeed");

        let request_count = *request_count
            .lock()
            .expect("request_count lock should not poison");
        assert_eq!(request_count, 2, "expected a second model round");

        let final_text = session
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Assistant))
            .map(SessionMessage::get_text)
            .unwrap_or_default();
        assert_eq!(final_text, "Read complete");
    }

    #[tokio::test]
    async fn create_user_message_persists_pending_subtask_payload() {
        let prompt = SessionPrompt::default();
        let mut session = Session::new("proj", ".");
        let input = PromptInput {
            session_id: session.id.clone(),
            message_id: None,
            model: None,
            agent: None,
            no_reply: false,
            system: None,
            variant: None,
            tools: None,
            parts: vec![PartInput::Subtask {
                prompt: "Inspect codegen path".to_string(),
                description: Some("Inspect codegen".to_string()),
                agent: "explore".to_string(),
            }],
        };

        prompt
            .create_user_message(&input, &mut session)
            .await
            .expect("create_user_message should succeed");

        let msg = session.messages.last().expect("user message should exist");
        let pending = msg
            .metadata
            .get("pending_subtasks")
            .and_then(|v| v.as_array())
            .expect("pending_subtasks metadata should exist");
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].get("agent").and_then(|v| v.as_str()),
            Some("explore")
        );
        assert_eq!(
            pending[0].get("prompt").and_then(|v| v.as_str()),
            Some("Inspect codegen path")
        );
        assert!(msg.parts.iter().any(|p| match &p.part_type {
            PartType::Subtask { status, .. } => status == "pending",
            _ => false,
        }));
    }
    #[test]
    fn shell_exec_uses_zsh_login_invocation() {
        let invocation = resolve_shell_invocation(Some("/bin/zsh"), "echo hello");
        assert_eq!(invocation.program, "/bin/zsh");
        assert_eq!(invocation.args[0], "-c");
        assert_eq!(invocation.args[1], "-l");
        assert!(invocation.args[2].contains(".zshenv"));
        assert!(invocation.args[2].contains("eval"));
    }

    #[test]
    fn shell_exec_uses_bash_login_invocation() {
        let invocation = resolve_shell_invocation(Some("/bin/bash"), "echo hello");
        assert_eq!(invocation.program, "/bin/bash");
        assert_eq!(invocation.args[0], "-c");
        assert_eq!(invocation.args[1], "-l");
        assert!(invocation.args[2].contains("shopt -s expand_aliases"));
        assert!(invocation.args[2].contains(".bashrc"));
    }

    #[tokio::test]
    async fn resolve_tools_with_mcp_registry_includes_mcp_tools() {
        let tool_registry = rocode_tool::create_default_registry().await;
        let mcp_registry = rocode_mcp::McpToolRegistry::new();
        mcp_registry
            .register(rocode_mcp::McpTool::new(
                "github",
                "search",
                Some("Search GitHub".to_string()),
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    }
                }),
            ))
            .await;

        let tools = resolve_tools_with_mcp_registry(&tool_registry, Some(&mcp_registry)).await;
        assert!(tools.iter().any(|t| t.name == "github_search"));
    }

    #[tokio::test]
    async fn execute_tool_calls_ignores_empty_tool_name() {
        let tool_registry = Arc::new(rocode_tool::ToolRegistry::new());
        tool_registry.register(NoArgEchoTool).await;

        let mut session = Session::new("proj", ".");
        let sid = session.id.clone();
        session
            .messages
            .push(SessionMessage::user(sid.clone(), "run tools"));

        let mut assistant = SessionMessage::assistant(sid);
        assistant.parts.push(crate::MessagePart {
            id: format!("prt_{}", uuid::Uuid::new_v4()),
            part_type: PartType::ToolCall {
                id: "call_empty".to_string(),
                name: " ".to_string(),
                input: serde_json::json!({}),
                status: crate::ToolCallStatus::Running,
                raw: None,
                state: None,
            },
            created_at: chrono::Utc::now(),
            message_id: None,
        });
        assistant.add_tool_call("call_ok", "noarg_echo", serde_json::json!({}));
        session.messages.push(assistant);

        let provider: Arc<dyn Provider> =
            Arc::new(StaticModelProvider::with_model("test-model", 8192, 1024));
        let ctx = ToolContext::new(session.id.clone(), "msg_test".to_string(), ".".to_string());

        SessionPrompt::execute_tool_calls(
            &mut session,
            tool_registry,
            ctx,
            provider,
            "mock",
            "test-model",
        )
        .await
        .expect("execute_tool_calls should succeed");

        let tool_msg = session
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Tool))
            .expect("tool message should exist");
        let result_ids: Vec<&str> = tool_msg
            .parts
            .iter()
            .filter_map(|part| match &part.part_type {
                PartType::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_ids, vec!["call_ok"]);
    }

    #[tokio::test]
    async fn execute_tool_calls_runs_no_arg_tool() {
        let tool_registry = Arc::new(rocode_tool::ToolRegistry::new());
        tool_registry.register(NoArgEchoTool).await;

        let mut session = Session::new("proj", ".");
        let sid = session.id.clone();
        session
            .messages
            .push(SessionMessage::user(sid.clone(), "run noarg"));
        let mut assistant = SessionMessage::assistant(sid);
        assistant.add_tool_call("call_noarg", "noarg_echo", serde_json::json!({}));
        session.messages.push(assistant);

        let provider: Arc<dyn Provider> =
            Arc::new(StaticModelProvider::with_model("test-model", 8192, 1024));
        let ctx = ToolContext::new(session.id.clone(), "msg_test".to_string(), ".".to_string());

        SessionPrompt::execute_tool_calls(
            &mut session,
            tool_registry,
            ctx,
            provider,
            "mock",
            "test-model",
        )
        .await
        .expect("execute_tool_calls should succeed");

        let tool_msg = session
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Tool))
            .expect("tool message should exist");

        let (content, is_error) = tool_msg
            .parts
            .iter()
            .find_map(|part| match &part.part_type {
                PartType::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    ..
                } if tool_call_id == "call_noarg" => Some((content.clone(), *is_error)),
                _ => None,
            })
            .expect("noarg result should exist");

        assert!(!is_error);
        assert_eq!(content, "{}");
    }

    #[tokio::test]
    async fn execute_tool_calls_routes_invalid_arguments_to_invalid_tool() {
        let tool_registry = Arc::new(rocode_tool::ToolRegistry::new());
        tool_registry.register(AlwaysInvalidArgsTool).await;
        tool_registry
            .register(rocode_tool::invalid::InvalidTool)
            .await;

        let mut session = Session::new("proj", ".");
        let sid = session.id.clone();
        session
            .messages
            .push(SessionMessage::user(sid.clone(), "run invalid"));
        let mut assistant = SessionMessage::assistant(sid);
        assistant.add_tool_call("call_invalid", "needs_path", serde_json::json!({}));
        session.messages.push(assistant);

        let provider: Arc<dyn Provider> =
            Arc::new(StaticModelProvider::with_model("test-model", 8192, 1024));
        let ctx = ToolContext::new(session.id.clone(), "msg_test".to_string(), ".".to_string());

        SessionPrompt::execute_tool_calls(
            &mut session,
            tool_registry,
            ctx,
            provider,
            "mock",
            "test-model",
        )
        .await
        .expect("execute_tool_calls should succeed");

        let assistant_msg = session
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Assistant))
            .expect("assistant message should exist");
        let tool_call = assistant_msg
            .parts
            .iter()
            .find_map(|part| match &part.part_type {
                PartType::ToolCall {
                    id,
                    name,
                    input,
                    status,
                    ..
                } if id == "call_invalid" => Some((name, input, status)),
                _ => None,
            })
            .expect("tool call should exist");
        assert_eq!(tool_call.0, "invalid");
        assert_eq!(
            tool_call.1.get("tool").and_then(|v| v.as_str()),
            Some("needs_path")
        );
        assert!(tool_call.1.get("receivedArgs").is_none());
        assert!(matches!(tool_call.2, crate::ToolCallStatus::Completed));

        let tool_msg = session
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Tool))
            .expect("tool message should exist");
        let (content, is_error) = tool_msg
            .parts
            .iter()
            .find_map(|part| match &part.part_type {
                PartType::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    ..
                } if tool_call_id == "call_invalid" => Some((content.clone(), *is_error)),
                _ => None,
            })
            .expect("invalid fallback result should exist");
        assert!(!is_error);
        assert!(content.contains("The arguments provided to the tool are invalid:"));
    }

    #[tokio::test]
    async fn execute_tool_calls_only_runs_running_tool_calls() {
        let tool_registry = Arc::new(rocode_tool::ToolRegistry::new());
        tool_registry.register(NoArgEchoTool).await;

        let mut session = Session::new("proj", ".");
        let sid = session.id.clone();
        session
            .messages
            .push(SessionMessage::user(sid.clone(), "run running only"));
        let mut assistant = SessionMessage::assistant(sid);
        assistant.parts.push(crate::MessagePart {
            id: format!("prt_{}", uuid::Uuid::new_v4()),
            part_type: PartType::ToolCall {
                id: "call_pending".to_string(),
                name: "noarg_echo".to_string(),
                input: serde_json::json!({}),
                status: crate::ToolCallStatus::Pending,
                raw: Some("{".to_string()),
                state: Some(crate::ToolState::Pending {
                    input: serde_json::json!({}),
                    raw: "{".to_string(),
                }),
            },
            created_at: chrono::Utc::now(),
            message_id: None,
        });
        assistant.add_tool_call("call_running", "noarg_echo", serde_json::json!({}));
        session.messages.push(assistant);

        let provider: Arc<dyn Provider> =
            Arc::new(StaticModelProvider::with_model("test-model", 8192, 1024));
        let ctx = ToolContext::new(session.id.clone(), "msg_test".to_string(), ".".to_string());

        SessionPrompt::execute_tool_calls(
            &mut session,
            tool_registry,
            ctx,
            provider,
            "mock",
            "test-model",
        )
        .await
        .expect("execute_tool_calls should succeed");

        let tool_msg = session
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Tool))
            .expect("tool message should exist");
        let result_ids: Vec<&str> = tool_msg
            .parts
            .iter()
            .filter_map(|part| match &part.part_type {
                PartType::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_ids, vec!["call_running"]);
    }

    #[tokio::test]
    async fn execute_tool_calls_reused_call_id_in_new_turn_still_executes() {
        let tool_registry = Arc::new(rocode_tool::ToolRegistry::new());
        tool_registry.register(NoArgEchoTool).await;

        let mut session = Session::new("proj", ".");
        let sid = session.id.clone();

        session
            .messages
            .push(SessionMessage::user(sid.clone(), "turn one"));
        let mut assistant_1 = SessionMessage::assistant(sid.clone());
        assistant_1.add_tool_call("tool-call-0", "noarg_echo", serde_json::json!({}));
        session.messages.push(assistant_1);
        let mut tool_msg_1 = SessionMessage::tool(sid.clone());
        tool_msg_1.add_tool_result("tool-call-0", "{}", false);
        session.messages.push(tool_msg_1);

        session
            .messages
            .push(SessionMessage::user(sid.clone(), "turn two"));
        let mut assistant_2 = SessionMessage::assistant(sid);
        assistant_2.add_tool_call("tool-call-0", "noarg_echo", serde_json::json!({}));
        session.messages.push(assistant_2);

        let provider: Arc<dyn Provider> =
            Arc::new(StaticModelProvider::with_model("test-model", 8192, 1024));
        let ctx = ToolContext::new(session.id.clone(), "msg_test".to_string(), ".".to_string());

        SessionPrompt::execute_tool_calls(
            &mut session,
            tool_registry,
            ctx,
            provider,
            "mock",
            "test-model",
        )
        .await
        .expect("execute_tool_calls should succeed");

        let tool_msgs: Vec<&SessionMessage> = session
            .messages
            .iter()
            .filter(|m| matches!(m.role, MessageRole::Tool))
            .collect();
        assert!(
            tool_msgs.len() >= 2,
            "expected a second tool message for the new turn"
        );

        let last_tool_msg = tool_msgs.last().expect("latest tool message should exist");
        let second_turn_result_count = last_tool_msg
            .parts
            .iter()
            .filter(|part| {
                matches!(
                    &part.part_type,
                    PartType::ToolResult { tool_call_id, .. } if tool_call_id == "tool-call-0"
                )
            })
            .count();
        assert_eq!(second_turn_result_count, 1);
    }

    // ── PartInput serde round-trip tests ──

    #[test]
    fn part_input_text_round_trip() {
        let part = PartInput::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");

        let back: PartInput = serde_json::from_value(json).unwrap();
        assert!(matches!(back, PartInput::Text { text } if text == "hello"));
    }

    #[test]
    fn part_input_file_round_trip() {
        let part = PartInput::File {
            url: "file:///tmp/test.rs".to_string(),
            filename: Some("test.rs".to_string()),
            mime: Some("text/plain".to_string()),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["type"], "file");
        assert_eq!(json["url"], "file:///tmp/test.rs");
        assert_eq!(json["filename"], "test.rs");

        let back: PartInput = serde_json::from_value(json).unwrap();
        assert!(matches!(back, PartInput::File { url, .. } if url == "file:///tmp/test.rs"));
    }

    #[test]
    fn part_input_agent_round_trip() {
        let part = PartInput::Agent {
            name: "explore".to_string(),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["type"], "agent");
        assert_eq!(json["name"], "explore");

        let back: PartInput = serde_json::from_value(json).unwrap();
        assert!(matches!(back, PartInput::Agent { name } if name == "explore"));
    }

    #[test]
    fn part_input_subtask_round_trip() {
        let part = PartInput::Subtask {
            prompt: "do stuff".to_string(),
            description: Some("stuff".to_string()),
            agent: "build".to_string(),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["type"], "subtask");
        assert_eq!(json["agent"], "build");

        let back: PartInput = serde_json::from_value(json).unwrap();
        assert!(matches!(back, PartInput::Subtask { agent, .. } if agent == "build"));
    }

    #[test]
    fn part_input_try_from_value() {
        let val = serde_json::json!({"type": "text", "text": "hi"});
        let part = PartInput::try_from(val).unwrap();
        assert!(matches!(part, PartInput::Text { text } if text == "hi"));
    }

    #[test]
    fn part_input_try_from_invalid_value() {
        let val = serde_json::json!({"type": "unknown", "data": 42});
        assert!(PartInput::try_from(val).is_err());
    }

    #[test]
    fn part_input_parse_array_mixed() {
        let arr = serde_json::json!([
            {"type": "text", "text": "hello"},
            {"type": "agent", "name": "explore"},
            {"type": "bogus"},
            {"type": "file", "url": "file:///x", "filename": "x", "mime": "text/plain"}
        ]);
        let parts = PartInput::parse_array(&arr);
        assert_eq!(parts.len(), 3); // bogus entry skipped
        assert!(matches!(&parts[0], PartInput::Text { text } if text == "hello"));
        assert!(matches!(&parts[1], PartInput::Agent { name } if name == "explore"));
        assert!(matches!(&parts[2], PartInput::File { url, .. } if url == "file:///x"));
    }

    #[test]
    fn part_input_parse_array_non_array() {
        let val = serde_json::json!("not an array");
        assert!(PartInput::parse_array(&val).is_empty());
    }

    #[test]
    fn part_input_file_skips_none_fields_in_json() {
        let part = PartInput::File {
            url: "file:///tmp/x".to_string(),
            filename: None,
            mime: None,
        };
        let json = serde_json::to_value(&part).unwrap();
        assert!(json.get("filename").is_none());
        assert!(json.get("mime").is_none());
    }

    // ── resolve_prompt_parts tests ──

    #[tokio::test]
    async fn resolve_prompt_parts_plain_text() {
        let parts =
            resolve_prompt_parts("just plain text", std::path::Path::new("/tmp"), &[]).await;
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], PartInput::Text { text } if text == "just plain text"));
    }

    #[tokio::test]
    async fn resolve_prompt_parts_agent_fallback() {
        // @explore doesn't exist as a file, but is a known agent
        let agents = vec!["explore".to_string(), "build".to_string()];
        let parts = resolve_prompt_parts(
            "check @explore for details",
            std::path::Path::new("/tmp"),
            &agents,
        )
        .await;
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], PartInput::Text { .. }));
        assert!(matches!(&parts[1], PartInput::Agent { name } if name == "explore"));
    }

    #[tokio::test]
    async fn resolve_prompt_parts_deduplicates() {
        let parts = resolve_prompt_parts(
            "see @explore and @explore again",
            std::path::Path::new("/tmp"),
            &["explore".to_string()],
        )
        .await;
        // text + one agent (deduplicated)
        assert_eq!(parts.len(), 2);
    }

    #[tokio::test]
    async fn resolve_prompt_parts_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        tokio::fs::write(&file, "fn main() {}").await.unwrap();

        let parts = resolve_prompt_parts("look at @test.rs", dir.path(), &[]).await;
        assert_eq!(parts.len(), 2);
        assert!(
            matches!(&parts[1], PartInput::File { mime, .. } if mime.as_deref() == Some("text/plain"))
        );
    }

    #[tokio::test]
    async fn resolve_prompt_parts_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("src");
        tokio::fs::create_dir(&sub).await.unwrap();

        let parts = resolve_prompt_parts("look at @src", dir.path(), &[]).await;
        assert_eq!(parts.len(), 2);
        assert!(
            matches!(&parts[1], PartInput::File { mime, .. } if mime.as_deref() == Some("application/x-directory"))
        );
    }

    /// Regression test for the prompt loop early-exit bug:
    /// When the assistant message has text + tool calls and finish="tool-calls",
    /// the loop must NOT break at the top-of-loop check.
    /// Previously, the check used `has_finish = !text.is_empty()` which caused
    /// premature exit when models emit text before tool calls.
    #[test]
    fn early_exit_does_not_break_on_tool_calls_finish() {
        // Simulate: user message at index 0, assistant at index 1
        let user = SessionMessage::user("s1", "hello");
        let mut assistant = SessionMessage::assistant("s1");
        // Assistant has text content (model explained before calling tools)
        assistant.parts.push(MessagePart {
            id: "prt_text".to_string(),
            part_type: PartType::Text {
                text: "Let me read those files for you.".to_string(),
                synthetic: None,
                ignored: None,
            },
            created_at: chrono::Utc::now(),
            message_id: None,
        });
        // finish_reason is "tool-calls" — loop should continue, not break
        assistant.finish = Some("tool-calls".to_string());

        let messages = vec![user, assistant];

        let last_user_idx = messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::User))
            .unwrap();
        let last_assistant_idx = messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant));

        // The early-exit check from the prompt loop
        let should_break = if let Some(assistant_idx) = last_assistant_idx {
            let assistant = &messages[assistant_idx];
            let is_terminal = assistant
                .finish
                .as_deref()
                .is_some_and(|f| !matches!(f, "tool-calls" | "tool_calls" | "unknown"));
            is_terminal && last_user_idx < assistant_idx
        } else {
            false
        };

        assert!(
            !should_break,
            "early-exit must NOT trigger when finish='tool-calls'"
        );
    }

    /// Verify that the early-exit check DOES break when finish is terminal
    /// (e.g. "stop") and assistant is after the last user message.
    #[test]
    fn early_exit_breaks_on_terminal_finish() {
        let user = SessionMessage::user("s1", "hello");
        let mut assistant = SessionMessage::assistant("s1");
        assistant.parts.push(MessagePart {
            id: "prt_text".to_string(),
            part_type: PartType::Text {
                text: "Here is my response.".to_string(),
                synthetic: None,
                ignored: None,
            },
            created_at: chrono::Utc::now(),
            message_id: None,
        });
        assistant.finish = Some("stop".to_string());

        let messages = vec![user, assistant];

        let last_user_idx = messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::User))
            .unwrap();
        let last_assistant_idx = messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant));

        let should_break = if let Some(assistant_idx) = last_assistant_idx {
            let assistant = &messages[assistant_idx];
            let is_terminal = assistant
                .finish
                .as_deref()
                .is_some_and(|f| !matches!(f, "tool-calls" | "tool_calls" | "unknown"));
            is_terminal && last_user_idx < assistant_idx
        } else {
            false
        };

        assert!(should_break, "early-exit MUST trigger when finish='stop'");
    }

    /// Verify that the early-exit check does NOT break when finish is None
    /// (assistant message still streaming / no FinishStep received yet).
    #[test]
    fn early_exit_does_not_break_when_finish_is_none() {
        let user = SessionMessage::user("s1", "hello");
        let mut assistant = SessionMessage::assistant("s1");
        assistant.parts.push(MessagePart {
            id: "prt_text".to_string(),
            part_type: PartType::Text {
                text: "partial response...".to_string(),
                synthetic: None,
                ignored: None,
            },
            created_at: chrono::Utc::now(),
            message_id: None,
        });
        // finish is None — still streaming
        assistant.finish = None;

        let messages = vec![user, assistant];

        let last_user_idx = messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::User))
            .unwrap();
        let last_assistant_idx = messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant));

        let should_break = if let Some(assistant_idx) = last_assistant_idx {
            let assistant = &messages[assistant_idx];
            let is_terminal = assistant
                .finish
                .as_deref()
                .is_some_and(|f| !matches!(f, "tool-calls" | "tool_calls" | "unknown"));
            is_terminal && last_user_idx < assistant_idx
        } else {
            false
        };

        assert!(
            !should_break,
            "early-exit must NOT trigger when finish is None"
        );
    }

    #[test]
    fn chat_message_hook_not_triggered_on_user_message_creation() {
        let source = include_str!("mod.rs");
        let create_user_fn = source
            .find("async fn create_user_message")
            .expect("create_user_message should exist");
        let loop_inner_fn = source
            .find("async fn loop_inner")
            .expect("loop_inner should exist");
        let create_user_section = &source[create_user_fn..loop_inner_fn];
        assert!(
            !create_user_section.contains("HookEvent::ChatMessage"),
            "ChatMessage hook should not be in create_user_message"
        );
    }
}
