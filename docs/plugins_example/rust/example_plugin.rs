//! Example Rust-side plugin (compile-time integration).
//!
//! NOTE:
//! - This is NOT dynamically loaded like TS plugins.
//! - You must wire it into your Rust startup path and rebuild.

use std::future::Future;
use std::pin::Pin;

use rocode_plugin::{
    Hook, HookContext, HookEvent, HookOutput, Plugin, PluginSystem,
};

pub struct ExampleRustPlugin;

impl Plugin for ExampleRustPlugin {
    fn name(&self) -> &str {
        "example-rust-plugin"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn register_hooks(
        &self,
        system: &PluginSystem,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            system
                .register(Hook::new(
                    "example.chat.headers",
                    HookEvent::ChatHeaders,
                    |ctx: HookContext| async move {
                        let mut headers = serde_json::Map::new();
                        headers.insert(
                            "x-rocode-rust-plugin".to_string(),
                            serde_json::Value::String("example-rust-plugin".to_string()),
                        );

                        let payload = serde_json::json!({
                            "headers": headers,
                            "sessionID": ctx.session_id,
                        });
                        Ok::<HookOutput, rocode_plugin::HookError>(
                            HookOutput::with_payload(payload),
                        )
                    },
                ))
                .await;
        })
    }
}

// Example of manual registration point:
//
// let registry = rocode_plugin::PluginRegistry::new();
// registry.register(std::sync::Arc::new(ExampleRustPlugin)).await;
