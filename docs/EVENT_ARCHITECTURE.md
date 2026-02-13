# Event-Driven Architecture

This document describes the in-process event-driven architecture used to coordinate pipeline jobs. It covers the event bus, event types, subscriber patterns, configuration, troubleshooting, and rollback procedures.

**Audience:** Developers maintaining or extending the evaluator's orchestration layer.

---

## 1. Architecture overview

The evaluator uses an **in-process event bus** built on Tokio channels to coordinate pipeline jobs. Events replace timer-based scheduling for specific workflows (discovery, classification, paper trading fast-path) while coexisting with the existing timer-based scheduler for all other jobs.

### Design principles

- **Feature-flagged:** Every event-driven behavior is behind a config flag. All flags default to `false`, so the system behaves identically to the pre-event codebase when flags are off.
- **Hybrid model:** Timer-based scheduling remains for jobs that don't benefit from event-driven triggers (ingestion, holders snapshots, WAL checkpoints, etc.). Event-driven triggers replace timers only where causal ordering or latency matters.
- **Broadcast + watch hybrid:** Pipeline and operational events use `tokio::sync::broadcast` (multi-subscriber pub/sub). Fast-path triggers use `tokio::sync::watch` (coalescing, latest-generation-wins).
- **No external broker:** All events are in-process. The same concepts can later be backed by Redis/SQS if multi-process scaling is needed.

### Key source files

| File | Purpose |
|------|---------|
| `crates/evaluator/src/event_bus.rs` | EventBus struct: publish/subscribe for all channel types |
| `crates/evaluator/src/events/mod.rs` | Event type definitions (PipelineEvent, FastPathTrigger, OperationalEvent) |
| `crates/evaluator/src/events/subscribers.rs` | Subscriber implementations (logging, discovery trigger, classification trigger, fast-path) |
| `crates/evaluator/src/main.rs` | Wiring: EventBus initialization, subscriber spawning, scheduler setup |
| `crates/common/src/config.rs` | `Events` config struct |
| `config/default.toml` | Default config values (all flags off) |

---

## 2. Event types

### 2.1 Pipeline events (`PipelineEvent`)

Pipeline events signal job completion and carry summary data. They are published via `broadcast` channels, meaning every subscriber receives every event.

| Variant | Published by | Payload | Downstream effect |
|---------|-------------|---------|-------------------|
| `MarketsScored` | `run_event_scoring_once` | `markets_scored`, `events_ranked`, `completed_at` | Triggers wallet discovery (when `enable_discovery_event_trigger=true`) |
| `WalletsDiscovered` | `run_wallet_discovery_once` | `market_id`, `wallets_added`, `discovered_at` | Informational (logged) |
| `TradesIngested` | `run_trades_ingestion_once` | `wallet_address`, `trades_count`, `ingested_at` | Triggers classification (batched) and fast-path (coalesced) |
| `WalletsClassified` | `run_persona_classification_once` | `wallets_classified`, `classified_at` | Informational (logged) |
| `WalletRulesEvaluated` | `run_wallet_rules_once` | `wallets_evaluated`, `transitions`, `evaluated_at` | Informational (logged) |

All variants are serializable (`serde::Serialize` + `serde::Deserialize`) with `#[serde(tag = "type")]` for JSON log compatibility.

### 2.2 Fast-path triggers (`FastPathTrigger`)

A `watch` channel that coalesces multiple triggers into a single generation counter. Used for latency-critical work like paper trading reactions to new trades.

```
TradesIngested → fast-path subscriber → EventBus.trigger_fast_path()
                                         → watch channel (generation += 1)
                                           → fast-path worker reads latest generation
```

Multiple `TradesIngested` events arriving in quick succession produce at most one downstream reaction (coalescing). The `generation` counter tells the worker how many triggers occurred.

### 2.3 Operational events (`OperationalEvent`)

Monitoring events for observability. Published via `broadcast` channels.

| Variant | Purpose |
|---------|---------|
| `JobStarted` | Job began execution |
| `JobCompleted` | Job finished successfully (includes `duration_ms`) |
| `JobFailed` | Job failed (includes `error` string) |
| `BackpressureWarning` | Queue approaching capacity (includes `current_size` / `capacity`) |

---

## 3. Event flow

### 3.1 Full pipeline flow

```
                    MarketsScored
event_scoring ─────────┐
                       v
               ┌─────────────────┐
               │ discovery trigger│ (when enable_discovery_event_trigger=true)
               │ subscriber       │
               └────────┬────────┘
                        v
            wallet_discovery (sends WalletsDiscovered)
                        │
                        v
              trades_ingestion ─── TradesIngested
                       │                  │
              ┌────────┴────────┐         │
              v                 v         v
    classification      fast-path     logging
    trigger             subscriber    subscriber
    (batched,           (coalescing   (logs all
     N-second            → paper       events)
     window)              trading
    → persona             fast tick)
      classification
```

### 3.2 Discovery trigger flow

When `enable_discovery_event_trigger=true`:

1. `run_event_scoring_once` publishes `MarketsScored` on the pipeline broadcast channel.
2. `spawn_discovery_trigger_subscriber` receives `MarketsScored` and sends `()` on the `wallet_discovery_tx` mpsc channel.
3. The wallet discovery worker loop (`wallet_discovery_rx.recv()`) wakes up and runs `run_wallet_discovery_once`.
4. The timer-based discovery scheduler job is **not added** to the scheduler (replaced by event trigger).

### 3.3 Classification trigger flow

When `enable_classification_event_trigger=true`:

1. `run_trades_ingestion_once` publishes `TradesIngested` for each wallet processed.
2. `spawn_classification_trigger_subscriber` accumulates wallet addresses in a `TradesIngestedAccumulator` (deduplicating by address).
3. Every `classification_batch_window_secs` (default: 300s), if any wallets accumulated, it sends `()` on the `persona_classification_tx` channel.
4. The persona classification worker wakes up and runs `run_persona_classification_once`.
5. The timer-based classification scheduler job is **not added** (replaced by batched event trigger).

### 3.4 Fast-path trigger flow

When `enable_fast_path_trigger=true`:

1. `run_trades_ingestion_once` publishes `TradesIngested`.
2. `spawn_fast_path_subscriber` receives `TradesIngested` and calls `event_bus.trigger_fast_path()`, incrementing the `watch` channel generation.
3. `spawn_fast_path_worker` watches the generation and sends tick signals to downstream consumers (e.g., paper trading).
4. Multiple rapid `TradesIngested` events coalesce into fewer downstream ticks (watch semantics: only the latest generation matters).

---

## 4. Channel types and their semantics

| Channel | Tokio primitive | Semantics | Use case |
|---------|----------------|-----------|----------|
| Pipeline | `broadcast::channel(capacity)` | Multi-subscriber, buffered, lagged subscribers get `RecvError::Lagged` | Job completion signals |
| Operational | `broadcast::channel(capacity)` | Same as pipeline | Monitoring/observability |
| Fast-path | `watch::channel(default)` | Single-value, coalescing, latest wins | Latency-critical triggers |
| Scheduler ticks | `mpsc::channel(8)` | Single-consumer, bounded | Timer → worker signaling |

### Broadcast channel behavior

- **Buffer:** `bus_capacity` (default 1000). If a subscriber falls behind by more than this many events, it receives `RecvError::Lagged(n)` where `n` is the number of skipped events.
- **No subscribers:** `publish_pipeline()` returns `Err(SendError)` when there are zero subscribers. The caller ignores this (`let _ = bus.publish_pipeline(...)`) so event publishing never blocks job execution.
- **Clone:** `EventBus` is `Clone` (all senders are `Clone`). Each `subscribe_*()` call creates a new independent receiver.

### Watch channel behavior

- **Coalescing:** If 5 triggers arrive before the watcher reads, it sees only the latest generation (5). This is intentional for fast-path: we want "something changed" not "exactly what changed."
- **No buffering:** Only the latest value is stored.

---

## 5. Subscriber patterns

### 5.1 Logging subscriber

**Purpose:** Logs all pipeline and operational events to stdout via `tracing::info!` / `tracing::warn!`.

**Lifecycle:** Spawned at startup when `events.enabled=true`. Runs until the EventBus is dropped (process shutdown).

**Pattern:** Uses `tokio::select!` to listen on both the pipeline and operational broadcast receivers simultaneously.

### 5.2 Discovery trigger subscriber

**Purpose:** Converts `MarketsScored` pipeline events into `()` signals on the wallet discovery mpsc channel.

**Lifecycle:** Spawned at startup when `enable_discovery_event_trigger=true`. Shuts down when either the pipeline channel closes or the discovery mpsc receiver is dropped.

**Pattern:** Simple loop with `recv()`, filters for `MarketsScored` only, ignores other events.

### 5.3 Classification trigger subscriber

**Purpose:** Batches `TradesIngested` events by accumulating wallet addresses, then triggers classification at a configurable interval.

**Lifecycle:** Spawned at startup when `enable_classification_event_trigger=true`. Uses `tokio::select!` to listen for both events and a periodic timer.

**Pattern:**
- On `TradesIngested`: accumulate wallet address in `TradesIngestedAccumulator` (HashSet, deduplicates).
- On timer tick: if accumulator is non-empty, drain it and send `()` on the classification channel.
- Empty windows produce no trigger (no wasted work).

### 5.4 Fast-path subscriber

**Purpose:** Bridges `TradesIngested` pipeline events to the coalescing `watch` channel for latency-critical downstream work.

**Lifecycle:** Spawned at startup when `enable_fast_path_trigger=true`.

**Pattern:** On `TradesIngested`, calls `event_bus.trigger_fast_path()` (increments generation). Ignores other events.

### 5.5 Fast-path worker

**Purpose:** Watches the fast-path `watch` channel and sends generation numbers to downstream consumers.

**Lifecycle:** Spawned by the orchestration layer. Shuts down when the watch sender or tick receiver is dropped.

**Pattern:** Loop on `fast_path_rx.changed()`, read generation, send on `tick_tx`.

---

## 6. Configuration guide

All event configuration lives under `[events]` in `config/default.toml`:

```toml
[events]
enabled = false              # Master kill switch
log_to_db = false            # Persist events to event_log table (future)
bus_capacity = 1000          # Buffer size for broadcast channels

# Event-driven triggers
enable_discovery_event_trigger = false        # MarketsScored -> wallet_discovery
enable_classification_event_trigger = false   # TradesIngested (batched) -> classification
enable_fast_path_trigger = false              # TradesIngested (coalescing) -> paper tick
classification_batch_window_secs = 300        # 5 minutes batching window
```

### Enabling event-driven mode

To progressively enable event-driven features:

**Step 1: Enable the event bus (monitoring only)**
```toml
[events]
enabled = true
```
This initializes the EventBus and spawns the logging subscriber. All jobs continue to use timer-based scheduling. You now see pipeline events in logs.

**Step 2: Enable discovery trigger**
```toml
enable_discovery_event_trigger = true
```
Wallet discovery now runs when event scoring completes rather than on a fixed timer. The timer-based discovery job is automatically removed from the scheduler.

**Step 3: Enable classification trigger**
```toml
enable_classification_event_trigger = true
classification_batch_window_secs = 300  # adjust as needed
```
Persona classification now runs after trades are ingested (batched by window) rather than on an hourly timer.

**Step 4: Enable fast-path trigger**
```toml
enable_fast_path_trigger = true
```
Paper trading reactions to new trades use coalescing for minimal latency.

### Flag interactions

| `enabled` | `enable_*_trigger` | Behavior |
|-----------|-------------------|----------|
| `false` | any | All timers, no events. Identical to pre-event codebase. |
| `true` | `false` | EventBus initialized, logging subscriber active, all jobs still timer-based. |
| `true` | `true` | Specific jobs switch from timer to event-driven trigger. |
| `false` | `true` | Trigger flags ignored (event bus not initialized). |

---

## 7. Troubleshooting guide

### Problem: Events are not being published

**Symptoms:** No `Pipeline event:` log lines despite `events.enabled=true`.

**Check:**
1. Verify `events.enabled = true` in config.
2. Check startup log for `"event bus enabled (capacity=N)"`.
3. Verify jobs are actually running (check scheduler tick logs).
4. Events with zero subscribers silently fail (`let _ = bus.publish_pipeline(...)`) -- ensure the logging subscriber started before the first event is published.

### Problem: Discovery not triggering after scoring

**Symptoms:** `MarketsScored` events appear in logs but discovery doesn't run.

**Check:**
1. Verify `enable_discovery_event_trigger = true`.
2. Check for `"event-driven discovery trigger enabled"` in startup logs.
3. Look for `"MarketsScored received -- triggering wallet discovery"` in logs.
4. If `wallet_discovery_mode = "continuous"`, the continuous loop overrides event-driven triggers. Set `wallet_discovery_mode = "scheduled"` to use event-driven discovery.

### Problem: Classification runs too frequently / too infrequently

**Adjust:** `classification_batch_window_secs`. Smaller = more responsive but more CPU. Larger = more batching efficiency.

**Check:**
1. Verify `enable_classification_event_trigger = true`.
2. Look for `"classification trigger: batched wallets"` in logs to see batch sizes.
3. If no `TradesIngested` events arrive in a window, no classification trigger fires (by design).

### Problem: Broadcast channel lagging

**Symptoms:** Log lines containing `"lagged, skipping events"` or `"lagged, continuing"`.

**Fix:** Increase `bus_capacity` in config. Default is 1000. If a subscriber consistently falls behind, it means events are being published faster than the subscriber can process them.

### Problem: Events published but no subscribers receive them

**Cause:** Subscribers must be created **before** events are published. The `broadcast` channel does not replay past events.

**Fix:** In `main.rs`, subscriber spawning happens before the scheduler starts. If you add new subscribers, ensure they are spawned in the same initialization section.

---

## 8. Rollback procedures

The event architecture is fully backward-compatible. Rollback is a configuration change, not a code change.

### Full rollback (disable all events)

Set in `config/default.toml` or environment override:

```toml
[events]
enabled = false
```

**Effect:** The EventBus is not initialized. All jobs run on timer-based scheduling. The system behaves identically to the pre-event codebase.

### Partial rollback (disable specific triggers)

Disable individual triggers while keeping the event bus and logging:

```toml
[events]
enabled = true
enable_discovery_event_trigger = false    # revert to timer-based discovery
enable_classification_event_trigger = false  # revert to hourly timer
enable_fast_path_trigger = false          # disable fast-path coalescing
```

Each trigger flag independently controls its own subscriber. Disabling a trigger re-adds the corresponding timer job to the scheduler.

### Rollback steps

1. Edit `config/default.toml` (or set `EVALUATOR_EVENTS__ENABLED=false` env var).
2. Restart the evaluator process.
3. Verify in logs: no `"event bus enabled"` message appears at startup.
4. Verify timer-based scheduling resumes: check for scheduler tick logs for all jobs.

### No data migration needed

Events are ephemeral (in-memory broadcast channels). There is no persistent event log to clean up when rolling back.

---

## 9. Extending the event system

### Adding a new pipeline event

1. Add a new variant to `PipelineEvent` in `crates/evaluator/src/events/mod.rs`.
2. Add serialization test in the same file.
3. Publish the event in the relevant job function (pass `event_bus: Option<&EventBus>`).
4. Handle the new variant in the logging subscriber (`events/subscribers.rs`).
5. Optionally add a new trigger subscriber if the event should drive downstream work.

### Adding a new subscriber

1. Add an `async fn spawn_*_subscriber(event_bus: Arc<EventBus>, ...) -> ()` in `events/subscribers.rs`.
2. Add a config flag in the `Events` struct (`crates/common/src/config.rs`).
3. Wire the subscriber in `main.rs` (spawn conditionally on the flag).
4. Add tests covering: event received, irrelevant events ignored, shutdown on channel close.

### Why broadcast + watch (not just broadcast)

- **Broadcast** is for events where every subscriber needs every event (logging, auditing, driving specific workflows).
- **Watch** is for fast-path work where only "something changed" matters, not the exact sequence of changes. This prevents thundering-herd problems when many `TradesIngested` events arrive in rapid succession.

---

## 10. References

| Document | Relevance |
|----------|-----------|
| `docs/ARCHITECTURE.md` | Runtime and orchestration overview (timer-based + event-driven) |
| `docs/STRATEGY_BIBLE.md` | Pipeline stages and what each job does |
| `config/default.toml` | All configuration defaults |
| `crates/evaluator/src/main.rs` | Wiring and initialization |
