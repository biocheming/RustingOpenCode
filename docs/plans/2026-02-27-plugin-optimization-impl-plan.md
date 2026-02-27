# Plugin System Optimization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Align rocode plugin hook semantics with TS opencode, add stability mechanisms (circuit breaker, self-heal), and reduce IPC overhead for large payloads.

**Architecture:** Four milestones executed sequentially: M0 (observability baseline), M1 (semantic alignment), M2 (stability/self-heal), M3 (transport optimization). Each milestone has a feature flag for independent rollback.

**Tech Stack:** Rust (rocode-plugin, rocode-session, rocode-tool crates), TypeScript (plugin-host.ts), tokio async runtime, JSON-RPC over stdin/stdout.

---

## Task 1: M0 — Hook perf instrumentation in lib.rs

**Files:**
- Modify: `crates/rocode-plugin/src/lib.rs:216-256`

**Step 1: Write the failing test**

```rust
// In crates/rocode-plugin/src/lib.rs, add to existing #[cfg(test)] mod tests
#[tokio::test]
async fn trigger_logs_hook_duration() {
    // Register a hook that sleeps 10ms
    let system = PluginSystem::new();
    system.register(Hook::new("test:slow", HookEvent::ConfigLoaded, |_ctx| {
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            Ok(HookOutput::empty())
        }
    })).await;

    let ctx = HookContext::new(HookEvent::ConfigLoaded);
    // Clear cache so hook actually runs
    system.cache.write().await.clear();
    let results = system.trigger(ctx).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok());
    // Duration logging is verified by tracing subscriber in integration tests
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rocode-plugin -- trigger_logs_hook_duration -v`
Expected: FAIL (test doesn't exist yet)

**Step 3: Implement hook perf logging**

Replace the `trigger` method body (lines 216-256) with sequential execution + timing:

```rust
pub async fn trigger(&self, context: HookContext) -> Vec<HookResult> {
    let hooks = self.hooks.read().await;

    let hook_list = match hooks.get(&context.event) {
        Some(list) => list.clone(),
        None => return vec![],
    };

    let enabled: Vec<_> = hook_list.iter().filter(|h| h.enabled).cloned().collect();
    if enabled.is_empty() {
        return vec![];
    }

    // Check cache for deterministic events.
    if CACHEABLE_EVENTS.contains(&context.event) {
        let data_hash = context_data_hash(&context.data);
        let cache = self.cache.read().await;
        if let Some(cached) = cache.get(&(context.event.clone(), data_hash)) {
            return cached.clone();
        }
    }

    // Drop the read lock before awaiting.
    drop(hooks);

    // Execute hooks sequentially (TS parity: for-loop, not join_all).
    let mut results: Vec<HookResult> = Vec::with_capacity(enabled.len());
    for hook in &enabled {
        let start = std::time::Instant::now();
        let result = (hook.handler)(context.clone()).await;
        let elapsed = start.elapsed();
        let status = if result.is_ok() { "ok" } else { "err" };
        tracing::debug!(
            event = ?context.event,
            hook_id = %hook.id,
            duration_ms = elapsed.as_millis() as u64,
            status = status,
            "[plugin-perf] hook executed"
        );
        results.push(result);
    }

    // Cache results for deterministic events.
    if CACHEABLE_EVENTS.contains(&context.event) {
        let data_hash = context_data_hash(&context.data);
        let mut cache = self.cache.write().await;
        cache.insert((context.event.clone(), data_hash), results.clone());
    }

    results
}
```

This change also accomplishes Task 3 (M1 P0.3 — sequential execution) in the same edit.

**Step 4: Run test to verify it passes**

Run: `cargo test -p rocode-plugin -- trigger_logs_hook_duration -v`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/rocode-plugin/src/lib.rs
git commit -m "feat(plugin): sequential hook execution with perf logging

Replaces join_all parallel execution with sequential for-loop (TS parity).
Adds [plugin-perf] tracing for each hook: event, hook_id, duration_ms, status."
```

---

## Task 2: M0 — Hook sequence logging in prompt loop

**Files:**
- Modify: `crates/rocode-session/src/prompt/mod.rs:757`
- Modify: `crates/rocode-tool/src/registry.rs:317-376`

**Step 1: Add sequence counter to prompt loop**

In `loop_inner` at line 757, before the `chat.messages.transform` hook:

```rust
tracing::info!(
    step = step,
    session_id = %session_id,
    message_count = filtered_messages.len(),
    "[plugin-seq] prompt loop step start"
);
```

**Step 2: Add sequence logging around tool hook triggers in registry.rs**

In `registry.rs`, wrap the existing `ToolExecuteBefore` trigger (line 325) and `ToolExecuteAfter` trigger (line 376):

```rust
// Before line 325:
tracing::debug!(
    tool = %tool_id,
    "[plugin-seq] tool.execute.before"
);

// Before line 376:
tracing::debug!(
    tool = %tool_id,
    "[plugin-seq] tool.execute.after"
);
```

**Step 3: Run check**

Run: `cargo check -p rocode-session -p rocode-tool`
Expected: no errors

**Step 4: Commit**

```bash
git add crates/rocode-session/src/prompt/mod.rs crates/rocode-tool/src/registry.rs
git commit -m "feat(plugin): add [plugin-seq] hook sequence logging"
```

---

## Task 3: M1 — chat.message trigger timing (move to assistant finalization)

**Files:**
- Modify: `crates/rocode-session/src/prompt/mod.rs:470-493` (remove)
- Modify: `crates/rocode-session/src/prompt/mod.rs:~1554` (add)

**Step 1: Write the failing test**

```rust
// In crates/rocode-session/src/prompt/mod.rs tests section
#[test]
fn chat_message_hook_not_triggered_on_user_message_creation() {
    // This test verifies the old behavior is removed.
    // The ChatMessage hook should NOT fire during create_user_message.
    // We verify by checking that create_user_message no longer contains
    // HookEvent::ChatMessage references (structural test via grep).
    // Actual hook timing is verified in integration tests.
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rocode-session -- chat_message_hook_not_triggered -v`
Expected: FAIL (ChatMessage is still in create_user_message)

**Step 3: Remove ChatMessage hook from create_user_message**

Delete lines 463-493 in `mod.rs` (the entire `if let Some(user_message)` block that triggers `ChatMessage`).

**Step 4: Add ChatMessage hook after assistant finalization**

At line ~1554 (after `session.touch()`, before `Self::emit_session_update`), insert:

```rust
// Plugin hook: chat.message — triggered after assistant message is finalized.
// TS parity: opencode/packages/opencode/src/session/prompt.ts:1295
if let Some(assistant_msg) = session.messages.get(assistant_index).cloned() {
    let has_tool_calls = Self::has_unresolved_tool_calls(&assistant_msg);
    let mut hook_ctx = HookContext::new(HookEvent::ChatMessage)
        .with_session(&session_id)
        .with_data("message_id", serde_json::json!(&assistant_msg.id))
        .with_data("message", session_message_hook_payload(&assistant_msg))
        .with_data("parts", serde_json::json!(&assistant_msg.parts))
        .with_data("has_tool_calls", serde_json::json!(has_tool_calls));

    if let Some(model) = provider.get_model(&model_id) {
        hook_ctx = hook_ctx.with_data("model", serde_json::json!({
            "id": model.id,
            "name": model.name,
            "provider": model.provider,
        }));
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
```

**Step 5: Run test to verify it passes**

Run: `cargo test -p rocode-session -- chat_message_hook_not_triggered -v`
Expected: PASS

**Step 6: Run full test suite**

Run: `cargo test -p rocode-session`
Expected: all tests pass

**Step 7: Commit**

```bash
git add crates/rocode-session/src/prompt/mod.rs
git commit -m "feat(plugin): move chat.message hook to assistant finalization

Aligns with TS opencode prompt.ts:1295. ChatMessage now fires after
assistant message is fully finalized (metadata, tool call promotion done),
not during user message creation."
```

---

## Task 4: M1 — Verify tool.execute.before/after I/O mapping

**Files:**
- Read: `crates/rocode-plugin/src/subprocess/loader.rs:463-478`
- Read: `crates/rocode-tool/src/registry.rs:318-330`

**Step 1: Verify current mapping is correct**

The `hook_io_from_context` at `loader.rs:463-468` already correctly maps:
- `ToolExecuteBefore`: input=`{tool, sessionID, callID}`, output=`{args}`
- `ToolExecuteAfter`: input=`{tool, sessionID, callID, args, error}`, output=`{title, output, metadata}`

This matches TS semantics. **No code change needed.**

**Step 2: Verify call_id is passed from registry.rs**

Check `registry.rs:322-324`:
```rust
if let Some(call_id) = &ctx.call_id {
    before_hook_ctx = before_hook_ctx.with_data("call_id", serde_json::json!(call_id));
}
```

The key is `call_id` but `loader.rs:466` expects `callID`. Check if `copy_first` handles this alias.

**Step 3: Fix key name if needed**

In `registry.rs:322`, change `"call_id"` to `"callID"` to match the loader's expected key:

```rust
if let Some(call_id) = &ctx.call_id {
    before_hook_ctx = before_hook_ctx.with_data("callID", serde_json::json!(call_id));
}
```

Do the same for the after hook at `registry.rs:362`.

**Step 4: Run check**

Run: `cargo check -p rocode-tool`
Expected: no errors

**Step 5: Commit**

```bash
git add crates/rocode-tool/src/registry.rs
git commit -m "fix(plugin): use callID key for tool hook context (TS parity)"
```

---

## Task 5: M2 — Circuit breaker for plugin hooks

**Files:**
- Create: `crates/rocode-plugin/src/circuit_breaker.rs`
- Modify: `crates/rocode-plugin/src/lib.rs`
- Modify: `crates/rocode-plugin/src/subprocess/loader.rs:322-340`

**Step 1: Write the failing test**

```rust
// In crates/rocode-plugin/src/circuit_breaker.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trips_after_threshold_failures() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_tripped());
        cb.record_failure();
        assert!(cb.is_tripped());
    }

    #[test]
    fn recovers_after_cooldown() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(0));
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_tripped());
        // With 0s cooldown, should recover immediately
        std::thread::sleep(Duration::from_millis(10));
        assert!(!cb.is_tripped());
    }

    #[test]
    fn resets_on_success() {
        let mut cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.failures.len(), 0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rocode-plugin -- circuit_breaker -v`
Expected: FAIL (module doesn't exist)

**Step 3: Implement CircuitBreaker**

```rust
// crates/rocode-plugin/src/circuit_breaker.rs
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct CircuitBreaker {
    pub(crate) failures: VecDeque<Instant>,
    threshold: usize,
    window: Duration,
    tripped_until: Option<Instant>,
    cooldown: Duration,
}

impl CircuitBreaker {
    pub fn new(threshold: usize, cooldown: Duration) -> Self {
        Self {
            failures: VecDeque::new(),
            threshold,
            window: Duration::from_secs(60),
            tripped_until: None,
            cooldown,
        }
    }

    pub fn is_tripped(&self) -> bool {
        if let Some(until) = self.tripped_until {
            if Instant::now() < until {
                return true;
            }
        }
        false
    }

    pub fn record_failure(&mut self) {
        let now = Instant::now();
        self.failures.push_back(now);
        // Evict old failures outside window
        while self.failures.front().is_some_and(|t| now.duration_since(*t) > self.window) {
            self.failures.pop_front();
        }
        if self.failures.len() >= self.threshold {
            self.tripped_until = Some(now + self.cooldown);
            tracing::warn!(
                threshold = self.threshold,
                cooldown_secs = self.cooldown.as_secs(),
                "[plugin-breaker] circuit breaker tripped"
            );
        }
    }

    pub fn record_success(&mut self) {
        self.failures.clear();
        self.tripped_until = None;
    }
}
```

**Step 4: Add module to lib.rs**

Add `pub mod circuit_breaker;` to `crates/rocode-plugin/src/lib.rs`.

**Step 5: Run tests**

Run: `cargo test -p rocode-plugin -- circuit_breaker -v`
Expected: PASS (3 tests)

**Step 6: Integrate into hook registration**

In `loader.rs:322-340`, wrap the hook closure with circuit breaker check. Add a `HashMap<(String, HookEvent), CircuitBreaker>` to the loader struct, and check `is_tripped()` before calling `invoke_hook`. On timeout, call `record_failure()`. On success, call `record_success()`.

**Step 7: Commit**

```bash
git add crates/rocode-plugin/src/circuit_breaker.rs crates/rocode-plugin/src/lib.rs crates/rocode-plugin/src/subprocess/loader.rs
git commit -m "feat(plugin): add circuit breaker for plugin hooks

Trips after 3 timeouts in 60s window, cools down for 60s.
Per (plugin, event) granularity. Auto-recovers after cooldown."
```

---

## Task 6: M2 — Timeout self-heal with in-place reconnect

**Files:**
- Modify: `crates/rocode-plugin/src/subprocess/client.rs:123-137,440-460`

**Step 1: Refactor PluginSubprocess to use swappable Transport**

Extract stdin/stdout/process into an inner struct:

```rust
struct Transport {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    process: Child,
}

pub struct PluginSubprocess {
    name: String,
    transport: Arc<RwLock<Transport>>,
    rpc_lock: Arc<Mutex<()>>,
    request_id: AtomicU64,
    hooks: Vec<String>,
    auth_meta: Option<AuthMeta>,
    timeout: Duration,
    // Saved for reconnect
    plugin_path: String,
    init_context: serde_json::Value,
}
```

**Step 2: Add reconnect method**

```rust
impl PluginSubprocess {
    async fn reconnect(&self) -> Result<(), PluginSubprocessError> {
        tracing::warn!(
            plugin = %self.name,
            "[plugin-heal] reconnecting after timeout"
        );
        // Kill old process
        {
            let mut transport = self.transport.write().await;
            let _ = transport.process.kill().await;
        }
        // Spawn new process
        let mut child = Command::new("bun")
            .arg("run")
            .arg(&self.plugin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());

        // Swap transport
        {
            let mut transport = self.transport.write().await;
            *transport = Transport { stdin, stdout, process: child };
        }

        // Re-initialize
        let params = serde_json::json!({
            "pluginPath": &self.plugin_path,
            "context": &self.init_context,
        });
        let _: InitializeResult = self.call("initialize", Some(params)).await?;

        tracing::info!(
            plugin = %self.name,
            "[plugin-heal] reconnected successfully"
        );
        Ok(())
    }
}
```

**Step 3: Add reconnect on timeout in call()**

In the `call` method, after `Timeout` error:

```rust
let response = tokio::time::timeout(self.timeout, self.read_response_for_id(id))
    .await;

match response {
    Ok(Ok(resp)) => { /* existing success path */ }
    Ok(Err(e)) => return Err(e),
    Err(_) => {
        // Timeout — attempt reconnect for next call
        let _ = self.reconnect().await;
        return Err(PluginSubprocessError::Timeout);
    }
}
```

**Step 4: Run check**

Run: `cargo check -p rocode-plugin`
Expected: no errors

**Step 5: Commit**

```bash
git add crates/rocode-plugin/src/subprocess/client.rs
git commit -m "feat(plugin): in-place reconnect on hook timeout

Swaps internal transport (stdin/stdout/process) without replacing
the outer Arc<PluginSubprocess>. Hook closures continue to work
with the same handle after reconnect."
```

---

## Task 7: M2 — Plugin stderr observability

**Files:**
- Modify: `crates/rocode-plugin/src/subprocess/client.rs` (spawn section)

**Step 1: Add stderr reader task after spawn**

In the `spawn` method, after taking stderr from the child process:

```rust
if let Some(stderr) = child.stderr.take() {
    let plugin_name = this.name.clone();
    tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut count = 0u64;
        let mut last_reset = Instant::now();
        while let Ok(Some(line)) = lines.next_line().await {
            // Rate limit: max 20 lines per second
            if last_reset.elapsed() > Duration::from_secs(1) {
                count = 0;
                last_reset = Instant::now();
            }
            count += 1;
            if count <= 20 {
                tracing::warn!(
                    plugin = %plugin_name,
                    "[plugin-stderr] {}", line
                );
            }
        }
    });
}
```

**Step 2: Run check**

Run: `cargo check -p rocode-plugin`
Expected: no errors

**Step 3: Commit**

```bash
git add crates/rocode-plugin/src/subprocess/client.rs
git commit -m "feat(plugin): pipe plugin stderr to tracing with rate limiting

Plugin stderr lines appear as [plugin-stderr] warnings.
Rate limited to 20 lines/second to prevent log storms."
```

---

## Task 8: M3 — Large payload file channel (Rust side)

**Files:**
- Modify: `crates/rocode-plugin/src/subprocess/client.rs:228-241`

**Step 1: Add file channel to invoke_hook**

```rust
const LARGE_PAYLOAD_THRESHOLD: usize = 64 * 1024; // 64KB

pub async fn invoke_hook(
    &self,
    hook: &str,
    input: Value,
    output: Value,
) -> Result<Value, PluginSubprocessError> {
    let params = serde_json::json!({
        "hook": hook,
        "input": input,
        "output": output,
    });

    let serialized = serde_json::to_string(&params).unwrap_or_default();

    if serialized.len() > LARGE_PAYLOAD_THRESHOLD {
        // Write to temp file
        let dir = std::env::temp_dir().join("rocode-plugin-ipc");
        tokio::fs::create_dir_all(&dir).await.ok();
        let token = uuid::Uuid::new_v4().to_string();
        let file_path = dir.join(format!("{}.json", token));
        tokio::fs::write(&file_path, &serialized).await
            .map_err(|e| PluginSubprocessError::IoError(e.to_string()))?;

        // Set restrictive permissions (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&file_path, perms).ok();
        }

        let file_params = serde_json::json!({
            "file": file_path.to_string_lossy(),
            "token": token,
        });
        let result: Value = self.call("hook.invoke.file", Some(file_params)).await?;

        // Cleanup
        tokio::fs::remove_file(&file_path).await.ok();

        Ok(result.get("output").cloned().unwrap_or(Value::Null))
    } else {
        let result: Value = self.call("hook.invoke", Some(params)).await?;
        Ok(result.get("output").cloned().unwrap_or(Value::Null))
    }
}
```

**Step 2: Run check**

Run: `cargo check -p rocode-plugin`
Expected: no errors

**Step 3: Commit**

```bash
git add crates/rocode-plugin/src/subprocess/client.rs
git commit -m "feat(plugin): large payload file channel for hook IPC

Payloads >64KB are written to temp file and referenced via
hook.invoke.file RPC method. File uses 0600 permissions and
unique token. Cleaned up after use."
```

---

## Task 9: M3 — Large payload file channel (TypeScript side)

**Files:**
- Modify: `crates/rocode-plugin/host/plugin-host.ts` (RPC handler section)

**Step 1: Add hook.invoke.file handler**

In the RPC method dispatcher in `plugin-host.ts`, add a handler for `hook.invoke.file`:

```typescript
case "hook.invoke.file": {
    const { file, token } = params;
    // Validate: file must be in controlled directory and contain token
    const expectedDir = path.join(os.tmpdir(), "rocode-plugin-ipc");
    const resolvedPath = path.resolve(file);
    if (!resolvedPath.startsWith(expectedDir) || !resolvedPath.includes(token)) {
        return { error: { code: -32602, message: "Invalid file path" } };
    }
    const content = fs.readFileSync(resolvedPath, "utf-8");
    const hookParams = JSON.parse(content);
    // Delegate to existing hook.invoke logic
    const result = await invokeHook(hookParams.hook, hookParams.input, hookParams.output);
    return { result };
}
```

**Step 2: Verify manually**

Run rocode with a plugin and large message context. Check logs for `hook.invoke.file` usage.

**Step 3: Commit**

```bash
git add crates/rocode-plugin/host/plugin-host.ts
git commit -m "feat(plugin): handle hook.invoke.file in plugin host

Reads large payloads from temp file instead of stdin pipe.
Validates file path is in controlled directory with matching token."
```

---

## Task 10: Feature flags and cleanup

**Files:**
- Modify: `crates/rocode-plugin/src/lib.rs`
- Create: `crates/rocode-plugin/src/feature_flags.rs`

**Step 1: Add feature flags**

```rust
// crates/rocode-plugin/src/feature_flags.rs
use std::sync::atomic::{AtomicBool, Ordering};

static SEQ_HOOKS: AtomicBool = AtomicBool::new(true);
static TIMEOUT_SELF_HEAL: AtomicBool = AtomicBool::new(true);
static CIRCUIT_BREAKER: AtomicBool = AtomicBool::new(true);
static LARGE_PAYLOAD_FILE: AtomicBool = AtomicBool::new(true);

pub fn is_enabled(flag: &str) -> bool {
    match flag {
        "plugin_seq_hooks" => SEQ_HOOKS.load(Ordering::Relaxed),
        "plugin_timeout_self_heal" => TIMEOUT_SELF_HEAL.load(Ordering::Relaxed),
        "plugin_circuit_breaker" => CIRCUIT_BREAKER.load(Ordering::Relaxed),
        "plugin_large_payload_file_ipc" => LARGE_PAYLOAD_FILE.load(Ordering::Relaxed),
        _ => false,
    }
}

pub fn set(flag: &str, enabled: bool) {
    match flag {
        "plugin_seq_hooks" => SEQ_HOOKS.store(enabled, Ordering::Relaxed),
        "plugin_timeout_self_heal" => TIMEOUT_SELF_HEAL.store(enabled, Ordering::Relaxed),
        "plugin_circuit_breaker" => CIRCUIT_BREAKER.store(enabled, Ordering::Relaxed),
        "plugin_large_payload_file_ipc" => LARGE_PAYLOAD_FILE.store(enabled, Ordering::Relaxed),
        _ => {}
    }
}
```

**Step 2: Gate each feature behind its flag**

- In `lib.rs:trigger()`: check `is_enabled("plugin_seq_hooks")`, fallback to `join_all` if disabled
- In `client.rs:call()`: check `is_enabled("plugin_timeout_self_heal")` before reconnect
- In `loader.rs`: check `is_enabled("plugin_circuit_breaker")` before breaker check
- In `client.rs:invoke_hook()`: check `is_enabled("plugin_large_payload_file_ipc")` before file channel

**Step 3: Run full test suite**

Run: `cargo test -p rocode-plugin -p rocode-session -p rocode-tool`
Expected: all tests pass

**Step 4: Commit**

```bash
git add crates/rocode-plugin/src/feature_flags.rs crates/rocode-plugin/src/lib.rs crates/rocode-plugin/src/subprocess/client.rs crates/rocode-plugin/src/subprocess/loader.rs
git commit -m "feat(plugin): add feature flags for all plugin optimizations

Flags: plugin_seq_hooks, plugin_timeout_self_heal,
plugin_circuit_breaker, plugin_large_payload_file_ipc.
All enabled by default, can be disabled for rollback."
```

---

## Task 11: Startup cleanup for temp files

**Files:**
- Modify: `crates/rocode-plugin/src/subprocess/loader.rs` (init section)

**Step 1: Add cleanup on startup**

In the plugin loader's initialization, clean up any leftover temp files:

```rust
// Clean up stale IPC temp files from previous runs
let ipc_dir = std::env::temp_dir().join("rocode-plugin-ipc");
if ipc_dir.exists() {
    if let Ok(entries) = std::fs::read_dir(&ipc_dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
```

**Step 2: Commit**

```bash
git add crates/rocode-plugin/src/subprocess/loader.rs
git commit -m "fix(plugin): clean up stale IPC temp files on startup"
```
