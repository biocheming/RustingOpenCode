// Tool execution + subsession methods for SessionPrompt

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::Mutex;

use rocode_provider::{Provider, ToolDefinition};

use crate::{MessageRole, PartType, Session, SessionMessage};

use super::subtask::SubtaskExecutor;
use super::{
    AgentParams, AskQuestionHook, ModelRef, PersistedSubsession, PersistedSubsessionTurn,
    SessionPrompt, SessionUpdateHook,
};

impl SessionPrompt {
    pub async fn execute_tool_calls(
        session: &mut Session,
        tool_registry: Arc<rocode_tool::ToolRegistry>,
        ctx: rocode_tool::ToolContext,
        provider: Arc<dyn Provider>,
        provider_id: &str,
        model_id: &str,
    ) -> anyhow::Result<()> {
        Self::execute_tool_calls_with_hook(
            session,
            tool_registry,
            ctx,
            provider,
            provider_id,
            model_id,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn execute_tool_calls_with_hook(
        session: &mut Session,
        tool_registry: Arc<rocode_tool::ToolRegistry>,
        ctx: rocode_tool::ToolContext,
        provider: Arc<dyn Provider>,
        provider_id: &str,
        model_id: &str,
        update_hook: Option<&SessionUpdateHook>,
        agent_lookup: Option<Arc<dyn Fn(&str) -> Option<rocode_tool::TaskAgentInfo> + Send + Sync>>,
        ask_question_hook: Option<AskQuestionHook>,
    ) -> anyhow::Result<usize> {
        let Some(last_assistant_index) = session
            .messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant))
        else {
            return Ok(0);
        };

        let resolved_call_ids: HashSet<String> = session
            .messages
            .iter()
            .skip(last_assistant_index + 1)
            .flat_map(|m| m.parts.iter())
            .filter_map(|p| match &p.part_type {
                PartType::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect();

        let tool_calls: Vec<(String, String, serde_json::Value)> = session.messages
            [last_assistant_index]
            .parts
            .iter()
            .filter_map(|p| match &p.part_type {
                PartType::ToolCall {
                    id,
                    name,
                    input,
                    status,
                    raw,
                    state,
                    ..
                } if !resolved_call_ids.contains(id) && !name.trim().is_empty() => {
                    Self::tool_call_input_for_execution(
                        status,
                        input,
                        raw.as_deref(),
                        state.as_ref(),
                    )
                    .map(|args| (id.clone(), name.clone(), args))
                }
                _ => None,
            })
            .collect();

        if tool_calls.is_empty() {
            return Ok(0);
        }

        if let Some(assistant_msg) = session.messages.get_mut(last_assistant_index) {
            for (call_id, tool_name, input) in &tool_calls {
                Self::upsert_tool_call_part(
                    assistant_msg,
                    call_id,
                    Some(tool_name),
                    Some(input.clone()),
                    None,
                    Some(crate::ToolCallStatus::Running),
                    Some(crate::ToolState::Running {
                        input: input.clone(),
                        title: None,
                        metadata: None,
                        time: crate::RunningTime {
                            start: chrono::Utc::now().timestamp_millis(),
                        },
                    }),
                );
            }
        }

        // Emit update so TUI shows tools in "Running" state immediately.
        Self::emit_session_update(update_hook, session);

        let subsessions = Arc::new(Mutex::new(Self::load_persisted_subsessions(session)));
        let default_model = format!("{}:{}", provider_id, model_id);
        let ctx = Self::with_persistent_subsession_callbacks(
            ctx,
            subsessions.clone(),
            provider,
            tool_registry.clone(),
            default_model,
            agent_lookup,
            ask_question_hook,
        )
        .with_registry(tool_registry.clone());
        let available_tool_ids: HashSet<String> =
            tool_registry.list_ids().await.into_iter().collect();

        let mut executed_calls = 0usize;
        let tool_results_msg = {
            let mut msg = SessionMessage::tool(ctx.session_id.clone());
            for (call_id, tool_name, input) in tool_calls {
                tracing::info!(
                    tool_call_id = %call_id,
                    tool_name = %tool_name,
                    input_type = %if input.is_object() { "object" } else if input.is_string() { "string" } else { "other" },
                    input_keys = %if input.is_object() {
                        input.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default()
                    } else {
                        input.to_string().chars().take(120).collect::<String>()
                    },
                    "[DIAG] executing tool call"
                );
                let mut tool_ctx = ctx.clone();
                tool_ctx.call_id = Some(call_id.clone());
                let repaired_tool_name =
                    Self::repair_tool_call_name(&tool_name, &available_tool_ids);
                let mut effective_tool_name = repaired_tool_name.clone();
                let mut effective_input =
                    if repaired_tool_name == "invalid" && tool_name != "invalid" {
                        Self::invalid_tool_payload(
                            &tool_name,
                            &format!("Unknown tool requested by model: {}", tool_name),
                        )
                    } else {
                        input
                    };
                effective_input =
                    rocode_tool::normalize_tool_arguments(&effective_tool_name, effective_input);
                if effective_tool_name != "invalid" {
                    if let Some(payload) =
                        Self::prevalidate_tool_arguments(&effective_tool_name, &effective_input)
                    {
                        tracing::warn!(
                            tool_name = %tool_name,
                            normalized_tool = %effective_tool_name,
                            "tool arguments failed prevalidation, routing to invalid tool"
                        );
                        effective_tool_name = "invalid".to_string();
                        effective_input = payload;
                    }
                }

                let mut execution = tool_registry
                    .execute(
                        &effective_tool_name,
                        effective_input.clone(),
                        tool_ctx.clone(),
                    )
                    .await;

                if effective_tool_name != "invalid"
                    && available_tool_ids.contains("invalid")
                    && matches!(&execution, Err(rocode_tool::ToolError::InvalidArguments(_)))
                {
                    let validation_error = execution
                        .as_ref()
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "Invalid arguments".to_string());
                    tracing::info!(
                        tool_name = %tool_name,
                        error = %validation_error,
                        "tool call validation failed, routing to invalid tool"
                    );
                    effective_tool_name = "invalid".to_string();
                    effective_input = Self::invalid_tool_payload(&tool_name, &validation_error);
                    effective_input = rocode_tool::normalize_tool_arguments(
                        &effective_tool_name,
                        effective_input,
                    );
                    execution = tool_registry
                        .execute(
                            &effective_tool_name,
                            effective_input.clone(),
                            tool_ctx.clone(),
                        )
                        .await;
                }

                let (content, is_error, title, metadata, attachments, state_attachments) =
                    match execution {
                        Ok(result) => {
                            let mut metadata = result.metadata;
                            let (attachments, state_attachments) =
                                Self::extract_tool_attachments_from_metadata(
                                    &mut metadata,
                                    &ctx.session_id,
                                    &ctx.message_id,
                                );
                            (
                                result.output,
                                false,
                                Some(result.title),
                                Some(metadata),
                                attachments,
                                state_attachments,
                            )
                        }
                        Err(e) => (
                            format!("Error: {}", e),
                            true,
                            Some("Tool Error".to_string()),
                            None,
                            None,
                            None,
                        ),
                    };
                let history_input = Self::sanitize_tool_call_input_for_history(
                    &effective_tool_name,
                    &effective_input,
                    if is_error {
                        Some(content.as_str())
                    } else {
                        None
                    },
                );

                Self::push_tool_result_part(
                    &mut msg,
                    call_id.clone(),
                    content.clone(),
                    is_error,
                    title.clone(),
                    metadata.clone(),
                    attachments.clone(),
                );
                executed_calls += 1;

                if let Some(assistant_msg) = session.messages.get_mut(last_assistant_index) {
                    let now = chrono::Utc::now().timestamp_millis();
                    let next_state = if is_error {
                        crate::ToolState::Error {
                            input: history_input.clone(),
                            error: content.clone(),
                            metadata: None,
                            time: crate::ErrorTime {
                                start: now,
                                end: now,
                            },
                        }
                    } else {
                        crate::ToolState::Completed {
                            input: history_input.clone(),
                            output: content.clone(),
                            title: title.clone().unwrap_or_else(|| "Tool Result".to_string()),
                            metadata: metadata.clone().unwrap_or_default(),
                            time: crate::CompletedTime {
                                start: now,
                                end: now,
                                compacted: None,
                            },
                            attachments: state_attachments.clone(),
                        }
                    };
                    Self::upsert_tool_call_part(
                        assistant_msg,
                        &call_id,
                        Some(&effective_tool_name),
                        Some(history_input),
                        None,
                        Some(if is_error {
                            crate::ToolCallStatus::Error
                        } else {
                            crate::ToolCallStatus::Completed
                        }),
                        Some(next_state),
                    );
                }

                // Emit update after each tool completes so TUI renders results incrementally.
                Self::emit_session_update(update_hook, session);
            }
            msg
        };

        if !tool_results_msg.parts.is_empty() {
            session.messages.push(tool_results_msg);
        }
        let persisted = subsessions.lock().await.clone();
        Self::save_persisted_subsessions(session, &persisted);
        Ok(executed_calls)
    }

    pub(super) fn repair_tool_call_name(
        tool_name: &str,
        available_tool_ids: &HashSet<String>,
    ) -> String {
        if available_tool_ids.contains(tool_name) {
            return tool_name.to_string();
        }

        let lower = tool_name.to_ascii_lowercase();
        if lower != tool_name && available_tool_ids.contains(&lower) {
            tracing::info!(
                original = tool_name,
                repaired = %lower,
                "repairing tool call name via lowercase match"
            );
            return lower;
        }

        if available_tool_ids.contains("invalid") {
            tracing::warn!(
                tool_name = tool_name,
                "unknown tool call, routing to invalid tool"
            );
            return "invalid".to_string();
        }

        tool_name.to_string()
    }

    pub(super) fn mcp_tools_from_session(session: &Session) -> Vec<ToolDefinition> {
        session
            .metadata
            .get("mcp_tools")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let name = item.get("name").and_then(|v| v.as_str())?.to_string();
                        let description = item
                            .get("description")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let parameters = item
                            .get("parameters")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({"type":"object"}));
                        Some(ToolDefinition {
                            name,
                            description,
                            parameters,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn load_persisted_subsessions(
        session: &Session,
    ) -> HashMap<String, PersistedSubsession> {
        session
            .metadata
            .get("subsessions")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    pub(super) fn save_persisted_subsessions(
        session: &mut Session,
        subsessions: &HashMap<String, PersistedSubsession>,
    ) {
        if subsessions.is_empty() {
            session.metadata.remove("subsessions");
            return;
        }
        if let Ok(value) = serde_json::to_value(subsessions) {
            session.metadata.insert("subsessions".to_string(), value);
        }
    }

    pub(super) fn with_persistent_subsession_callbacks(
        ctx: rocode_tool::ToolContext,
        subsessions: Arc<Mutex<HashMap<String, PersistedSubsession>>>,
        provider: Arc<dyn Provider>,
        tool_registry: Arc<rocode_tool::ToolRegistry>,
        default_model: String,
        agent_lookup: Option<Arc<dyn Fn(&str) -> Option<rocode_tool::TaskAgentInfo> + Send + Sync>>,
        ask_question_hook: Option<AskQuestionHook>,
    ) -> rocode_tool::ToolContext {
        let parent_directory = ctx.directory.clone();
        let agent_lookup_for_subsessions = agent_lookup.clone();
        let ctx = if let Some(lookup) = agent_lookup {
            ctx.with_get_agent_info(move |name| {
                let lookup = lookup.clone();
                async move { Ok(lookup(&name)) }
            })
        } else {
            ctx
        };

        let ctx = if let Some(ref question_hook) = ask_question_hook {
            let session_id = ctx.session_id.clone();
            let question_hook = question_hook.clone();
            ctx.with_ask_question(move |questions| {
                let question_hook = question_hook.clone();
                let session_id = session_id.clone();
                async move { question_hook(session_id, questions).await }
            })
        } else {
            ctx
        };

        let ctx = ctx.with_get_last_model({
            let default_model = default_model.clone();
            move |_session_id| {
                let default_model = default_model.clone();
                async move { Ok(Some(default_model)) }
            }
        });

        let ctx = ctx.with_create_subsession({
            let subsessions = subsessions.clone();
            let parent_directory = parent_directory.clone();
            let agent_lookup = agent_lookup_for_subsessions.clone();
            move |agent, _title, model, disabled_tools| {
                let subsessions = subsessions.clone();
                let parent_directory = parent_directory.clone();
                let agent_lookup = agent_lookup.clone();
                async move {
                    let session_id = format!("task_{}_{}", agent, uuid::Uuid::new_v4().simple());
                    let max_steps = agent_lookup
                        .as_ref()
                        .and_then(|lookup| lookup(&agent))
                        .and_then(|info| info.steps);
                    let mut state = subsessions.lock().await;
                    state.insert(
                        session_id.clone(),
                        PersistedSubsession {
                            agent,
                            model,
                            max_steps,
                            directory: Some(parent_directory),
                            disabled_tools,
                            history: Vec::new(),
                        },
                    );
                    Ok(session_id)
                }
            }
        });

        ctx.with_prompt_subsession(move |session_id, prompt| {
            let subsessions = subsessions.clone();
            let provider = provider.clone();
            let tool_registry = tool_registry.clone();
            let default_model = default_model.clone();
            let parent_directory = parent_directory.clone();
            let ask_question_hook = ask_question_hook.clone();

            async move {
                let current = {
                    let state = subsessions.lock().await;
                    state.get(&session_id).cloned()
                }
                .ok_or_else(|| {
                    rocode_tool::ToolError::ExecutionError(format!(
                        "Unknown subagent session: {}. Start without task_id first.",
                        session_id
                    ))
                })?;

                let output = Self::execute_persisted_subsession_prompt(
                    &current,
                    &prompt,
                    provider,
                    tool_registry,
                    &default_model,
                    Some(parent_directory.as_str()),
                    ask_question_hook,
                    Some(session_id.clone()),
                )
                .await
                .map_err(|e| rocode_tool::ToolError::ExecutionError(e.to_string()))?;

                let mut state = subsessions.lock().await;
                if let Some(existing) = state.get_mut(&session_id) {
                    existing.history.push(PersistedSubsessionTurn {
                        prompt,
                        output: output.clone(),
                    });
                }
                Ok(output)
            }
        })
    }

    pub(super) async fn execute_persisted_subsession_prompt(
        subsession: &PersistedSubsession,
        prompt: &str,
        provider: Arc<dyn Provider>,
        tool_registry: Arc<rocode_tool::ToolRegistry>,
        default_model: &str,
        fallback_directory: Option<&str>,
        ask_question_hook: Option<AskQuestionHook>,
        question_session_id: Option<String>,
    ) -> anyhow::Result<String> {
        let model = Self::resolve_subsession_model(
            subsession.model.as_deref(),
            default_model,
            provider.id(),
        );

        let composed_prompt = Self::compose_subsession_prompt(&subsession.history, prompt);
        let working_directory = subsession
            .directory
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .or_else(|| fallback_directory.map(str::trim).filter(|d| !d.is_empty()));
        let mut executor =
            SubtaskExecutor::new(&subsession.agent, &composed_prompt).with_model(model);
        if let Some(directory) = working_directory {
            executor = executor.with_working_directory(directory);
        }
        if let Some(question_hook) = ask_question_hook {
            let session_id = question_session_id.unwrap_or_else(|| "subtask".to_string());
            executor = executor.with_ask_question_hook(question_hook, session_id);
        }
        executor = executor.with_max_steps(subsession.max_steps);
        executor.agent_params = AgentParams {
            max_tokens: Some(2048),
            temperature: Some(0.2),
            top_p: None,
        };

        executor
            .execute_inline(provider, &tool_registry, &subsession.disabled_tools)
            .await
    }

    pub(super) fn resolve_subsession_model(
        requested_model: Option<&str>,
        default_model: &str,
        current_provider_id: &str,
    ) -> ModelRef {
        let mut model = Self::parse_model_string(requested_model.unwrap_or(default_model));
        if model.provider_id == "default" && model.model_id == "default" {
            model = Self::parse_model_string(default_model);
        }

        // Subsession execution reuses the parent provider object.
        // If a subagent model comes from another provider namespace (for example
        // plugin config like "opencode/big-pickle"), running it against the
        // current provider causes model-not-found errors. Fallback to the
        // parent's default model in that mismatch case.
        if model.provider_id != "default" && model.provider_id != current_provider_id {
            tracing::warn!(
                requested_provider = %model.provider_id,
                requested_model = %model.model_id,
                current_provider = %current_provider_id,
                fallback_model = %default_model,
                "subsession model provider differs from current provider; falling back to default model"
            );
            return Self::parse_model_string(default_model);
        }

        model
    }

    pub(super) fn parse_model_string(raw: &str) -> ModelRef {
        if let Some((provider_id, model_id)) = raw.split_once(':').or_else(|| raw.split_once('/')) {
            return ModelRef {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
            };
        }
        if raw.is_empty() {
            return ModelRef {
                provider_id: "default".to_string(),
                model_id: "default".to_string(),
            };
        }
        ModelRef {
            provider_id: "default".to_string(),
            model_id: raw.to_string(),
        }
    }

    pub(super) fn compose_subsession_prompt(
        history: &[PersistedSubsessionTurn],
        prompt: &str,
    ) -> String {
        if history.is_empty() {
            return prompt.to_string();
        }

        let history_text = history
            .iter()
            .rev()
            .take(8)
            .rev()
            .map(|turn| format!("User:\n{}\n\nAssistant:\n{}", turn.prompt, turn.output))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        format!(
            "Continue this subtask session.\n\nPrevious conversation:\n{}\n\nNew request:\n{}",
            history_text, prompt
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use std::collections::HashSet;

    #[test]
    fn persisted_subsessions_roundtrip_via_session_metadata() {
        let mut session = Session::new("proj", ".");
        let mut map = HashMap::new();
        map.insert(
            "task_explore_1".to_string(),
            PersistedSubsession {
                agent: "explore".to_string(),
                model: Some("anthropic:claude".to_string()),
                max_steps: Some(12),
                directory: Some("/tmp/project".to_string()),
                disabled_tools: vec!["task".to_string()],
                history: vec![PersistedSubsessionTurn {
                    prompt: "Inspect src".to_string(),
                    output: "Done".to_string(),
                }],
            },
        );

        SessionPrompt::save_persisted_subsessions(&mut session, &map);
        let loaded = SessionPrompt::load_persisted_subsessions(&session);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["task_explore_1"].agent, "explore");
        assert_eq!(loaded["task_explore_1"].history.len(), 1);
    }

    #[test]
    fn parse_model_string_supports_provider_prefix() {
        let model = SessionPrompt::parse_model_string("openai:gpt-4o");
        assert_eq!(model.provider_id, "openai");
        assert_eq!(model.model_id, "gpt-4o");
    }

    #[test]
    fn resolve_subsession_model_falls_back_on_provider_mismatch() {
        let model = SessionPrompt::resolve_subsession_model(
            Some("opencode:big-pickle"),
            "zhipuai-coding-plan:glm-4.6",
            "zhipuai-coding-plan",
        );
        assert_eq!(model.provider_id, "zhipuai-coding-plan");
        assert_eq!(model.model_id, "glm-4.6");
    }

    #[test]
    fn resolve_subsession_model_keeps_same_provider_model() {
        let model = SessionPrompt::resolve_subsession_model(
            Some("zhipuai-coding-plan:GLM-5"),
            "zhipuai-coding-plan:glm-4.6",
            "zhipuai-coding-plan",
        );
        assert_eq!(model.provider_id, "zhipuai-coding-plan");
        assert_eq!(model.model_id, "GLM-5");
    }

    #[test]
    fn compose_subsession_prompt_includes_recent_history() {
        let history = vec![PersistedSubsessionTurn {
            prompt: "Find files".to_string(),
            output: "Found 10 files".to_string(),
        }];
        let composed = SessionPrompt::compose_subsession_prompt(&history, "Continue");
        assert!(composed.contains("Previous conversation"));
        assert!(composed.contains("Find files"));
        assert!(composed.contains("Continue"));
    }

    #[test]
    fn repair_tool_call_name_keeps_exact_match() {
        let tools = HashSet::from([
            "read".to_string(),
            "glob".to_string(),
            "invalid".to_string(),
        ]);
        assert_eq!(SessionPrompt::repair_tool_call_name("read", &tools), "read");
    }

    #[test]
    fn repair_tool_call_name_repairs_case_mismatch() {
        let tools = HashSet::from([
            "read".to_string(),
            "glob".to_string(),
            "invalid".to_string(),
        ]);
        assert_eq!(SessionPrompt::repair_tool_call_name("Read", &tools), "read");
    }

    #[test]
    fn repair_tool_call_name_routes_unknown_to_invalid() {
        let tools = HashSet::from([
            "read".to_string(),
            "glob".to_string(),
            "invalid".to_string(),
        ]);
        assert_eq!(
            SessionPrompt::repair_tool_call_name("read_html_file", &tools),
            "invalid"
        );
    }

    #[test]
    fn mcp_tools_from_session_reads_runtime_metadata() {
        let mut session = Session::new("proj", ".");
        session.metadata.insert(
            "mcp_tools".to_string(),
            serde_json::json!([{
                "name": "repo_search",
                "description": "Search repository",
                "parameters": {"type":"object","properties":{"q":{"type":"string"}}}
            }]),
        );

        let tools = SessionPrompt::mcp_tools_from_session(&session);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "repo_search");
    }
}
