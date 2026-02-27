# rocode 插件系统优化设计（v2）

## 1. 范围与目标

- 让 rocode 的插件触发语义与 TS opencode 对齐，降低 Bun 高 CPU 与超时后的"卡住感"。
- 不改任何插件代码（包括 oh-my-opencode），只改 rocode 主体。
- 先做语义正确性，再做稳定性，再做传输优化。

## 2. 里程碑与顺序

1. **M0** 基线与观测（不改行为）
2. **M1** P0 语义对齐（阻断项）
3. **M2** P2 稳定性与自愈（第二优先）
4. **M3** P1 传输降本（最后做，分阶段）

---

## 3. M0 基线与观测（不改行为）

### 涉及文件

- `crates/rocode-plugin/src/lib.rs`
- `crates/rocode-session/src/prompt/mod.rs`
- `crates/rocode-tool/src/registry.rs`

### 任务

- 增加 hook 级别 perf 日志：event、plugin、payload_bytes、duration_ms、timeout/error。
- 记录每轮 prompt 的 hook 序列与次数（含 chat.message、experimental.chat.messages.transform、tool.execute.before、tool.execute.after）。
- 输出统一日志前缀：`[plugin-perf]`、`[plugin-seq]`。

### 验收

- 能用同一段会话把 TS 与 rocode 的 hook 序列并排比较。
- 有 p50/p95 数据，不再靠体感判断。

---

## 4. M1 P0 语义对齐（阻断项）

### 4.1 chat.message 触发时机对齐

**涉及文件**: `crates/rocode-session/src/prompt/mod.rs`

**当前行为**: 在 `create_user_message` 末尾触发 `ChatMessage`（`mod.rs:470`），payload 是 user message。

**目标行为**: 在 assistant 消息本轮定型后触发（含 tool part 状态更新后），payload 用 assistant message/parts。每个 assistant message ID 只触发一次。

**具体改动**:
1. 移除 `create_user_message` 中的 `ChatMessage` hook 触发代码（`mod.rs:470-493`）。
2. 在 `loop_inner` 中 `process_response` 返回、assistant message 写入 session、tool part 状态更新完成后，触发 `ChatMessage`。
3. payload 对齐 TS 结构：
   - input: `{sessionID, agent, model, messageID, variant}`
   - output: `{message: <assistant_message>, parts: <assistant_parts>}`

**验收**:
- 触发对象是 assistant，不是 user。
- 工具调用场景下不会提前触发导致插件读到半成品内容。

### 4.2 Hook I/O 映射纠偏（最小改动）

**涉及文件**: `crates/rocode-plugin/src/subprocess/loader.rs`

**当前问题**: `hook_io_from_context`（`loader.rs:452`）已有 input/output 拆分和 key 归一化，但 `tool.execute.before` 的 args 放在了 input 而非 output，与 TS 语义不一致。

**目标行为**（保持 TS 语义）:
- `tool.execute.before`: input=`{tool, sessionID, callID}`, output=`{args}`
- `tool.execute.after`: input 含 `tool/sessionID/callID/args`, output 含结果对象
- `chat.message`: input=`{sessionID, agent, model, messageID, variant}`, output=`{message, parts}`
- `chat.messages.transform`: input=`{}`, output=`{messages}`

**具体改动**: 在 `hook_io_from_context` 中调整 key 归属，不重写整层映射，保留现有 alias 兼容逻辑。

**验收**: 插件可通过 before hook 改写 `output.args` 并影响实际工具入参。

### 4.3 插件执行顺序与 TS 对齐

**涉及文件**: `crates/rocode-plugin/src/lib.rs`

**当前行为**: `trigger` 方法使用 `join_all` 并行执行所有 hook（`lib.rs:246`）。

**目标行为**: 按注册顺序串行 `await`，与 TS 的 `for` 循环语义一致。

**具体改动**: 把 `join_all(futures).await` 改为 `for` 循环逐个 `await`。保留已有 priority 规则与 `CACHEABLE_EVENTS` 缓存逻辑。

**验收**:
- 多插件同 hook 时，执行顺序稳定且可预测。
- 与 TS 的链式副作用语义一致。

---

## 5. M2 P2 稳定性与自愈（第二优先）

### 5.1 超时后"同句柄自愈重连"

**涉及文件**:
- `crates/rocode-plugin/src/subprocess/client.rs`
- `crates/rocode-plugin/src/subprocess/loader.rs`

**关键设计决策**: 不替换外层 `Arc<PluginSubprocess>` 句柄（hook closure 捕获的是 Arc），而是在同一个 Arc 内做 in-place 重连（swap 内部传输层：stdin/stdout/process）。

**具体改动**:
1. 将 `PluginSubprocess` 的 `stdin`、`stdout`、`process` 字段包装为可热切换的内部结构（`Arc<RwLock<Transport>>`）。
2. 在 `call` 方法中，当 `Timeout` 错误发生时：
   - kill 当前子进程
   - 重新 spawn 子进程，重新发送 `initialize` RPC
   - swap 内部 transport 字段
   - 对当前调用返回错误（不重试，避免重复副作用）
3. 下一次 hook 调用自动使用新的 transport。

**验收**:
- 单次 timeout 后无需重启 rocode，后续 hook 恢复。
- 不出现"老 closure 持有失效 client"的悬挂问题。
- 并发调用下无死锁/悬挂；句柄一致性可验证。

### 5.2 断路器

**涉及文件**: `crates/rocode-plugin/src/subprocess/loader.rs`

**设计**:
```rust
struct CircuitBreaker {
    failures: VecDeque<Instant>,  // 滑动窗口
    tripped_until: Option<Instant>,
}
```

**规则**:
- 维度：`(plugin, event)`。
- 60 秒窗口内 3 次 timeout → 熔断 60 秒。
- 熔断期间返回 `Ok(HookOutput::empty())` 并记日志。
- 窗口后自动半开恢复；下一次成功则重置计数器。

**验收**:
- 异常插件不再把 CPU 长时间拖满。
- 熔断状态有可观测日志。

### 5.3 stderr 可观测性

**涉及文件**: `crates/rocode-plugin/src/subprocess/client.rs`

**具体改动**:
1. spawn 子进程后，起一个 tokio task 持续读 stderr，按行输出到 `tracing::warn!("[plugin:{name}] {line}")`。
2. 可通过配置控制日志级别（默认 warn，可调为 debug）。
3. 做简单限流（每秒最多 N 行），避免日志风暴。

**验收**: 出问题时可直接从日志定位插件行为。

---

## 6. M3 P1 传输降本（分阶段）

### 6.1 阶段一：大 payload 文件通道（安全版）

**涉及文件**:
- `crates/rocode-plugin/src/subprocess/client.rs`
- `crates/rocode-plugin/host/plugin-host.ts`

**设计**:
- 超过阈值（默认 64KB）走文件通道。
- 不用 `__payload_file` 通用键（有冲突风险），改为显式 RPC 方法 `hook.invoke.file`。
- 临时文件放受控目录（`$TMPDIR/rocode-plugin-ipc/`），0600 权限，带一次性 token 校验。
- 调用完毕后立即删除；进程启动时清理残留文件。

**协议**:
```
Rust → bun: {"method": "hook.invoke.file", "params": {"hook": "...", "file": "/tmp/rocode-plugin-ipc/xxx.json", "token": "abc123"}}
bun → Rust: {"result": {"output": ...}}  // 或同样走文件回传
```

**验收**:
- 大消息场景 IPC 序列化压力下降。
- 无路径注入与文件残留问题。

### 6.2 阶段二：sdk.request 反向 RPC（单独 RFC）

**不并入本轮主线**。单独设计，先给协议草案与并发模型（request-id 路由、多路复用、取消/超时语义），评审通过后再排期。

---

## 7. 测试任务单

### 单元测试
- chat.message 只在 assistant 定型后触发。
- tool.execute.before 改 args 能生效。
- 多插件同 hook 串行顺序正确。
- timeout 后下一次 hook 恢复。
- 断路器触发/恢复路径。

### 集成测试
- 使用一个"故意 sleep 超时"的测试插件验证自愈与熔断。
- 使用大消息 messages.transform 验证大 payload 文件通道。

### 回归测试
- task/read/write 现有链路不回归。
- 插件未安装、插件报错、插件超时三类异常路径都可继续会话。

---

## 8. 发布与回滚策略

每个里程碑加独立 feature flag：
- `plugin_seq_hooks`
- `plugin_timeout_self_heal`
- `plugin_circuit_breaker`
- `plugin_large_payload_file_ipc`

先灰度 M1 + M2，观察 24 小时日志，再开 M3。任一指标恶化可单独关闭对应 flag 回滚。

---

## 9. 量化验收标准

- hook 序列与 TS 对齐率：100%（基准场景）。
- timeout 后恢复时间：下一次 hook（<2s）可用。
- 大消息场景 Bun CPU 峰值持续时间明显下降（至少 40%）。
- tool.execute.before 参数改写成功率：100%。

---

## 10. Issue 列表

| ID | Issue | 预计人天 | 依赖 | 风险 |
|---|---|---:|---|---|
| I-00 | 插件 perf/序列基线日志埋点 | 0.8 | 无 | 低 |
| I-01 | TS vs rocode hook 序列对比脚本 | 0.7 | I-00 | 低 |
| I-10 | chat.message 触发迁移到 assistant 定型后 | 1.0 | I-00 | 中 |
| I-11 | tool.execute.before/after I/O 映射纠偏 | 0.8 | I-00 | 高 |
| I-12 | 插件触发从并行改串行 | 0.6 | I-00 | 中 |
| I-13 | M1 回归测试包 | 1.0 | I-10,I-11,I-12 | 中 |
| I-20 | 超时后同句柄自愈重连 | 1.5 | I-13 | 高 |
| I-21 | 自愈状态管理与并发保护 | 1.0 | I-20 | 高 |
| I-22 | (plugin,event) 维度断路器 | 1.2 | I-20 | 中 |
| I-23 | 插件 stderr 提升可观测性 | 0.8 | I-20 | 低 |
| I-24 | M2 稳定性测试包 | 1.2 | I-21,I-22,I-23 | 中 |
| I-30 | 大 payload 文件通道（Rust 端） | 1.2 | I-24 | 中 |
| I-31 | 大 payload 文件通道（plugin-host.ts 端） | 1.2 | I-30 | 中 |
| I-32 | 临时文件安全加固 | 0.8 | I-31 | 高 |
| I-33 | M3 性能回归与对比 | 1.0 | I-32 | 中 |
| I-34 | sdk.request 反向 RPC RFC（仅设计） | 1.5 | I-24 | 中 |
| I-40 | Feature flag 与配置项接入 | 0.8 | I-13,I-24,I-33 | 低 |
| I-41 | 发布灰度与回滚预案 | 0.8 | I-40 | 低 |

**总计**: ~18 人天

### 关键路径

```
I-00 → I-10/I-11/I-12 → I-13
I-13 → I-20 → I-21/I-22/I-23 → I-24
I-24 → I-30 → I-31 → I-32 → I-33
I-40 → I-41（发布门）
```

### 并行建议（2 人配置）

1. **人员A（Rust主线）**: I-00, I-10, I-11, I-12, I-20, I-21, I-22, I-40
2. **人员B（QA+TS）**: I-01, I-13, I-23, I-24, I-31, I-33, I-41
3. **架构评审穿插**: I-34（不阻塞主线）
