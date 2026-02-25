use std::collections::HashMap;

use rocode_agent::AgentExecutor;
use rocode_plugin::init_global;
use rocode_plugin::subprocess::{PluginContext, PluginLoader};
use rocode_provider::{
    bootstrap_config_from_raw, create_registry_from_bootstrap_config, AuthInfo,
    ConfigModel as BootstrapConfigModel, ConfigProvider as BootstrapConfigProvider,
    ProviderRegistry,
};

pub(crate) fn list_models_interactive(registry: &ProviderRegistry) {
    println!("\nAvailable Models:\n");
    for provider in registry.list() {
        println!("  [{}]", provider.id());
        for model in provider.models() {
            println!("    {}", model.id);
        }
        println!();
    }
    println!("Use /model <model_id> to select a model");
    println!();
}

pub(crate) fn list_providers_interactive(registry: &ProviderRegistry) {
    println!("\nConfigured Providers:\n");
    for provider in registry.list() {
        let models_count = provider.models().len();
        println!("  {} - {} model(s)", provider.id(), models_count);
    }
    println!();
}

pub(crate) fn select_model(
    _executor: &mut AgentExecutor,
    model_id: &str,
    registry: &ProviderRegistry,
) -> anyhow::Result<()> {
    let model = registry
        .list()
        .iter()
        .flat_map(|p| p.models())
        .find(|m| m.id == model_id)
        .ok_or_else(|| anyhow::anyhow!("Model not found: {}", model_id))?;

    println!("Selected model: {} ({})\n", model_id, model.name);
    Ok(())
}

const DEFAULT_PLUGIN_SERVER_URL: &str = "http://127.0.0.1:4096";

pub(crate) async fn setup_providers(config: &rocode_config::Config) -> anyhow::Result<ProviderRegistry> {
    let auth_store = load_plugin_auth_store(config).await;

    // Convert config providers to bootstrap format
    let bootstrap_providers = convert_config_providers(config);
    let bootstrap_config = bootstrap_config_from_raw(
        bootstrap_providers,
        config.disabled_providers.clone(),
        config.enabled_providers.clone(),
        config.model.clone(),
        config.small_model.clone(),
    );

    Ok(create_registry_from_bootstrap_config(
        &bootstrap_config,
        &auth_store,
    ))
}

/// Convert rocode_config::ProviderConfig map to bootstrap ConfigProvider map.
fn convert_config_providers(
    config: &rocode_config::Config,
) -> std::collections::HashMap<String, BootstrapConfigProvider> {
    let Some(ref providers) = config.provider else {
        return std::collections::HashMap::new();
    };

    providers
        .iter()
        .map(|(id, p)| (id.clone(), provider_to_bootstrap(p)))
        .collect()
}

fn provider_to_bootstrap(provider: &rocode_config::ProviderConfig) -> BootstrapConfigProvider {
    let mut options = provider.options.clone().unwrap_or_default();
    if let Some(api_key) = &provider.api_key {
        options
            .entry("apiKey".to_string())
            .or_insert_with(|| serde_json::Value::String(api_key.clone()));
    }
    if let Some(base_url) = &provider.base_url {
        options
            .entry("baseURL".to_string())
            .or_insert_with(|| serde_json::Value::String(base_url.clone()));
    }

    let models = provider.models.as_ref().map(|models| {
        models
            .iter()
            .map(|(id, model)| (id.clone(), model_to_bootstrap(id, model)))
            .collect()
    });

    BootstrapConfigProvider {
        name: provider.name.clone(),
        api: provider.base_url.clone(),
        npm: provider.npm.clone(),
        options: (!options.is_empty()).then_some(options),
        models,
        blacklist: (!provider.blacklist.is_empty()).then_some(provider.blacklist.clone()),
        whitelist: (!provider.whitelist.is_empty()).then_some(provider.whitelist.clone()),
        ..Default::default()
    }
}

fn model_to_bootstrap(id: &str, model: &rocode_config::ModelConfig) -> BootstrapConfigModel {
    let mut options = HashMap::new();
    if let Some(api_key) = &model.api_key {
        options.insert(
            "apiKey".to_string(),
            serde_json::Value::String(api_key.clone()),
        );
    }

    let variants = model.variants.as_ref().map(|variants| {
        variants
            .iter()
            .map(|(name, variant)| (name.clone(), variant_to_bootstrap(variant)))
            .collect()
    });

    BootstrapConfigModel {
        id: model.model.clone().or_else(|| Some(id.to_string())),
        name: model.name.clone(),
        provider: model.base_url.as_ref().map(|url| {
            rocode_provider::bootstrap::ConfigModelProvider {
                api: Some(url.clone()),
                npm: None,
            }
        }),
        options: (!options.is_empty()).then_some(options),
        variants,
        ..Default::default()
    }
}

fn variant_to_bootstrap(
    variant: &rocode_config::ModelVariantConfig,
) -> HashMap<String, serde_json::Value> {
    let mut values = variant.extra.clone();
    if let Some(disabled) = variant.disabled {
        values.insert("disabled".to_string(), serde_json::Value::Bool(disabled));
    }
    values
}

async fn load_plugin_auth_store(config: &rocode_config::Config) -> HashMap<String, AuthInfo> {
    let loader = match PluginLoader::new() {
        Ok(loader) => loader,
        Err(error) => {
            tracing::warn!(%error, "failed to initialize plugin loader in CLI");
            return HashMap::new();
        }
    };
    init_global(loader.hook_system());

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            tracing::warn!(%error, "failed to get cwd for plugin loader context");
            return HashMap::new();
        }
    };
    let directory = cwd.to_string_lossy().to_string();
    let server_url =
        std::env::var("OPENCODE_SERVER_URL").unwrap_or_else(|_| DEFAULT_PLUGIN_SERVER_URL.into());
    let context = PluginContext {
        worktree: directory.clone(),
        directory,
        server_url,
    };

    if let Err(error) = loader.load_builtins(&context).await {
        tracing::warn!(%error, "failed to load builtin auth plugins in CLI");
    }

    if !config.plugin.is_empty() {
        if let Err(error) = loader.load_all(&config.plugin, &context).await {
            tracing::warn!(%error, "failed to load configured plugins in CLI");
        }
    }

    let mut auth_store = HashMap::new();
    for (provider_id, bridge) in loader.auth_bridges().await {
        match bridge.load().await {
            Ok(result) => {
                if let Some(api_key) = result.api_key {
                    auth_store.insert(
                        provider_id.clone(),
                        AuthInfo::Api {
                            key: api_key.clone(),
                        },
                    );
                    if provider_id == "github-copilot" {
                        auth_store.insert(
                            "github-copilot-enterprise".to_string(),
                            AuthInfo::Api { key: api_key },
                        );
                    }
                }
            }
            Err(error) => {
                tracing::warn!(provider = provider_id, %error, "failed to load plugin auth in CLI");
            }
        }
    }

    auth_store
}

pub(crate) fn show_help() {
    println!();
    println!("Available commands:");
    println!("  exit, quit   - End the session");
    println!("  help         - Show this help message");
    println!("  clear        - Clear conversation history");
    println!("  stats        - Show session statistics");
    println!();
    println!("Model commands:");
    println!("  /models      - List all available models");
    println!("  /model <id>  - Switch to a specific model");
    println!("  /providers   - List configured providers");
    println!();
    println!("Tips:");
    println!("  - Use --model to specify a model (e.g., --model claude-sonnet-4)");
    println!("  - Use --provider to specify a provider (anthropic, openai)");
    println!("  - Use --prompt to send an initial message");
    println!();
}
