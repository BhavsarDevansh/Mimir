# Hooks Engine

> **Issue:** #386
>
> **Phase:** 2 — Knowledge Graph / Core Agent
>
> **Version:** 0.131.0

## Overview

The hooks engine (`mimir-core/src/hooks/mod.rs`) is a typed background-task system that replaces LLM-orchestrated remembering with deterministic, server-side learning. Instead of the conversational LLM deciding whether to call the `remember` tool, the daemon enqueues typed hook instances on deterministic triggers (turn completed, connector item staged, fact inserted) and a single dispatch loop drains them through the durable `JobQueue` under per-hook queue policies, key scopes, debounce windows, execution gates, and retry policies.

This removes the prompt-injection path where a user could steer the model into or out of calling `remember`, and it makes learning work for OpenAI-compatible remote clients that never call tools (issue #388). The accepted cost is an occasional trivial extraction (a lone "hello" after the debounce window).

## Design

### Triggers

`Trigger` is a minimal typed enum (v1; the #68 domain-event bus can become the general trigger source later):

- `TurnCompleted { session_id, payload }` — a non-incognito chat turn completed (assistant response persisted).
- `ConnectorItemStaged { item_id, payload }` — a connector item was staged and needs LLM extraction.
- `FactInserted` — a fact was inserted / memory became dirty.

Each trigger carries an `Arc<dyn Any + Send + Sync>` payload; handlers downcast to their expected type. The engine itself never inspects payload contents.

### Queue policies

- `Multiple` — every trigger enqueues; FIFO.
- `SingularFirstWins` — the first instance stays; new triggers are dropped while one is pending or running for the key.
- `SingularLastWins { debounce }` — a pending instance is replaced with the latest payload and re-enqueued at the tail (true debounce); a running instance is unaffected and the new trigger enqueues a fresh pending instance. An optional `merge` function accumulates the new payload into the old one (e.g. chat turns since the last hook run).

### Key scope

Singularity is either `Global` (one pending instance for the whole hook) or `PerKey` (one pending instance per trigger key, e.g. per `session_id`), so two sessions can each have one pending instance while a third hook stays globally unique.

### Execution gates

- `IdleGated { cooldown }` — dispatch waits for the user-activity cooldown and a completely idle LLM worker pool, so background work never steals LLM capacity from interactive chat.
- `Ungated` — dispatch immediately; LLM calls route through the shared `LlmWorkerPool` system queue.

### Durability

The pending queue is in-memory; runs stay durable in `JobQueue`. A daemon restart loses only pending instances — chat re-triggers on the next turn, connector items are re-staged by the durable sync cursor (issues #314, #332), and condensation re-triggers on the next fact write. Each registered hook owns one durable `JobQueue` job whose handler executes the hook's currently running instance via a `Weak<EngineInner>` reference (no reference cycle between engine and queue).

### Retry

Each hook has a `RetryPolicy { max_attempts, backoff }`. A handler returns `HookOutcome::Success`, `RetryableFailure` (re-enqueue with exponential backoff while the budget lasts), or `TerminalFailure` (drop; the handler is responsible for recording any durable terminal state). The engine also re-enqueues on `JobQueue` errors and drops instances on cancellation.

## Hooks in v1

### `remember.chat`

- Trigger: `TurnCompleted`, fired on both the blocking and streaming chat paths after the assistant message is persisted, non-incognito sessions only (incognito stays a hard no-persistence guarantee, #155).
- Key: `session_id`. Policy: `SingularLastWins` with debounce `agent.remember_debounce_seconds` (default 10) and `merge_chat_turns` accumulation, so a burst of messages becomes one extraction over the accumulated transcript.
- Gate: `IdleGated` with the scheduler's cooldown.
- Handler: `ChatLearningHandler` (`mimir-server/src/state/hooks.rs`) runs the existing Librarian extraction pipeline (`extract_facts_with_context`) with classification→confidence mapping, the sensitive-pending confirmation gate, and the overwrite/coexistence matrix still enforced in Rust, unchanged.

### `connector_item.remember`

- Trigger: `ConnectorItemStaged`, fired by the Email connector's `extract` step when a prose message needs LLM extraction. Routing stays deterministic in Rust: structured-parse first, LLM hook only when needed.
- Key: item id (email UID) for attribution. Policy: `Multiple` — every item enqueued individually, FIFO, no dedup.
- Gate: `Ungated` — queues through `LlmWorkerPool` like connector extraction did before.
- Handler: `EmailExtractionHook` (`mimir-connectors/src/email/llm/hook.rs`) parses the RFC 822 payload, runs `extract_prose_facts`, and inserts through `normalize_and_insert` with connector provenance. Per-item retry / terminal-failure semantics moved from the connector cycle (issue #262) into the hook runner: the payload carries the per-connector `llm_extraction_max_attempts` budget, and the final failed attempt records a durable terminal failure in the shared `ProseRetryLedger` so the message is never re-processed and the failure surfaces via `Degraded` health.

### `memory.condensation`

- Trigger: `FactInserted`, fired by the KG dirty notify path (and the knowledge-optimization job) instead of the old dirty-signal `Notify` listener submission.
- Policy: `SingularLastWins`, `Global`, with the scheduler's debounce and cooldown — identical behaviour to the pre-hook dirty-signal submission.
- Handler: `CondensationHandler` (`mimir-server/src/state/hooks.rs`) rebuilds the condensed memory block via `MemoryCondenser`. `POST /memory/refresh` force-runs this hook (`force_run` bypasses debounce, cooldown, idle gates, and the pending queue).

## API surface

- `HookEngine::new(job_queue, llm)` — returns `(Arc<HookEngine>, shutdown_rx)`.
- `register(Hook)` — registers a hook and its durable `JobQueue` job; duplicate ids fail with `AlreadyRegistered`.
- `trigger(Trigger)` — enqueues (or drops / replaces) an instance for every registered hook matching the trigger kind; returns per-hook `TriggerOutcome` (`Enqueued` / `Dropped` / `Replaced`).
- `notify_user_activity()` — resets the cooldown for idle-gated hooks.
- `force_run(hook_id)` — runs a hook immediately with an empty `()` payload, bypassing all gates; errors with `NotRegistered` / `AlreadyRunning`.
- `pending_depth()` / `pending_depth_for(hook_id)` / `running_count()` — observability; `pending_depth` is surfaced in `GET /status` as `hook_queue_depth`.
- `start(shutdown_rx)` — the single dispatch loop; runs until the shutdown watch channel fires.
- `shutdown()` — signals the loop and cancels the in-flight hook run.

## Testing

- `mimir-core/src/hooks/tests.rs` — unit tests for each queue policy (drop, replace-at-tail with fresh payload, FIFO), key scope, debounce window, idle gating, retry backoff, force-run, and shutdown.
- `mimir-server/tests/chat_learning_tests.rs` — server integration tests: non-incognito blocking and streaming turns enqueue the hook and persist facts; incognito turns never enqueue any hook and write no facts.
- `mimir-server/tests/kb_query_tests.rs` — asserts the `remember` tool is absent from the registry and the OpenAI export.

## Non-goals

- User-scriptable or plugin hooks — internal Rust hooks only; config tunes thresholds, not behaviours.
- The general domain-event bus from #68 — v1 uses the minimal typed trigger enum.
- Migrating the scheduled jobs (knowledge optimization, pending cleanup, events scan) onto hooks.
- Removing `retrieve_context` or the KG query tools — they stay LLM tools.
