use futures::StreamExt;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use rocode_agent::{AgentExecutor, AgentInfo, AgentRegistry};
use rocode_command::{CommandContext, CommandRegistry};
use rocode_config::loader::load_config;
use rocode_provider::StreamEvent;
use rocode_session::system::{EnvironmentContext, SystemPrompt};
use rocode_tool::registry::create_default_registry;

use crate::cli::RunOutputFormat;
use crate::providers::{
    list_models_interactive, list_providers_interactive, select_model, setup_providers, show_help,
};
use crate::remote::run_non_interactive_attach;
use crate::util::{append_cli_file_attachments, collect_run_input, parse_model_and_provider};

pub(crate) async fn run_non_interactive(
    message: Vec<String>,
    command: Option<String>,
    continue_last: bool,
    session: Option<String>,
    fork: bool,
    share: bool,
    model: Option<String>,
    agent_name: String,
    files: Vec<PathBuf>,
    format: RunOutputFormat,
    title: Option<String>,
    attach: Option<String>,
    dir: Option<PathBuf>,
    _port: Option<u16>,
    variant: Option<String>,
    _thinking: bool,
) -> anyhow::Result<()> {
    if let Some(dir) = dir {
        std::env::set_current_dir(&dir).map_err(|e| {
            anyhow::anyhow!("Failed to change directory to {}: {}", dir.display(), e)
        })?;
    }

    if fork && !continue_last && session.is_none() {
        anyhow::bail!("--fork requires --continue or --session");
    }

    let mut input = collect_run_input(message)?;
    append_cli_file_attachments(&mut input, &files)?;

    if let Some(base_url) = attach {
        return run_non_interactive_attach(
            base_url,
            input,
            command,
            continue_last,
            session,
            fork,
            share,
            model,
            variant,
            format,
            title,
        )
        .await;
    }

    if continue_last || session.is_some() || fork || share {
        eprintln!(
            "Note: session/share flags are currently applied when using `run --attach <server>`."
        );
    }

    if let Some(command_name) = command {
        let cwd = std::env::current_dir()?;
        let mut registry = CommandRegistry::new();
        let _ = registry.load_from_directory(&cwd);
        let args = if input.trim().is_empty() {
            Vec::new()
        } else {
            input
                .split_whitespace()
                .map(|part| part.to_string())
                .collect::<Vec<_>>()
        };
        let rendered =
            registry.execute(&command_name, CommandContext::new(cwd).with_arguments(args))?;
        input = rendered;
    }

    if input.trim().is_empty() {
        let (provider, model_id) = parse_model_and_provider(model);
        return run_chat_session(model_id, provider, agent_name, None, false).await;
    }

    let (provider, model_id) = parse_model_and_provider(model);
    run_chat_session(model_id, provider, agent_name, Some(input.clone()), true).await?;

    if matches!(format, RunOutputFormat::Json) {
        println!(
            "{}",
            serde_json::json!({
                "type": "completed",
                "timestamp": chrono::Utc::now().timestamp_millis(),
                "input": input
            })
        );
    }

    Ok(())
}

async fn run_chat_session(
    model: Option<String>,
    provider: Option<String>,
    agent_name: String,
    initial_prompt: Option<String>,
    single_shot: bool,
) -> anyhow::Result<()> {
    let current_dir = std::env::current_dir()?;
    let config = load_config(&current_dir)?;

    let provider_registry = Arc::new(setup_providers(&config).await?);

    if provider_registry.list().is_empty() {
        eprintln!("Error: No API keys configured.");
        eprintln!("Set one of the following environment variables:");
        eprintln!("  - ANTHROPIC_API_KEY");
        eprintln!("  - OPENAI_API_KEY");
        eprintln!("  - OPENROUTER_API_KEY");
        eprintln!("  - GOOGLE_API_KEY");
        eprintln!("  - MISTRAL_API_KEY");
        eprintln!("  - GROQ_API_KEY");
        eprintln!("  - XAI_API_KEY");
        eprintln!("  - DEEPSEEK_API_KEY");
        eprintln!("  - COHERE_API_KEY");
        eprintln!("  - TOGETHER_API_KEY");
        eprintln!("  - PERPLEXITY_API_KEY");
        eprintln!("  - CEREBRAS_API_KEY");
        eprintln!("  - DEEPINFRA_API_KEY");
        eprintln!("  - VERCEL_API_KEY");
        eprintln!("  - GITLAB_TOKEN");
        eprintln!("  - GITHUB_COPILOT_TOKEN");
        eprintln!("  - GOOGLE_VERTEX_API_KEY + GOOGLE_VERTEX_PROJECT_ID + GOOGLE_VERTEX_LOCATION");
        std::process::exit(1);
    }

    let tool_registry = Arc::new(create_default_registry().await);

    let agent_registry = AgentRegistry::from_config(&config);
    let mut agent_info = agent_registry
        .get(&agent_name)
        .cloned()
        .unwrap_or_else(|| AgentInfo::build());

    if let Some(ref model_id) = model {
        let provider_id = provider.clone().unwrap_or_else(|| {
            if model_id.starts_with("claude") {
                "anthropic".to_string()
            } else {
                "openai".to_string()
            }
        });
        agent_info = agent_info.with_model(model_id.clone(), provider_id);
    }

    println!("\n╔══════════════════════════════════════════╗");
    println!("║        OpenCode Interactive Mode         ║");
    println!("╚══════════════════════════════════════════╝");
    println!();
    println!("  Model: {}", model.as_deref().unwrap_or("auto"));
    println!("  Agent: {}", agent_name);
    println!("  Directory: {}", current_dir.display());
    println!();
    println!("  Commands: exit, quit, help, clear");
    println!();
    let mut executor =
        AgentExecutor::new(agent_info.clone(), provider_registry.clone(), tool_registry);

    // Build model-specific system prompt + environment context (TS parity: SystemPrompt.provider + SystemPrompt.environment)
    {
        let (model_api_id, provider_id) = match &agent_info.model {
            Some(m) => (m.model_id.clone(), m.provider_id.clone()),
            None => (
                "claude-sonnet-4-20250514".to_string(),
                "anthropic".to_string(),
            ),
        };
        let model_prompt = SystemPrompt::for_model(&model_api_id);
        let env_ctx = EnvironmentContext::from_current(
            &model_api_id,
            &provider_id,
            current_dir.to_string_lossy().as_ref(),
        );
        let env_prompt = SystemPrompt::environment(&env_ctx);
        let full_prompt = format!("{}\n\n{}", model_prompt, env_prompt);
        executor = executor.with_system_prompt(full_prompt);
    }

    if let Some(prompt_text) = initial_prompt {
        println!("User: {}", prompt_text);
        process_message(&mut executor, &prompt_text).await?;
        if single_shot {
            return Ok(());
        }
    }

    let stdin = io::stdin();

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        if input == "exit" || input == "quit" {
            println!("\nGoodbye!");
            break;
        }

        if input == "help" {
            show_help();
            continue;
        }

        if input == "clear" {
            println!("Conversation cleared.\n");
            continue;
        }

        if input == "/models" || input == "models" {
            list_models_interactive(&provider_registry);
            continue;
        }

        if input.starts_with("/model ") {
            let model_id = input.strip_prefix("/model ").unwrap().trim();
            if let Err(e) = select_model(&mut executor, model_id, &provider_registry) {
                eprintln!("Error selecting model: {}", e);
            }
            continue;
        }

        if input == "/providers" || input == "providers" {
            list_providers_interactive(&provider_registry);
            continue;
        }

        if input == "stats" {
            println!("Messages: {}", executor.conversation().messages.len());
            println!();
            continue;
        }

        match process_message(&mut executor, input).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("\nError: {}", e);
            }
        }
    }

    Ok(())
}

async fn process_message(executor: &mut AgentExecutor, input: &str) -> anyhow::Result<()> {
    print!("\nAssistant: ");
    io::stdout().flush()?;

    let stream = executor.execute_streaming(input.to_string()).await?;

    let mut stream = std::pin::pin!(stream);
    let mut full_response = String::new();

    while let Some(event) = stream.next().await {
        match event {
            Ok(StreamEvent::TextDelta(text)) => {
                print!("{}", text);
                full_response.push_str(&text);
                io::stdout().flush()?;
            }
            Ok(StreamEvent::ToolCallStart { id: _, name }) => {
                println!("\n[Calling tool: {}]", name);
            }
            Ok(StreamEvent::ToolCallDelta { .. }) => {}
            Ok(StreamEvent::Done) => {
                break;
            }
            Ok(StreamEvent::Error(e)) => {
                eprintln!("\nError: {}", e);
                break;
            }
            Err(e) => {
                eprintln!("\nError: {}", e);
                break;
            }
            _ => {}
        }
    }

    println!("\n");
    Ok(())
}
