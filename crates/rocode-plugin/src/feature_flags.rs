//! Runtime feature flags for plugin optimizations.
//!
//! Each flag defaults to `true` (enabled). Use [`set`] to toggle at runtime
//! for rollback without redeployment.

use std::sync::atomic::{AtomicBool, Ordering};

static SEQ_HOOKS: AtomicBool = AtomicBool::new(true);
static TIMEOUT_SELF_HEAL: AtomicBool = AtomicBool::new(true);
static CIRCUIT_BREAKER: AtomicBool = AtomicBool::new(true);
static LARGE_PAYLOAD_FILE: AtomicBool = AtomicBool::new(true);

/// Check whether a named feature flag is enabled.
///
/// Known flags:
/// - `"plugin_seq_hooks"` — sequential hook execution with per-hook timing
/// - `"plugin_timeout_self_heal"` — auto-reconnect on RPC timeout
/// - `"plugin_circuit_breaker"` — circuit breaker around subprocess hooks
/// - `"plugin_large_payload_file_ipc"` — file-based IPC for large payloads
pub fn is_enabled(flag: &str) -> bool {
    match flag {
        "plugin_seq_hooks" => SEQ_HOOKS.load(Ordering::Relaxed),
        "plugin_timeout_self_heal" => TIMEOUT_SELF_HEAL.load(Ordering::Relaxed),
        "plugin_circuit_breaker" => CIRCUIT_BREAKER.load(Ordering::Relaxed),
        "plugin_large_payload_file_ipc" => LARGE_PAYLOAD_FILE.load(Ordering::Relaxed),
        _ => false,
    }
}

/// Set a feature flag at runtime. Unknown flag names are silently ignored.
pub fn set(flag: &str, enabled: bool) {
    match flag {
        "plugin_seq_hooks" => SEQ_HOOKS.store(enabled, Ordering::Relaxed),
        "plugin_timeout_self_heal" => TIMEOUT_SELF_HEAL.store(enabled, Ordering::Relaxed),
        "plugin_circuit_breaker" => CIRCUIT_BREAKER.store(enabled, Ordering::Relaxed),
        "plugin_large_payload_file_ipc" => LARGE_PAYLOAD_FILE.store(enabled, Ordering::Relaxed),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_flags_enabled_by_default() {
        assert!(is_enabled("plugin_seq_hooks"));
        assert!(is_enabled("plugin_timeout_self_heal"));
        assert!(is_enabled("plugin_circuit_breaker"));
        assert!(is_enabled("plugin_large_payload_file_ipc"));
    }

    #[test]
    fn unknown_flag_returns_false() {
        assert!(!is_enabled("nonexistent_flag"));
    }

    #[test]
    fn set_toggles_flag() {
        // Disable and re-enable to avoid polluting other tests
        set("plugin_seq_hooks", false);
        assert!(!is_enabled("plugin_seq_hooks"));
        set("plugin_seq_hooks", true);
        assert!(is_enabled("plugin_seq_hooks"));
    }
}
