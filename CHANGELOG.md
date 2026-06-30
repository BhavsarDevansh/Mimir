# Changelog

## [0.61.0] — 2026-06-30

### Optimization & robustness sweep (issues #161–#168)

- **#161** (core): Completed truncated doc comments on `JobError::is_not_registered`
  and `JobError::is_already_running` (`mimir-core/src/job_queue.rs`).
- **#162** (core): `DailySchedule::parse` now enforces a strict five-character
  `HH:MM` format with zero-padded fields; non-zero-padded inputs like `"2:30"`
  are rejected with `JobError::InvalidSchedule` for config-file determinism.
- **#164** (client): SSE stream parser already caps the buffer at 1 MiB
  (`MAX_SSE_EVENT_SIZE`) and scans delimiters linearly via `memchr` with a
  resume-from-cursor optimization — verified and documented.
- **#165** (client): Added fallible `MimirClient::try_new(base_url,
  connect_timeout, timeout) -> Result<Self, ClientError>`; `new` keeps the
  panicking default for back-compat. Build failures map to
  `ClientError::Connection`.
- **#166** (core): `LlmClient::new` and `new_direct` are now fallible
  (`Result<Self, LlmError>`); added `LlmError::ClientBuild`. Daemon startup
  (`start_server`, `AppState::from_config`) propagates the error instead of
  panicking; pool workers log and exit on a build failure.
- **#167** (client): DRY'd the repeated `check_response` + `resp.json::<T>()`
  pattern behind `send_response`/`send_json`/`get_json`/`post_json`/`check_status`;
  `stop` keeps bespoke 503 handling.
- **#168** (cli): `parse_datetime` interprets offsetless datetimes and
  date-only inputs in the local timezone (sharing the now-public
  `DailySchedule::naive_to_utc_local`); explicit RFC3339 offsets are preserved
  as UTC.

**Breaking (internal API):** `LlmClient::new` / `LlmClient::new_direct` now
return `Result`. Internal callers are updated; the OpenAI-compatible HTTP
endpoint is unaffected.

## [0.60.7] — 2026-06-29

### Fix — Handle already-fired shutdown trigger in `watch_shutdown`

Addressed PR #176 review feedback (CodeRabbit): `watch_shutdown` could miss a
SIGTERM/Ctrl-C fired in the gap between `spawn_os_signal_shutdown` and
`shutdown_tx.subscribe()`. `watch::Receiver::changed()` only wakes on *future*
updates, so a freshly subscribed receiver whose trigger already fired before
subscription would wait indefinitely (until sender drop, which never happens
during serving).

**Fix:** Check the current watch value via `borrow_and_update()` before
awaiting `changed()`. An already-fired trigger returns immediately; later
triggers are still caught by `changed()`.

- `mimir-server/src/lib.rs` — `watch_shutdown` now checks the current value
  first; added regression test
  `test_watch_shutdown_handles_already_fired_trigger`.
- `docs/shutdown.md` — documented the subscription-race guard.

## [0.60.6] — 2026-06-29

### Refactor — Single OS-signal listener for graceful shutdown

Addressed PR #176 review feedback (CodeRabbit): `serve_with_bounded_drain`
previously built two independent `shutdown_signal` futures — one for axum's
`with_graceful_shutdown` and one for the phase-1 serving loop — each
registering its own `ctrl_c()`/`SIGTERM` listener. The phase-1 waiter could
observe a signal before axum's graceful-shutdown future had registered
interest, leaving axum accepting connections until the drain bound kicked in.

**Fix:** Capture Ctrl-C / SIGTERM **once** in a dedicated
`spawn_os_signal_shutdown` task that fans the notification into the shared
`shutdown_tx` watch channel (the same channel `/stop` writes to). Both axum's
graceful-shutdown future and the phase-1 loop now observe that channel via
`watch_shutdown`, so they fire in lockstep with no duplicate OS-signal
listeners.

- `mimir-server/src/lib.rs` — replaced `shutdown_signal` with
  `spawn_os_signal_shutdown` and `watch_shutdown`; updated
  `serve_with_bounded_drain` to use the shared trigger.
- `docs/shutdown.md` — documented the single-listener trigger architecture.


## [0.60.5] — 2026-06-29

### Fix — Daemon no longer self-terminates 30 s after start

The graceful-shutdown drain bound was incorrectly applied to the **entire**
server lifetime: `tokio::time::timeout(Duration::from_secs(30), server_fut)`
wrapped the whole serving future, so the daemon unconditionally exited 30 s
after it began listening — whether or not a shutdown was ever requested. The
first `mimir chat`/`mimir ask` after start worked (inside the 30 s window);
any command issued later failed with `Mimir is not running.` because the
daemon had already exited with status 0 (so `Restart=on-failure` did not
relaunch it).

**Root cause:** `mimir-server/src/lib.rs` bounded the server future instead of
only the post-signal drain phase.

**Fix:** Extracted `serve_with_bounded_drain`, which splits shutdown into two
phases — an **unbounded serve** phase (poll the server concurrently with the
shutdown trigger) and a **drain bounded to `GRACEFUL_DRAIN_TIMEOUT` (30 s)**
phase (applied only after a trigger fires via Ctrl-C, `SIGTERM`, or `/stop`).
A wedged SSE stream can no longer keep the process alive past systemd's
`TimeoutStopSec`, and the daemon no longer dies on a fixed timer.

- `mimir-server/src/lib.rs` — `serve_with_bounded_drain`, `GRACEFUL_DRAIN_TIMEOUT`,
  regression test `test_serve_outlives_drain_timeout`.
- `docs/shutdown.md`, `docs/wiki/daemon-shutdown.md`, `docs/systemd-integration.md`
  — updated to describe the two-phase shutdown and the unbounded serving lifetime.


## [0.60.4] — 2026-06-29

### Docs — Knowledge Graph documentation audit & gap-fill (#64)

Completed the Knowledge Graph documentation set requested by issue #64 by auditing the existing equivalent docs against the issue's required content and filling stale/missing sections, plus adding the two genuinely missing wiki pages. Existing filenames were kept (DRY; avoids breaking cross-references).

**Technical docs (`docs/`):**
- `knowledge-graph-schema.md` — removed the dropped `entity_dates`/`entity_date_types` tables; documented the events & reminders overlay (`event_types`, `event_statuses`, `auto_complete_policies`, `events`, `pending_event_meta`); corrected the `predicates`→`relationship_types`/`relationship_constraints` rename (migration `031`); fixed lookup-row counts (`relation_types` 3→4, `change_types` 7→9); added `optimization_runs`/`optimization_pass_runs`/`memory_priorities`; completed the migration ordering (023–041); replaced the stale "Entity Dates & Recurrence" and "Future Work" sections.
- `Confidence-Model.md` — added a "Why No Time-Based Decay" rationale and a "Confidence Change Events" table mapping each trigger to its `ChangedBy` actor.
- `inference-engine.md` — added a "How to Add a New Rule" section and replaced the stale "Nightly Optimization" stub list with a reference to the implemented 10-pass pipeline.
- `nightly-optimization.md` — added a per-operation "Transaction Model" section, the `JobPriority` levels table, and a "Trigger & Daemon-Down Handling" note.
- `fact-extraction-pipeline.md` — audited; already current (LLM-orchestrated `remember` tool, sensitivity gate, confirm/reject flow).

**Wiki docs (`docs/wiki/`):**
- `knowledge-graph.md` — reframed as the "second brain" distinct from condensed memory; replaced the dropped entity-dates bullet with events & reminders; added an inference key-concept; replaced the stale "Future Commands (Planned)" with real `mimir kb` examples and a "Relationship to the Wider System" section; corrected the semantic-dedup "future work" note.
- `cli-commands.md` — fixed the stale `mimir memory` section (now KG-backed, not a file) and documented `--refresh`.
- `memory.md` — added "What Appears in Memory vs. What Stays in the Knowledge Graph"; fixed the `mimir kg query`→`mimir kb query` typo.
- `forgetting.md` (new) — soft-delete to a 30-day trash bin, restore, cascade forget, and bulk safeguards (>100 `--yes`, sensitive `--confirm-sensitive`, full-reset `DELETE EVERYTHING` + backup).
- `obsidian-sync.md` (new) — planned export/import design and file format, documented as **not yet implemented and deferred to post-Phase-5**.

## [0.60.3] — 2026-06-29

### Fixed — corroboration docs & nightly recalculation efficiency (#79, PR #174)

- **Documented the pending-confirmation corroboration path.** The confidence-model and "what works now" docs described corroboration only against an existing `Active` fact; `insert_fact_in_tx` also corroborates matching `pending_confirmation` facts. Both docs now state `Active` **or** `pending_confirmation`, matching the implementation.
- **Corrected the wiki corroboration cap statement.** Verified the wiki already states the non-explicit corroboration cap as `0.95` (not `1.1`); wording aligned with the pending-confirmation path for accuracy.
- **Nightly `confidence_recalc` skips already-refreshed rows.** Because each root-aware recalculation cascades to inferred descendants and clears their stale flags in one transaction, later iterations in the stale-fact snapshot could reopen transactions and re-walk subtrees already cleared by an ancestor pass. The loop now re-checks `stale_confidence` cheaply before recalculating, avoiding quadratic work on large stale branches.

## [0.60.2] — 2026-06-27

### Fixed — confidence cascade & nightly recalculation correctness (#79, PR #174)

- **Corroboration at the cap clears `stale_confidence`.** A corroborated fact already at the non-explicit cap (`0.95`) had its confidence unchanged, so the previous delta check skipped the whole update and left the row flagged stale despite new provenance. The update that clears `stale_confidence` now runs whenever corroboration applies, while the `ConfidenceChange` audit entry and the descendant cascade remain gated on an actual confidence delta.
- **Cascade uses a recursion-stack guard, not a global visited set.** `cascade_inner_tx` removed a fact from the visited set when its subtree finished, so a descendant reachable through multiple parents (a diamond graph) is recalculated once per updated parent and ends up with the correct final confidence instead of being skipped after the first parent updates.
- **Nightly `confidence_recalc` updates the stale root fact.** The pass previously only cascaded from each stale fact to its children and never recalculated/cleared the selected row itself, so the same facts could stay stale indefinitely. It now uses a root-aware transactional path (`confidence::recalculate_stale_fact`) that recalculates the stale row (inferred) or just clears its flag (non-inferred), writes a `ConfidenceChange` audit entry only when confidence changes, and then cascades to inferred descendants in the same transaction.

## [0.60.1] — 2026-06-27

### Fixed — corroboration guard consistency (#79)

- The corroboration guard in `insert_fact_in_tx` now treats `System`-sourced new facts as explicit, matching the boost-eligibility check and the documented contract. A `System` fact is no longer able to corroborate (and boost) an overlapping non-explicit fact; explicit facts (`UserEdit`/`System`) only add their source and supersede, never corroborate.

## [0.60.0] — 2026-06-27

### Added — corroboration detection in fact insertion (#79)

- **Corroboration is now resolved inside `insert_fact_in_tx`** for every insert
  path (extraction pipeline, batch insert, direct `KnowledgeGraph::insert_fact`),
  within the same transaction as supersession. When a new **non-explicit** fact
  covers the same claim as an existing `Active` (or pending-confirmation) fact —
  same `subject_id + relationship_type_id + object`, temporally overlapping
  `valid_from`/`valid_until` — Mimir adds a source row to the existing fact
  instead of creating a duplicate, and boosts the existing fact's confidence by
  `+0.05` per independent corroborating source, capped at `0.95`.
- **Explicit and inferred facts are excluded from the boost.** Explicit
  (`UserEdit`/`System`) facts stay at `1.0` — corroboration only adds the source
  for provenance. Inferred fact confidence is structural (recalculated from
  parents) and is never boosted by a corroborating source.
- **Re-statements are a no-op.** A source with identical provenance
  (`(source_type, connector_id, raw_reference)`) already recorded against the
  fact is not an independent corroboration and is skipped, which also avoids the
  `sources` UNIQUE-index collision.
- **Non-overlapping temporal ranges never corroborate** — they form a timeline
  of separate facts, matching the existing temporal-facts model.
- **Comprehensive in-transaction confidence cascade.** `cascade_confidence_change`
  is now transaction-aware (`cascade_confidence_change_in_tx`) and runs
  unbounded (cycle-guarded by a `visited` set) so a corroborated confidence
  change propagates to every inferred child for accuracy, with no artificial
  depth cap. The legacy depth-budget parameter was removed (the only caller, the
  nightly optimiser, already ran unbounded).
- **Audit + stale flag.** Corroboration writes `SourceAdded` and
  `ConfidenceChange` audit entries (the latter recording the triggering
  `source_id`) and clears `stale_confidence` on the existing fact.
- **Removed the pre-insertion `find_existing_fact` stub** in `extract.rs` and the
  now-unused `ExtractionOutcome.corroborated` / `ProcessResult::Corroborated`
  plumbing; corroboration is owned by the insert layer.

### Notes

- The `deterministic_dedup_merges_identical_fact_triples` test now sets up its
  duplicate via a direct SQL insert, because live same-claim non-explicit facts
  corroborate at insert time and can no longer coexist. The nightly dedup pass
  remains a safety net for coexisting duplicates (legacy data, direct writes).

## [0.59.1] — 2026-06-25

### Fixed — third pass on PR #173 review feedback

- **`confirm_fact` no longer errors after the confirmation commit.** The
  overlay-rebuild read of `pending_event_meta` ran after `tx.commit()`, so a
  `?`-propagated failure would make confirmation look failed to the caller even
  though the fact was already Active and no longer pending. The read now logs and
  falls back to the legacy one-time overlay path instead of returning an error
  (#3).
- **Legacy-fallback test now exercises the future-dated branch.** The
  `confirm_legacy_pending_fact_falls_back_to_one_time_reminder` test uses a
  future-dated fixture and asserts the one-time `Reminder` overlay is created;
  a second test covers the no-`valid_from` (no-overlay) case (#4).

### Notes

- CodeRabbit findings #1 (NULL confidence) and #2 (`event_type_roundtrips`
  `#[tokio::test]`) are stale re-posts: `facts.confidence` is `NOT NULL` and the
  test attribute is present at line 117. No code change required.

## [0.59.0] — 2026-06-25

### Fixed — second pass on PR #173 review feedback

- **Sensitive facts preserve event metadata across confirmation.** The
  extracted recurrence / `event_type` / `auto_complete_policy` /
  `requires_user_action` are now persisted in a new `pending_event_meta` table at
  extraction time and used by `confirm_fact` to rebuild the overlay faithfully,
  instead of synthesising one-time `Reminder` defaults. A confirmed sensitive
  recurring reminder keeps recurring; a confirmed sensitive task/deadline keeps
  requiring user action and surfaces as overdue. Legacy pending facts that
  predate the table fall back to the one-time `Reminder` overlay. This removes
  the Phase A limitation noted in 0.58.0 (#6).
- **`get_active_recurring` filters past-due rows in SQL.** The advance-pass
  query now takes the scan `now` and adds `trigger_date < now`, so the
  twice-daily scan only loads and sorts rows that can actually advance instead
  of fetching every future recurring event (#7).

### Added

- **Migration 041** — `pending_event_meta` table (fact-keyed event-shape cache
  for pending sensitive facts, removed on confirm / cascade-deleted on reject).
- **Public API:** `queries::event::{PendingEventMeta, insert_pending_event_meta,
  get_pending_event_meta, delete_pending_event_meta}`.

### Notes

- CodeRabbit findings #1 (advance filter) and #3 (`event_type_roundtrips`
  `#[tokio::test]`) were already satisfied by the current code and required no
  change; finding #2 (NULL confidence) is invalid because `facts.confidence` is
  `NOT NULL` and the derive and Upcoming queries already share the
  `confidence >= 0.5` gate.

## [0.58.0] — 2026-06-25

### Fixed — PR #173 review feedback on the events & reminders subsystem

- **Idempotent overlay derivation.** The derive scan now inserts overlays with
  `INSERT ... ON CONFLICT(fact_id) DO NOTHING` and only counts actual inserts,
  so a concurrent extraction can no longer trip the `fact_id` unique constraint
  (#3).
- **Recurring user-action events are no longer auto-advanced.** The advance pass
  now filters to `Recurring`-policy events with `requires_user_action = false`;
  recurring deadlines/tasks stay past their trigger date and surface as overdue,
  matching the documented contract (#4).
- **Sensitive time-bound facts get an overlay on confirmation.** Sensitive facts
  return `Pending` before the event block; `confirm_fact` now derives a one-time
  `AutoCompleteOnDate` overlay for future-dated sensitive facts when they are
  confirmed. Recurrence / `requires_user_action` are not carried across the
  sensitivity gate in Phase A (documented limitation) (#5).
- **Scan / Upcoming confidence alignment.** The derive query now applies the
  same `confidence >= 0.5` gate as the Upcoming render, so overlays are only
  created for facts that will surface (no hidden overlays for low-confidence
  interaction facts) (#7). Note: `facts.confidence` is `NOT NULL`, so the
  original "NULL confidence" framing was revised.
- **Calendar-day relative suffix.** `format_upcoming_line` computes the
  `today` / `in N days` suffix from `date_naive()` differences, so an event
  early the next calendar day is no longer mislabelled `today` (#8).
- **Docs.** `RecurringYearly` references in `docs/events-reminders.md` updated
  to `Recurring` to match the renamed policy (#1).

### Added

- **Env overrides for events.** `MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES`
  (comma-separated `HH:MM`) and `MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS` now flow
  through `apply_env_overrides_with`, matching the rest of the config (#2).
- **Public API:** `KnowledgeGraph::insert_event_if_absent` and
  `queries::event::insert_event_if_absent` for idempotent overlay creation.

### Notes

- `event_type` from extraction is intentionally limited to `Task`/`Reminder` in
  Phase A; the remaining `EventType` variants are seeded for later phases (#6).
## [0.57.0] — 2026-06-25

### Feature — events & reminders subsystem (issue #74)

A smart events/reminders layer for the knowledge graph. Events are modelled as
a lifecycle + recurrence overlay on facts: a fact with a future `valid_from` is
a one-time event; a fact tagged with recurrence (e.g. a birthday) is a
recurring event; a fact flagged `requires_user_action` is a task/deadline.
Upcoming events surface automatically in the "Upcoming" memory section.

- **New `events` table** (migration 039) with `event_types`, `event_statuses`,
  and `auto_complete_policies` lookup tables, keyed on `facts(id)`.
- **`entity_dates` deprecated and removed.** Its recurrence logic
  (`next_occurrence`) moved to `models::recurrence`; the unused
  `entity_dates` / `entity_date_types` tables are dropped (migration 040, no
  data migration required).
- **Deterministic scan job** `events.upcoming_scan` (default 06:00 & 18:00)
  derives overlays for future-dated facts, auto-completes one-time events past
  their trigger date, and advances recurring events to their next occurrence.
  `RequiresUserAction` events stay active and surface as overdue.
- **Extraction bridge.** The `remember` tool schema gains optional `recurrence`
  and `requires_user_action` fields; the extraction pipeline creates event
  overlays for qualifying facts (no natural-language date parsing in Rust —
  the LLM supplies the ISO-8601 `valid_from`).
- **`render_upcoming_section` refactored** to an event-based query, replacing
  the `entity_dates` and category 900–999 branches.
- **Config:** new `[knowledge.events]` section (`schedule_times`,
  `horizon_days`).

## [0.56.2] — 2026-06-24

### Bugfix — daemon service reliability

Fixes the three interlocking issues that made the installed `mimir.service`
fail to start cleanly and intermittently stop itself.

- **CLI no longer targets the wrong port.** Client commands resolved their
  base URL from a hardcoded `http://127.0.0.1:8080` (or `MIMIR_BASE_URL`),
  ignoring the configured `server.bind_addr`. A daemon bound to, e.g.
  `0.0.0.0:8008` was therefore reported as "not running", the daemon guard
  prompted to start an already-running service, and the auto-spawned duplicate
  failed to bind (address in use). The CLI now resolves `MIMIR_BASE_URL` →
  `server.bind_addr` (wildcard hosts normalised to loopback) → compiled default
  (`mimir/src/constants.rs`, `mimir-core::config::resolve_base_url`).
- **Cheap `/health` liveness endpoint.** The daemon guard and `mimir stop`
  probed `/status`, which performs a live LLM round-trip
  (`fetch_model_context_window`) plus knowledge-graph reads on every call. A
  slow/unreachable provider made the 500 ms probe time out on a healthy
  daemon. Added `GET /health` (trivial 200, no LLM/DB work) and pointed the
  guard + reachability check at it (`mimir/src/daemon_guard.rs`,
  `mimir-server/src/lib.rs`).
- **SIGTERM shutdown no longer deadlocks.** Only `POST /stop` broadcast the
  `shutdown_tx` watch channel; the SIGTERM/Ctrl-C path relied on `AppState`
  being dropped during runtime teardown to release the config file-watcher's
  `spawn_blocking` thread — a race that, when lost, deadlocked tokio's
  `BlockingPool::shutdown` until systemd aborted the unit with `SIGABRT` after
  `TimeoutStopSec` (the "it stops itself" symptom). The server now broadcasts
  `shutdown_tx` deterministically while the runtime is still alive, and wraps
  the server future in the documented 30 s `tokio::time::timeout`
  (`mimir-server/src/lib.rs`).
- **Config file-watcher no longer floods the journal.** Reading the config
  file generated `Access`/close events that, with only a filename filter, fed a
  self-reload loop (~1 reload/second even with no real change). The watcher now
  ignores `Access` events and dedupes by `(mtime, size)` so each genuine
  content change reloads at most once (`mimir-server/src/lib.rs`).

### Tests

- `mimir-core::config::base_url_tests` — base-URL resolution and config
  `bind_addr` reading (12 cases).
- `mimir-server::tests::test_health_returns_ok_without_llm` — `/health` does
  not touch the LLM backend.
- `mimir/tests/e2e.rs::e2e_sigterm_exits_promptly` — the real binary exits
  promptly on SIGTERM under an isolated environment.

## [0.56.1] — 2026-06-23

### Bugfix

- **Tool-call-start JSON printed to console.** The server emits a
  `tool_call_start` SSE event (containing `name` and `display_name`) before a
  tool executes, but the client SSE parser had no arm for that event type, so
  the raw JSON payload fell through to the default text path and was printed
  verbatim alongside the formatted result line. Added a `ToolCallStartInfo`
  type and `StreamItem::ToolCallStart` variant, a matching parser arm, and CLI
  handling so the event renders as a dim "🔧 DisplayName…" indicator instead
  of leaking JSON.

## [0.56.0] — 2026-06-23

### Bug & Performance Sweep

Address all open `bug` and `performance` labelled issues. Each was verified
before fixing; performance changes include before/after measurements.

#### Bugs

- **#45 — `get_current_time` returns UTC instead of user's time zone.** The
  tool now returns a structured payload (`local`, `utc`, `offset`) derived
  from the host's local timezone via `chrono::Local`, so the agent can derive
  UTC from the offset. The formatting helper is generic over the timezone for
  deterministic unit testing.
- **#80 — Some config settings don't do anything (temperature).** The LLM
  client captured `temperature` at startup, so hot-reloaded changes had no
  effect. Added `LlmBackend::with_temperature_override`; the chat route now
  applies the live config snapshot temperature per request.
- **#81 — Certain CLI commands don't work.** `mimir chat` accepted no flags
  and always sent `model`/`personality`/`incognito` as `None`, ignoring
  `--verbose`. Added `--model`, `--verbose`, `--incognito`, `--personality`
  flags plus REPL slash-commands (`/model`, `/personality`, `/incognito`,
  `/verbose`) that toggle at runtime; verbose now reports token usage.
- **#155 — Incognito mode can still write facts via `remember`.** Added a
  `Tool::is_write_tool` marker (default `false`); `RememberTool` opts in. The
  chat routes now suppress write-capable tools from the exported tool set and
  refuse to execute them during incognito turns, so no facts are persisted.

#### Performance

- **#160 — `api-types` leaks `null` fields in KG wire types.** Added
  `#[serde(skip_serializing_if = "Option::is_none")]` to the sparse `Option`
  fields of `AuditRow`, `CategoryResponse`, `CategoryDetailResponse`,
  `TrashRow`, `OptimizationStatusResponse`, `OptimizationRunNowResponse`, and
  `OptimizationRunSummary`. Sparse payload sizes shrink 42–75% per row
  (e.g. a sparse `AuditRow` is 64 B vs 123 B; a sparse `CategoryResponse` is
  19 B vs 76 B).
- **#163 — `escape_fts5` keeps leading/trailing whitespace in the phrase.**
  The quoted phrase now uses the trimmed value so padded queries no longer
  alter FTS5 phrase-matching semantics. `fts5_escape_mixed_inputs` benchmark:
  1.41 µs → 1.26 µs (~7% incidental).
- **#164 — SSE stream parser has unbounded buffer growth and O(n²) scan.**
  The client SSE parser now caps a single event at 1 MiB (emitting a
  `ClientError` on overflow) and resumes the delimiter scan from the last
  inspected offset (using `memchr::memmem`), making the cost linear in the
  event size. Benchmark on the partial-event accumulation path:
  - 1024 chunks: 31.5 ms → 542 µs (~58×)
  - 4096 chunks: 494.9 ms → 1.21 ms (~408×)

### Notes

- `memchr` 2.8 added to `mimir-client`.
- Five pre-existing `mimir-server` KB pending/confirm/reject tests fail on
  `main` independently of this change (index-out-of-bounds in
  `insert_pending_fact`); left untouched per scope.

## [0.55.1] — 2026-06-23

### Sensitivity Content Check: Word-Boundary Matching (#142)

Fix a false-positive vector flagged in PR review. The keyword-based content
fallback (`is_sensitive_by_content`) previously matched keywords as raw
substrings, so benign words containing a sensitive keyword (e.g. "hospitality"
contains "hospital", "indebted" contains "debt", "visage" contains "visa")
could be confirmed sensitive whenever the LLM also set `is_sensitive=true`.

- **`mimir-knowledge/src/sensitivity.rs`:** `is_sensitive_by_content` now
  matches each keyword as a whole word using ASCII alphanumeric boundaries via
  the new private `contains_keyword_word` helper, eliminating embedded-word
  false positives while still catching genuine single-word uses like "diabetes"
  or "allergic".
- **Tests:** Added word-boundary regression tests for "hospitality", "indebted",
  and "visage", plus a trailing-punctuation case and a genuine "hospital" word
  case.

## [0.55.0] — 2026-06-22

### Rework Sensitivity Detection (#142)

Move sensitivity detection from LLM-only to deterministic Rust validation,
eliminating false positives where benign preferences were routed into the
pending-confirmation dead end.

- **New module `mimir-knowledge/src/sensitivity.rs`:** Pure, synchronous
  sensitivity gate with two signals:
  - `is_sensitive_by_category(category_ids)` — checks the fact's catalogue
    category IDs against the `SENSITIVE_CATEGORIES` constant (health, allergies,
    financial, romantic, cultural/religious, values/philosophy).
  - `is_sensitive_by_content(object)` — keyword-based fallback for
    miscategorised facts (e.g. "allergic", "diabetes", "salary", "debt",
    "divorce", "citizenship").
  - `is_sensitive(llm_flag, category_ids, object)` — combined AND gate: a fact
    is sensitive only if the LLM flags it **and** Rust agrees. Rust can narrow
    but never widen.
- **Extraction prompt softened:** "Flag ... Mimir will validate your assessment
  in Rust."
- **Sensitivity check wired into `process_extracted_fact`** — the single funnel
  point covering `extract_facts`, `extract_facts_with_context`, and
  `process_remember_output`.
- **35 unit tests + 7 integration tests** covering all issue acceptance
  criteria.

## [0.54.5] — 2026-06-22

### Review Fixes (PR #169)

Address CodeRabbit review feedback on the tests-and-benchmarks change set.

- **`mimir-api-types`:** `roundtrip_tests!` sparse-field check now parses the
  serialised JSON into a `serde_json::Map` and asserts key absence via
  `contains_key`, instead of the previous substring-based `json.contains`
  that could match field names inside values.
- **`mimir-client`:** wiremock-backed KB endpoint tests
  (`kb_query`, `kb_browse`, `kb_profile`, `kb_audit`, `kb_trash`) now assert
  the expected query-string parameters via `query_param` matchers, catching
  regressions in query encoding rather than just the route path.
- **`mimir-core`:** `bench_daily_schedule_next_after` uses a fixed
  `DateTime<Utc>` reference instead of `Utc::now()`, so the benchmark baseline
  is deterministic and reproducible across runs. Added
  `daily_schedule_parse_accepts_non_zero_padded_input` to document chrono's
  padding-agnostic `%H:%M` parsing.
- **`mimir` (binary):** Corrected the misleading comment in
  `truncate_zero_max_yields_just_ellipsis_or_empty` to state the deterministic
  "just ellipsis" outcome.

## [0.54.4] — 2026-06-21

### Testing & Benchmarks

Workspace-wide expansion of inline unit tests and pure-helper benchmarks on
the `tests-and-benchmarks` branch.

- **`mimir-api-types`:** 12 → 46 unit tests. New `roundtrip_tests!` macro
  asserts populated + sparse (all-`None`) serde roundtrips and
  `skip_serializing_if` omission for every KG wire type.
- **`mimir-client`:** ~24 → 64 tests. Adds pure unit tests for the SSE parser
  primitives (`find_double_newline`, `parse_sse_event`) and wiremock-backed
  tests for all previously-uncovered `MimirClient` methods.
- **`mimir-core`:** 179 → 211 lib tests. New inline tests for `job_queue`,
  `tools::{output,permission,error}` pure helpers.
- **`mimir-knowledge`:** ~74 → 110 lib tests. New inline tests for
  `models::enums`, `retrieval::types`, `inference::rules::transitivity`,
  `models::{entity_date,memory}` helpers.
- **`mimir-server`:** 50 → 65 lib tests. New `error.rs` tests covering every
  `ApiError` response helper, including verification that internal error
  details are masked from clients.
- **`mimir` (binary):** 15 → 29 bin tests. New `kb.rs` tests for
  `parse_datetime`, `confidence_color`, and `truncate`.
- **Benchmarks:** three new pure-helper suites — `mimir-api-types/wire_types`,
  `mimir-core/pure_helpers`, `mimir-knowledge/pure_helpers` — covering
  non-hotpath pathways (FTS5 escaping, confidence scoring, serde roundtrips,
  schedule arithmetic, tool-output rendering).

### Documentation

- New `docs/unit-tests.md` and `docs/wiki/Testing-and-Benchmarks.md`;
  `docs/benchmarks.md` updated with the new pure-helper suites.

### Issues

Triaged nine prescriptive follow-ups as GitHub issues #160–#168
(api-types `skip_serializing_if` consistency, doc-comment completion,
`DailySchedule::parse` strictness, `escape_fts5` whitespace, SSE buffer DoS,
client/LLM construction robustness, client DRY, `parse_datetime` timezone).

## [0.54.3] — 2026-06-21

### Security

- **`mimir-server`:** the sensitive-fact confirmation lifecycle routes
  (`GET /kb/pending`, `POST /kb/facts/{id}/confirm`, `POST /kb/facts/{id}/reject`)
  are now wrapped in the `require_loopback` middleware, matching the guard
  already applied to `/kb/optimization/run-now`, `/memory/refresh`, and `/stop`.
  Only loopback peers can list or mutate pending sensitive facts. No CSRF /
  `Origin` validation is applied because there is no browser frontend for these
  routes (the CLI / `mimir-client` is the only client); that hardening belongs
  to a workspace-wide pass over all mutation routes.

## [0.54.2] — 2026-06-21

### Fixed

- **`mimir-knowledge`:** `extract::reject_fact` now clears `fact_dependencies`
  rows before the hard-delete. The `fact_dependencies` FK is `ON DELETE
  RESTRICT` (migration 017), so rejecting a pending sensitive fact that
  participates in a dependency edge previously hit a foreign-key violation.
  Mirrors the dependency cleanup already performed by
  `KnowledgeGraph::delete_stale_pending` and `forget_fact_tx`.
- **`mimir-knowledge`:** `KnowledgeGraph::delete_stale_pending` now re-checks
  the stale predicate inside each per-fact transaction and only counts
  committed deletes. A fact confirmed or rejected between the id scan and the
  delete is skipped rather than incorrectly hard-deleted and given a spurious
  `Rejected` audit entry.
- **`mimir-knowledge`:** the optimization runner's `pending_confirmation_cleanup`
  pass now uses the configured `knowledge.pending_cleanup.retention_days`
  (via a new `OptimizationConfig.pending_cleanup_retention_days` field) instead
  of a hardcoded 7 days, so the pass and the scheduled `knowledge.pending_cleanup`
  job share one configured expiry window.
- **docs:** `docs/wiki/facts.md` confirm/reject examples now use the positional
  `<fact-id>` syntax, matching `cli-commands.md` and `README.md`.

## [0.54.1] — 2026-06-21

### Fixed

- **`mimir-knowledge`:** removed the orphaned, never-called
  `queries::fact::delete_stale_pending` helper. It duplicated
  `KnowledgeGraph::delete_stale_pending` with divergent, FK-violating semantics
  (skipped `fact_dependencies` cleanup and the `Rejected` audit entry).
  `KnowledgeGraph::delete_stale_pending` is now the single source of truth for
  stale pending-fact auto-expiry.

## [0.54.0] — 2026-06-21

### Added

- **Pending sensitive-fact confirmation lifecycle (`mimir-server`, `mimir-client`,
  `mimir`, `mimir-api-types`, `mimir-knowledge`):** the existing internal
  `confirm_fact`/`reject_fact` APIs are now exposed end-to-end. Sensitive facts
  (allergies, health, etc.) stored with `pending_confirmation = TRUE` no longer
  sit in limbo.
  - HTTP routes: `GET /kb/pending`, `POST /kb/facts/{id}/confirm`,
    `POST /kb/facts/{id}/reject` (returns `204 No Content`; optional `reason`
    body field written to the audit log).
  - CLI commands: `mimir kb pending`, `mimir kb confirm --fact-id N`,
    `mimir kb reject --fact-id N [--reason "..."]`.
  - API types: `PendingListResponse`, `ConfirmFactResponse`, `RejectFactRequest`,
    and a public `PendingFactRow`.
  - New `KnowledgeGraph::list_pending_facts()` and `delete_stale_pending()` query
    methods.

- **Daily pending-fact auto-cleanup job (`mimir-server`):** a new
  `knowledge.pending_cleanup` background job hard-deletes facts still awaiting
  confirmation past a configurable retention window. Configurable under
  `[knowledge.pending_cleanup]` with `retention_days` (default `7`) and
  `schedule_time` (default `"03:30"`). Implements the 7-day auto-deletion rule
  described in `VISION/02-Knowledge-Graph/Learning-Modes.md`.

### Changed

- **`reject_fact` now accepts an optional reason (`mimir-knowledge`):** the
  free function and `KnowledgeGraph` method take `reason: Option<&str>`,
  threaded through to the audit log. Internal API change (acceptable per the
  breaking-changes policy).


## [0.53.1] — 2026-06-19

### Fixed

- **Librarian transcript now escapes newlines in message content
  (`mimir-knowledge/src/extract.rs`):** `\r` and `\n` in `msg.content` are
  replaced with literal `\r`/`\n` sequences before the `[Role]:` label is
  applied, preventing a user message containing text like
  `[Assistant]: …` from forging a labelled line and bypassing the source
  discipline boundary. Adds a regression test
  (`prompt_escapes_multiline_content_so_roles_cannot_be_forged`).

### Changed

- **Librarian wiki documentation aligned with implemented novelty check
  (`docs/wiki/librarian-agent.md`):** the duplicate-handling paragraph now
  states that facts restating the core-facts block are skipped (not that
  confidence is "strengthened"), matching the novelty check instruction.

### Tests

- **Assert message-turn shape before indexing (`mimir-knowledge/tests/librarian_agent.rs`):**
  the `calls[0].len() == 2` assertion now precedes the indexing into
  `calls[0][0]`/`calls[0][1]` so a shape change fails clearly instead of
  panicking out-of-bounds.

## [0.53.0] — 2026-06-19

### Changed

- **Librarian extraction prompt redesigned (Issue #139):** the Librarian's
  `build_extraction_prompt` now composes a KG-focused base (rules, category
  taxonomy, predicate standards, list splitting, deduplication, output contract
  — extracted into a shared `build_base_prompt`) with the *same* core-facts
  block the core agent injects (`Personality::CORE_FACTS_HEADER` + condensed
  memory, emitted only when non-empty) and the recent conversation rendered as
  labelled `[User]` / `[Assistant]` messages under `## Recent conversation`.
  The user's identity is read from the core-facts block by the LLM, exactly as
  the core agent resolves identity — `UserIdentity` is no longer threaded
  through the contextual extraction path. A "Source discipline" instruction
  tells the Librarian to extract facts ONLY from `[User]` messages and never
  from `[Assistant]` messages (its own prior output), and a "Novelty check"
  instruction tells it to extract only facts not already present in the
  core-facts block. The instruction tells the LLM to skip emitting facts that
  merely restate what is already known (exact duplicates are discarded by Rust
  regardless of classification), and to use the Correction classification for
  corrections — avoiding contradictory "strengthen confidence" guidance.
  The transcript now lives in the system prompt once; the user turn handed to
  the LLM is a short action instruction, removing the previous duplication.

### Added

- `mimir_core::conversation::{ConversationMessage, MessageRole}` — a labelled
  transcript message type. `extract_facts_with_context` and
  `KnowledgeGraph::extract_facts_with_context` now take `&[ConversationMessage]`
  instead of a `ConversationTurn` + `UserIdentity`, so the amount of
  conversation context handed to the Librarian can be increased in future
  without changing the prompt-builder signature. `LibrarianAgent::run`
  converts the turn into `[User, Assistant]` messages today.
- `Personality::CORE_FACTS_HEADER` is now `pub` so the Librarian reuses the
  core agent's core-facts label (DRY).

### Removed

- `build_contextual_extraction_prompt` (folded into `build_extraction_prompt`).
- The "Recent related facts about the user" DB snapshot
  (`get_facts_by_subject`) from the Librarian prompt — novelty checking now
  relies on the core-facts block.
- `identity: UserIdentity` field from `LibrarianContext` and the
  `identity` parameter from `extract_facts_with_context`.

### Notes

- This deviates from the original #139 spec (which was outdated): identity is
  not rendered as a separate prompt line, there is no dedicated recent-facts
  snapshot section, and the exact `## What I already know about you` /
  `## Recently learned` / `## Conversation to analyze` headers from the issue
  are not used. Pronoun-resolution prompting is deferred to the follow-up
  "Phase 2: Pronoun resolution in fact extraction" issue.

## [0.52.0] — 2026-06-18

### Changed

- **System prompt hardened for the agentic architecture (Issue #138):** the
  system prompt composed by `Personality::system_prompt` now appends shared
  operating directives to every preset (built-in and custom). The directives
  tell the LLM not to invent facts about the user (say so if the answer is not
  known), to dispatch a retrieval agent via the `retrieve_context` tool when
  the core facts are insufficient (refining and re-dispatching until answered
  or confirmed absent), and to call the `remember` tool for explicit
  assertions, corrections, and meaningful casual mentions — never for chitchat.
  The injected memory section is relabelled `Core facts about the user` (third
  person, framed as a condensed subset, not exhaustive). The legacy `Key facts
  I know about you:` block and its note mentioning `kg_query`/`kg_search`/
  `kg_related` are removed; those tools are the retrieval agent's internal
  tools and are no longer surfaced to the core LLM. The four `built_in_*`
  presets keep their tone text unchanged — directives are composed once in
  `system_prompt` for DRY.

### Notes

- **#138 acceptance criteria revised by design:** the "you will receive a
  synthesized context block" criterion is dropped — Mimir uses LLM-condensed
  core facts, not a Rust distillation layer (#129 will not ship as written).
  The "remove the `remember` instruction" criterion is reversed to *encourage*
  `remember`, matching the #137 inline-LLM-orchestrated learning design and the
  `test_chat_extracts_facts_after_response` contract. An automatic Librarian
  fallback (#156) was filed to queue background extraction when `remember` is
  not called for a configurable number of turns.

## [0.51.0] — 2026-06-18

### Changed

- **Learning is now LLM-orchestrated (Issue #137):** the unconditional
  background Librarian that re-extracted facts after every non-incognito chat
  turn has been retired. Fact learning now happens when the conversational LLM
  calls the `remember` tool inline while composing its reply, and pre-response
  retrieval stays LLM-driven via the `retrieve_context` tool. The LLM decides
  *whether* to learn/retrieve; Rust still owns the policy (confidence
  assignment, overwrite rules, sensitive-fact confirmation) via
  `process_remember_output`, so the model cannot self-assign confidence or
  override policy. This reframes issue #137 away from a Rust rule-based intent
  classifier — NLU is the LLM's job, and orchestration emerges from structured
  tool selection.
- **`remember` tool description** now summarises the classification semantics
  (Explicit overwrites, Casual coexists, Correction supersedes) and nudges
  canonical relationship types, preserving extraction quality without a second
  LLM call.

### Removed

- `submit_librarian_goal` and its two call sites in the chat route. The
  `LibrarianAgent`, `LibrarianGoal`/`LibrarianContext`, and
  `KnowledgeGraph::extract_facts_with_context` remain as a library API for
  future on-demand/bulk extraction; they are simply no longer auto-invoked.

### Notes

- End-user-visible behaviour: Mimir no longer silently learns from chitchat. It
  learns when it judges a turn contains worth-remembering information.
- Sensitive facts still require confirmation; the overwrite/coexistence matrix
  in `VISION/02-Knowledge-Graph/Learning-Modes.md` is unchanged and enforced in
  Rust.

## [0.50.0] — 2026-06-18

### Changed

- **Predicate resolution is fully data-driven (Issue #136):** the deprecated
  hardcoded `normalize_predicate` synonym map and the duplicate
  `normalize_relationship_type` snake_case helper have been removed from
  `mimir-knowledge/src/extract.rs`. The extraction pipeline now resolves every
  fact's `relationship_type` through `KnowledgeGraph::ensure_relationship_type`,
  which consults the `relationship_type_aliases` table (seeded by migrations
  `036`/`037`) and auto-registers unknown predicates as new canonical types.
- **DRY batch processing:** `process_extracted_facts` and
  `process_remember_output` now share a single `process_fact_batch` helper, and
  predicate normalization reuses `normalize_alias` from `mimir-knowledge/src/lib.rs`
  instead of a local copy. Predicate-resolution errors are tolerated per-fact, so
  one malformed predicate no longer aborts the whole extraction batch.

### Notes

- End-user behaviour is unchanged: `attended`→`studied_at`, `hobbies`→`hobby`,
  `works_for`→`works_at`, etc. all resolve via seeded aliases.
- Side effect of routing through `ensure_relationship_type`: an unknown predicate
  on a fact that is later rejected (e.g. invalid `subject_type`) still registers its
  canonical type. This is intentional and idempotent.

## [0.49.1] — 2026-06-18

### Fixed

- **Address PR #152 review feedback (Issue #135 ontology seed):**
  - Migration `037` now uses `ON CONFLICT` UPSERTs (not `INSERT OR IGNORE`) for the
    canonical predicates and their self-aliases, enforcing the canonical `(id, name)`
    contract on upgrade instead of silently preserving stale mappings.
  - Migration `038` runs inside a transaction with foreign-key enforcement on
    (`PRAGMA foreign_keys = OFF` removed) and uses `CREATE TABLE/INDEX IF NOT EXISTS`
    for defensive idempotency.
  - `insert_category_alias` now uses an atomic `INSERT OR IGNORE` + post-insert
    resolution, eliminating the `SELECT`-then-`INSERT` race that could surface raw
    `UNIQUE`-constraint errors instead of the documented `Validation` error.
  - `category_aliases_test` re-queries the subtree after inserting the unrelated
    fact so the exclusion assertion is meaningful.
  - `relationship_ontology_test` self-alias check is now read-only (direct canonical
    id lookup) instead of mutating the DB via `ensure_relationship_type`.

## [0.49.0] — 2026-06-18

### Added

- **Core relationship ontology (category-first, Issue #135)**: the knowledge graph is
  now seeded with a category-first ontology. Predicate aliases own verb canonicalization
  (thin canonical verbs + English synonyms); the Dewey `categories` tree owns grouping,
  hierarchy, and multi-tag precision.
  - Migration `037` seeds the remaining core predicates (`studied`, `completed_degree`,
    `educational_status`, `job_title`, `likes`, `dislikes`) with explicit ids 26–31 and
    self-aliases, so the alias table remains the single source of truth for resolution.
  - Migration `038` adds the `category_aliases` table (`alias` → `category_id`,
    globally unique) and seeds domain words (`education`, `hobbies`, `residence`,
    `family`, `identity`, `employment`, `pets`, …) mapping to existing Dewey category
    nodes. Both migrations are idempotent (`INSERT OR IGNORE`).
  - New `queries::category` helpers: `resolve_category_alias`, `insert_category_alias`,
    `get_descendant_category_ids` (recursive CTE over `categories.parent_id`), and
    `get_facts_in_category_subtree` (facts tagged anywhere in a root + descendants).
    `KnowledgeGraph` exposes thin wrappers for each.
  - Unit tests verify predicate/alias counts, alias resolution, category-alias counts,
    subtree retrieval, and idempotency across re-init.

### Changed

- **Design shift documented**: grouping/hierarchy is intentionally served by categories,
  not abstract parent predicates. `relationship_type_hierarchy` is kept but no longer
  seeded with abstract parents; reworking `kg_query --include_subtree` to expand by
  category subtree (rather than the predicate DAG) is a tracked follow-up (#134, #136).
- Updated `docs/knowledge-graph-schema.md`, `docs/wiki/what-works-now.md`, new
  `docs/wiki/categories-and-aliases.md`, and `README.md` to reflect the category-first
  layering.

## [0.48.1] — 2026-06-18

### Fixed

- **`kg_query` subtree offset contract**: when `include_subtree` is `true` and the
  requested predicate does not exist (empty result set), the response `offset` is now
  forced to `0` instead of echoing the caller-supplied `offset`. This closes a gap where
  the documented "subtree mode disables offset pagination" contract was only honoured on
  the populated-result path.

## [0.48.0] — 2026-06-17

### Added

- **Relationship-type DAG subtree query (Issue #134)**: facts can now be retrieved
  for a relationship type and all of its descendants in the
  `relationship_type_hierarchy` DAG via a SQLite recursive CTE. Querying a broad
  category (e.g. `education`) returns facts stored under more specific descendant
  types (`studied_at`, `graduated_from`, …) without the caller needing to know every
  predicate name.
  - `queries::fact::get_facts_by_relationship_subtree(pool, subject_id, root_type_id,
    min_confidence, limit)` and the matching `count_facts_by_relationship_subtree` walk
    the DAG in a single statement, seeding the CTE with the root type so its own facts
    are included. Filters and ordering match `get_facts_by_subject_filtered` (non-pending,
    status `NOT IN (5, 6)`, confidence floor, sorted by confidence descending).
  - `KnowledgeGraph::get_facts_by_relationship_subtree(entity_id, root_type_id, limit)`
    is a convenience wrapper with `min_confidence = 0.0`.
  - `kg_query` gains an `include_subtree` boolean parameter (default `false`). When set
    with a `predicate`, the predicate (alias-aware) becomes the subtree root; an unknown
    predicate returns an empty result set, and `include_subtree` without a `predicate`
    is rejected with `ToolError::InvalidArguments`.

### Changed

- Extracted a shared `enrich_with_sources` helper in `queries/fact.rs` so the exact-match
  and subtree fact queries share the source-batching logic (DRY).

### Tests

- Added `mimir-knowledge/tests/relationship_subtree_test.rs` covering subtree inclusion of
  root + descendants, diamond-path deduplication, status/pending/confidence/limit filters,
  temporal-bound preservation, multi-valued same-type facts, the `KnowledgeGraph` wrapper,
  and the `kg_query` `include_subtree` parameter (including alias resolution and the
  predicate-required contract).

### Documentation

- Updated `docs/kg-tools.md`, `docs/knowledge-graph-schema.md`, `docs/wiki/kg-tools.md`,
  and `docs/wiki/knowledge-graph.md` with the subtree query and `include_subtree` parameter.

## [0.47.0] — 2026-06-16

### Added

- **Relationship type alias resolution (Issue #133)**: `ensure_relationship_type` now resolves
  incoming names through the `relationship_type_aliases` table before creating a new canonical
  type. New canonical types automatically register their normalized name as a self-alias, making
  the alias table the single source of truth for relationship-type lookup.
- Migration `036_seed_relationship_type_aliases.sql` backfills self-aliases for every existing
  relationship type and seeds the legacy hardcoded synonyms from `extract.rs::normalize_predicate`
  (e.g., `attended` → `studied_at`) as data-driven aliases.

### Changed

- `mimir-knowledge/src/extract.rs::normalize_predicate` is now deprecated. Fact extraction
  normalizes predicates to snake_case and resolves aliases through the alias table before list
  expansion; the hardcoded synonym map remains only as a deprecated fallback.
- `get_relationship_type_id` now resolves aliases through `relationship_type_aliases`, matching
  `ensure_relationship_type` behavior.

### Tests

- Added `ensure_relationship_type_resolves_alias_to_canonical` and
  `ensure_relationship_type_creates_new_type_and_self_alias` to
  `mimir-knowledge/tests/relationship_type_dag_test.rs`.
- Updated existing tests and lookup-sync expectations to account for the seeded relationship
  type ontology.

### Documentation

- Updated `docs/knowledge-graph-schema.md`, `docs/wiki/knowledge-graph.md`, and
  `docs/fact-extraction-pipeline.md` to describe alias-aware resolution and the deprecated
  hardcoded fallback.

## [0.46.1] — 2026-06-16

### Fixed

- **Relationship type alias/canonical collision checks (PR #149 review)**: centralised
  alias↔canonical collision validation in `mimir-knowledge` and applied it inside the
  same transaction for every relationship-type write path (`ensure_relationship_type`,
  `ensure_relationship_type_in_tx`, `insert_relationship_type`, and
  `insert_relationship_type_alias`). Previously these checks could be bypassed when
  creating relationship types directly or through the transactional fact-insert path.

### Tests

- Added `relationship_type_dag_test.rs` cases covering:
  - `insert_relationship_type` rejecting a canonical name that shadows an existing alias.
  - `insert_relationship_type` rejecting an alias that shadows an existing canonical name.
  - `insert_facts_batch` (transactional create path) rejecting a relationship type name
    that shadows an existing alias.

### Documentation

- Updated `docs/knowledge-graph-schema.md` with the collision invariants section.
- Updated `docs/wiki/knowledge-graph.md` with a relationship types overview.

## [0.46.0] — 2026-06-16

### Added

- **Relationship type DAG schema (Issue #132)**: added `relationship_type_hierarchy`
  and `relationship_type_aliases` tables to `mimir-knowledge`. Relationship types now
  support a directed acyclic graph (multiple parents allowed) and globally unique English
  aliases, enabling data-driven predicate discovery instead of hardcoded synonym tables.
- `RelationshipType` and `NewRelationshipType` models in `mimir-knowledge/src/models/relationship_type.rs`.
- New `KnowledgeGraph` API methods: `insert_relationship_type_hierarchy`,
  `insert_relationship_type_alias`, `resolve_relationship_type_alias`,
  `get_descendant_relationship_type_ids`, and `get_ancestor_relationship_type_ids`.
- Cycle detection for hierarchy inserts, returning `KnowledgeError::RelationshipTypeCycle`.
- Alias resolution integrated into fact extraction; the legacy hardcoded `normalize_predicate`
  map remains as a deprecated fallback until the core ontology is seeded.

### Tests

- New `mimir-knowledge/tests/relationship_type_dag_test.rs` covering migrations,
  DAG traversal, alias resolution, global alias uniqueness, self-loops, indirect
  cycles, and alias-based predicate normalization.
- Updated `mimir-knowledge/tests/migrations_test.rs` to assert the new tables exist.

### Documentation

- Updated `docs/knowledge-graph-schema.md` with the relationship type DAG design.
- Updated `docs/wiki/what-works-now.md` to list the new DAG + aliases feature.
- Updated `README.md` to mention the relationship ontology layer.

## [0.45.1] — 2026-06-15

### Fixed

- `ConversationTurn` equality and hashing now ignore the `timestamp` field, restoring
  `AgentRuntime` deduplication of identical `(agent kind, goal)` pairs.
- `AgentRuntime::submit` always removes the pending goal key, even when the agent task
  panics, preventing permanent leaks in the pending set.
- Chat routes skip Librarian fact extraction when the user message is empty, avoiding
  wasted LLM calls for empty chat turns.

## [0.45.0] — 2026-06-15

### Added

- **Librarian Agent (Issue #130)**: Replaced the fire-and-forget `spawn_fact_extraction` helper with a reusable background agent. The `Agent` trait and `AgentRuntime` live in `mimir-core`; `LibrarianAgent` lives in `mimir-knowledge`. After each non-incognito chat turn, the route submits a `LibrarianGoal` carrying the full `ConversationTurn`, and the agent extracts facts using the configured user identity, condensed memory, and recent related facts.
- New shared types: `mimir_core::conversation::ConversationTurn` and `mimir_core::identity::UserIdentity`.
- New extraction entrypoint: `mimir_knowledge::KnowledgeGraph::extract_facts_with_context` builds a rich contextual prompt for the `remember` tool.
- New integration tests in `mimir-knowledge/tests/librarian_agent.rs` verify fact extraction from a conversation turn and prompt content.

### Changed

- Chat routes (`/chat` and `/chat/stream`) now submit a Librarian goal instead of spawning `spawn_fact_extraction` directly.
- `test_state_with_config` in `mimir-server` now resolves or creates the configured user entity so background agents can run in server integration tests.

### Documentation

- Added `docs/librarian-agent.md` (technical design) and `docs/wiki/librarian-agent.md` (user-facing overview).
- Updated `docs/fact-extraction-pipeline.md` and `docs/wiki/what-works-now.md` to describe the Librarian Agent.
- Updated `README.md` and `VISION/09-Roadmap/Phase-2-Knowledge-Graph.md` to reflect the new agent framework and note future goal-directed research.

## [0.44.0] — 2026-06-14

### Changed
 
 - **Scheduler async mutex**: `BackgroundScheduler` now uses `tokio::sync::Mutex` for pending/running job state and `submit()` is `async`, eliminating clippy `await_holding_lock` warnings and preventing accidental blocking of the async runtime.
 - **Centralized path resolution**: All config/data path construction now routes through `mimir_core::paths`. New helpers added: `skills_dir()`, `history_path()`, `personalities_dir()`. `ToolsConfig::default_path()` and `SkillsPermissionsConfig::default_path()` no longer duplicate `dirs::config_dir()` logic.
 - **Skill permission config placement**: `SkillsPermissionsConfig` moved from the `mimir` binary crate into `mimir_core::skills::permissions_config`, consolidating skill-related persistence in the core library.
 - **DRY HTTP client handling**: `mimir_client` response status handling is centralized in `MimirClient::check_response`, removing duplicated error blocks across every API method.
 - **DRY tool registration**: `mimir-server` startup now registers native tools through a single `register_tool` helper instead of repeating `if let Err(e) = ...` warning blocks.
 - **DRY daemon guard checks**: The `mimir` CLI dispatch loop uses a single `ensure_daemon` helper instead of repeating the same `ensure_daemon_running` error-handling block for every daemon-requiring subcommand.
 - **Shared category API types**: `CategoryResponse` and `CategoryDetailResponse` moved from `mimir-server` into `mimir_api_types` so the server and HTTP client share the same wire types.
 - **Category CLI uses MimirClient**: `mimir kb category` subcommands now use `MimirClient` instead of raw `reqwest` calls, and category methods (`kb_categories`, `kb_category_show`, `kb_category_create`, `kb_category_delete`) were added to the client.
 - **DRY `mimir kb` client construction**: All `mimir kb` handlers now share a local `make_client(base_url)` helper instead of repeating `MimirClient::new(base_url)` at every call site.
 - **DRY kb CLI error handling**: All `mimir kb` handlers use a shared `exit_with_error` helper instead of repeating the same `eprintln!`/`exit(1)` block.
 - **DRY skill/tool command helpers**: `mimir tool/skill` enable/disable/permission handlers use shared `set_*_permission_or_exit` and `persist_*_or_exit` helpers; skill name validation and origin parsing are also shared.
 - **DRY CLI error exits**: Remaining ad-hoc `eprintln!("Error: ...")`/`std::process::exit(1)` blocks in `mimir/src/commands.rs` were routed through the shared `exit_with_error` helper.
 - **Shared CLI error helper exported**: `commands::exit_with_error` is now `pub` so `mimir/src/main.rs` can use it for the `ask` no-query guard, removing another standalone error-exit block.
 - **DRY init/main warnings and errors**: `mimir/src/init.rs` and `mimir/src/main.rs` now use shared `exit_with_error` and a local `warn_on_err` helper, removing duplicated `eprintln!`/`std::process::exit(1)` blocks for daemon startup and systemd activation.
 - **DRY config init**: `Config::init` and `Config::init_at` now share a single `Config::write_default_config` helper instead of duplicating the atomic default-config writing logic.
 - **DRY identity seeding auto-merge**: The accidental-duplicate auto-merge loop in `seed_identity_facts` was extracted into `auto_merge_accidental_duplicates`, flattening nested matches and removing duplicated warning formatting.
 - **DRY best-effort warning helper**: Added a shared `warn_err` helper in `mimir-server` and applied it to tool registration, tools-config loading, alias wiring, and auto-merge, removing repeated `if let Err(e) = ... { tracing::warn!(...) }` blocks.
 - **Single-lock session cache**: `ContextManager::ensure_session_exists` now acquires the `sessions` cache lock once and holds it across the database existence check, removing a redundant second lock acquisition.
 - **DRY HTTP client URL builder**: `MimirClient` now builds endpoint URLs through a private `url()` helper, removing repeated `format!("{}/...", self.base_url)` strings.
 - **DRY environment overrides**: `Config::apply_env_overrides_with` now uses a local `set_from_env!` macro, collapsing dozens of repeated `if let Some(v) = getenv(...) { ... }` blocks into declarative one-liners.
 - **Shared server types**: `CategoryResponse` and `CategoryDetailResponse` are now re-exported through `mimir_server::types` alongside other shared API types.
 - **DRY init error handling**: `mimir init` uses a shared `exit_with_error` helper instead of a one-off `eprintln!`/`exit(1)` block.

### Added
 
 - Unit tests for `SkillsPermissionsConfig` load/save round-trip and invalid TOML handling.
 - Path helper tests for `skills_dir`, `history_path`, and `personalities_dir`.
 - Unit tests for the new `MimirClient` category methods.
 - Unit tests for the `warn_err` best-effort warning helper.
 - Unit test for the `MimirClient::url` helper.
 
 ## [0.43.4] — 2026-06-14

### Fixed

- Replaced all hardcoded `"finish_retrieval"` strings in `RetrievalAgent` with the `FinishRetrievalTool::NAME` constant, removing a maintenance risk if the tool name changes.
- `RetrievalAgent` now executes non-`finish_retrieval` retrieval tool calls concurrently via `futures::future::join_all`, while still assembling tool result messages in the original call order.

## [0.43.3] — 2026-06-14

### Fixed

- `RetrieveContextTool::name()` now returns `Self::NAME` instead of a hardcoded string, keeping the tool name and registry constant in sync.
- `RetrievalAgent::merge_entity_facts` now upgrades an "Unknown" root-entity placeholder when a typed entity with the same name is merged, and skips adding an "Unknown" placeholder when a typed entity already exists. This eliminates duplicate entities across `kg_related` root-entity accumulation and typed results from `kg_query`/`kg_search`.

## [0.43.2] — 2026-06-14

### Fixed

- Added language tags to unlabelled fenced code blocks in `docs/kg-tools.md`, `docs/retrieval-agent.md`, and `docs/wiki/retrieval-agent.md` to satisfy markdownlint MD040.
- Cleaned up the malformed release summary header in `docs/wiki/what-works-now.md` so the version and implemented features are accurate.
- `RetrievalAgent` entity/fact deduplication now preserves full identity: entities are matched by `name` *and* `entity_type`, and facts are compared using all structural and lifecycle fields.
- `RetrieveContextTool` no longer logs the raw retrieval task; it logs only the task length to avoid exposing potentially sensitive user context.
- `retrieve_context` now uses the request-resolved LLM (including per-request model overrides) instead of the startup LLM in both blocking and streaming chat handlers.

## [0.43.1] — 2026-06-12

### Fixed

- RetrievalAgent now emits a tool-result message for `finish_retrieval` even when the LLM erroneously calls it alongside other tools, preventing an unbalanced conversation that could be rejected by the backend.
- `accumulate_kg_query` now parses `valid_from` and `valid_until` from `KgQueryTool` JSON output instead of discarding them as `None`.

## [0.43.0] — 2026-06-12

### Added

- **Agentic context retrieval** (Issue #128). The main LLM can now call `retrieve_context` to launch a dedicated RetrievalAgent. The agent runs an ephemeral, internal LLM session with only retrieval tools (`kg_query`, `kg_related`, `kg_search`, `search_conversation_history`), investigating the knowledge graph and conversation history for up to 25 rounds before returning a structured `RetrievedContext`. This enables multi-step, parallel research for complex questions (e.g. "What should I make for dinner with Mary, Bob, and Tom?").
- New `RetrievedContext`, `RetrievedEntity`, `RetrievedFact`, `RetrievedRelation`, and `ConversationSnippet` types in `mimir-knowledge/src/retrieval/types.rs`.
- `FinishRetrievalTool` — internal termination signal used by the RetrievalAgent to signal completion.
- SSE `event: tool_call_start` in the streaming chat handler, emitted before each tool execution to give users real-time visibility into Mimir's research phase.

### Changed

- Bumped workspace version to 0.43.0.

## [0.42.2] — 2026-06-12

### Fixed

- `init_schema` now only rebuilds the FTS5 index when `messages_fts` is newly created, eliminating unnecessary startup latency and I/O for large conversation histories.
- `seed_identity_facts` inserts identity facts before the alias/auto-merge block, ensuring the canonical entity always has at least as many facts as any qualifying duplicate and preventing `auto_merge_pair` from deleting it.
- Removed dead `escape_fts5` duplicate from `mimir-knowledge/src/queries/entity.rs`; all callers already use `mimir_core::fts5::escape_fts5`.

## [0.42.1] — 2026-06-12

### Fixed

- `seed_identity_facts` now auto-merges bare-name duplicate entities when the preferred name matches an existing entity with ≤2 facts. This resolves the stale-duplicate scenario where a short-name entity was created before the alias was wired up to the canonical entity.
- Added `KnowledgeGraph::count_entity_facts()` helper in `mimir-knowledge` for counting facts referencing an entity as subject or object.

## [0.42.0] — 2026-06-12

### Added

- Added `messages_fts` FTS5 virtual table for full-text search over conversation history (`mimir-core/src/context.rs`).
- Added `ContextManager::search_messages()` for BM25-ranked search with snippet extraction.
- Added `search_conversation_history` built-in tool (`mimir-core/src/tools/builtins/search_conversation_history.rs`).
- Extracted `escape_fts5` to `mimir-core/src/fts5.rs` for shared use across crates.

### Changed

- **Breaking (internal):** Migrated `sessions.id` from `TEXT` (UUID) to `INTEGER PRIMARY KEY AUTOINCREMENT` for faster lookups and smaller storage. All session IDs are now `i64` across the workspace.
- Removed `uuid` dependency from `mimir-core`.
- Incognito session IDs now use negative atomic `i64` counters instead of UUIDs.
- Axum routes now auto-reject non-numeric session IDs with `400 Bad Request` via `Path<i64>`.

### Fixed

- Updated all tests, benchmarks, and integration tests to use integer session IDs.
- Updated API types (`mimir-api-types`), server routes (`mimir-server`), client library (`mimir-client`), CLI (`mimir`), and documentation to reflect integer session IDs.

## [0.41.3] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (additional finding addressed):
  - Added `MIMIR_SCHEDULER_DEBOUNCE_SECONDS` and `MIMIR_SCHEDULER_COOLDOWN_SECONDS` environment variable overrides in `mimir-core/src/config.rs`, following the existing `apply_env_overrides_with` pattern.

## [0.41.2] — 2026-06-11

### Fixed

- **Code review feedback for PR #144** (5 findings addressed):
  - Removed unused `jq_for_opt` clone in `mimir-server/src/state.rs` optimisation job closure.
  - Added `DaemonJob::from_job_id()` helper to eliminate duplicated string-to-variant mapping in `mimir-core/src/scheduler.rs`.
  - Log SQL errors in `relationship_type_id` instead of silently swallowing them with `.ok()?`.
  - Clarified memory condensation documentation: separated 2500-character budget from top-N limit (500).
  - Corrected nightly-optimization wiki to state "last minute" instead of "last few minutes" to match the 60-second cooldown default.

## [0.41.1] — 2026-06-11

### Fixed

- `LlmWorkerPool` `in_flight` counter is now incremented and decremented around every job processed by workers. Previously the counter was always zero, causing the scheduler's idle gate to incorrectly allow background jobs while LLM requests were in flight.
- `BackgroundScheduler::submit()` now correctly deduplicates against jobs that are already *running*, not just pending. Prevents back-to-back execution when a submit arrives during an active run.
- `BackgroundScheduler::shutdown()` is now called during `AppState::shutdown()`, wiring the scheduler's private shutdown channel into the daemon's graceful teardown sequence. Prevents stale "Running" DB rows when the runtime drops mid-job.

## [0.41.0] — 2026-06-11

### Added

- Unified `BackgroundScheduler` in `mimir-core` that deduplicates, debounces, and gates all background jobs on user downtime and LLM idle state.
- `DaemonJob` typed enum replaces stringly-typed job IDs for `JobQueue::run_now` and `status`.
- Demand-driven memory condensation: `KnowledgeGraph` emits a `tokio::sync::Notify` on dirty; a listener submits `DaemonJob::MemoryCondensation` to the scheduler.
- Configurable `memory.condensation_top_n` (default 500) replaces hard-coded top-20 hash in condensation pipeline.
- `[scheduler]` config section with `debounce_seconds` (default 5) and `cooldown_seconds` (default 60).
- `LlmWorkerPool` tracks in-flight job count via `in_flight_count()`, exposed through `LlmBackend`.

### Changed

- Replaced fixed 30-second interval loop for auto-condensation with event-driven scheduler.
- `POST /memory/refresh` now uses `force_submit` to bypass scheduler gates.
- Nightly optimization callback now submits condensation through the scheduler instead of direct `run_now`.
- `JobQueue::list_jobs()` added for scheduled-job polling.

### Fixed

- `relationship_type_id` no longer uses `?` on `Result` inside `Option`-returning function (Rust 2024 edition compatibility).

## [0.40.7] — 2026-06-10

### Fixed

- Fact extraction now falls back to parsing the assistant's text content as JSON
  when the LLM does not emit a structured tool call. This resolves intermittent
  extraction failures with backends such as Ollama + Gemma that do not support
  `tool_choice`.
- The daemon guard spawns the background server in its own Unix process group,
  preventing Ctrl-C in the terminal from killing the daemon.
- `generate_and_install_service_file` now ensures config and data directories
  exist before writing the systemd unit, preventing NAMESPACE failures when
  `ReadWritePaths` references missing directories.

## [0.40.6] — 2026-06-10

### Fixed

- Addressed remaining CodeRabbit review feedback for PR #125:
  - Fixed CHANGELOG entry for uninstall.sh redirect typo to use literal characters.
  - Aligned test-only init_at() with production init() by ensuring cache directory exists.
  - Replaced silent unwrap_or((0,)) with explicit match on the fact-count query during auto-merge to avoid treating DB errors as zero facts.
  - Documented the auto-merge threshold (fact_count <= 2) in process_extracted_fact.
  - Optimized category validation in insert_facts_batch to query only referenced category IDs instead of the full table.
  - Tidied SQL formatting in get_facts_by_subject_and_predicate.
  - Documented the alias score adjustment (1.1) in entity search queries.
  - Removed unreachable Windows path checks from the Linux-only resolve_executable_path function.
  - Added defensive mimir substring check in uninstall.sh remove_dir before rm -rf.
  - Removed unused serial_test::serial imports in mimir-core tests.

## [0.40.5] — 2026-06-10

### Fixed

- Fixed fact-loss bug where multiple atemporal facts with the same subject and predicate but different objects (e.g. multiple hobbies) would incorrectly supersede each other, leaving only the last-inserted fact. The temporal overlap logic in `insert_fact_in_tx` now respects a `MULTI_VALUED_PREDICATES` allow-list (`hobby`, `likes`, `has_pets`, `has_sibling`, etc.) so that independent values for these predicates coexist instead of overwriting one another.

## [0.40.4] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (additional findings addressed):
  - Fixed typo in `scripts/uninstall.sh` where `error()` redirected with `&&2` instead of `>&2`.
  - Fixed `insert_facts_batch` atomicity by calling `ensure_relationship_type_in_tx` inside the batch transaction instead of autocommitting via `ensure_relationship_type`.
  - Moved `preferred_name` alias registration and auto-merge side effects in `process_extracted_fact` to after the dedup/corroboration check, preventing irreversible mutations on duplicate facts.
  - Aligned `generate_service_file` implementation with its docs and test by removing the unused `cache_dir` parameter and updating callers.

## [0.40.3] — 2026-06-10

### Fixed

- **Code review feedback for PR #125** (8 findings addressed):
  - Strengthened `normalize_predicate` to handle `name` → `has_name`, `nickname` → `preferred_name`, `favorite_food`/`color`/`colour` variants, and trimmed leading/trailing whitespace.
  - Expanded `LIST_PREDICATES` to include `has_pets`, `has_child`, `has_parent`, `has_sibling`, and `has_partner`.
  - Removed extra whitespace from the `remember` tool description.
  - `remember` tool output now includes actual error messages instead of just counts.
  - Replaced flaky `tokio::time::sleep(200ms)` in chat integration test with a deterministic polling loop and timeout.
  - `spawn_fact_extraction` now skips empty/whitespace-only messages.
  - Renamed `user_message_clone` to `user_message` in `chat_stream_handler` to clarify ownership.
  - Optimized `seed_identity_facts`: replaced full 1,000-fact scan with targeted predicate-specific queries; both identity inserts are now performed atomically via `KnowledgeGraph::insert_facts_batch`.

### Changed

- Added `relationship_type_id`, `get_facts_by_subject_and_predicate`, and `insert_facts_batch` to `KnowledgeGraph` API.

## [0.40.2] — 2026-06-10

### Fixed

- **Chat fact extraction wired up**: The fact-extraction pipeline (`mimir-knowledge/src/extract.rs`) was fully implemented but never triggered from chat. Both `/chat` and `/chat/stream` endpoints now spawn a background task after persisting the assistant response to extract facts from the user message. This fixes the long-standing issue where Mimir could query the knowledge graph but never write to it from conversation.
- **DRY refactor**: Extracted the duplicated extraction-spawning logic into `spawn_fact_extraction` in `mimir-server/src/routes/chat.rs`.

- **`remember` tool**: Registered `RememberTool` in the tool registry so the LLM can proactively write facts during conversation. The tool accepts structured `RememberOutput` and processes each fact through the same validation, dedup, confidence-assignment, and insertion pipeline used by background extraction.
- **System prompt updated**: The injected memory note now tells the LLM to use the `remember` tool whenever the user shares something worth saving.
- **Extraction prompt enriched**: Added detailed predicate standards (e.g., `studied_at` not `attended`, `hobby` not `hobbies`), explicit list-splitting instructions, and deduplication guidance to the fact extraction system prompt.
- **Predicate normalisation**: Rust-side `normalize_predicate` maps common LLM synonyms to canonical names (e.g., `attended` → `studied_at`, `hobbies` → `hobby`).
- **Comma-separated list splitting**: `split_list_objects` expands single facts with comma-separated values into multiple independent facts for allow-listed predicates (e.g., `hobby: "A, B, C"` → three separate `hobby` facts).

### Changed

- **Documentation**: Updated `docs/fact-extraction-pipeline.md`, `docs/chat-server.md`, and `docs/wiki/fact-extraction.md` to reflect that extraction is now live in the daemon.

## [0.40.1] — 2026-06-10

### Fixed

- **Personality prompts**: Removed references to a non-existent `memory` tool from `transparent`, `concise`, and `warm` presets. The LLM was instructed to use a tool that was not registered, causing `ToolError::NotFound("memory")` during conversations.
- **Identity seeding**: When the server starts, it now inserts `has_name` and `preferred_name` facts into the knowledge graph for the user entity (if not already present). This ensures Mimir can learn the user's identity through the existing memory condensation pipeline instead of relying on prompt injection.

## [0.40.0] — 2026-06-10

### Added

- **Issue #63**: Comprehensive testing suite for `mimir-knowledge`.
  - Inline unit tests for confidence model, clock, entity/fact models, and forget logic.
  - Temporal point-in-time DB integration test (`tests/temporal_point_in_time.rs`).
  - Criterion benchmark suite (`mimir-knowledge/benches/kg_benchmarks.rs`) with 10k-fact dataset covering entity resolution, FTS5, graph traversal, inference chain, and memory condensation.
  - `Clock::today()` and `MockClock::advance(Duration)` for deterministic temporal testing.

### Changed

- `MockClock::advance_seconds(i64)` replaced with `advance(&self, duration: Duration)`.


## [0.39.0] — 2026-06-10

### Added

- **Issue #61**: Full `mimir kb` CLI command suite (Phase A).
  - New commands: `kb query`, `kb show`, `kb edit`, `kb browse`, `kb profile`.
  - Existing commands (`kb audit`, `kb forget`, `kb restore`, `kb trash`) rewritten to go through the daemon via HTTP instead of opening SQLite directly.
  - All commands support `--json` for scripting output.
  - Human-readable output uses `tabled` for tables and `colored` for confidence color-coding (green >0.9, yellow 0.7–0.9, red <0.7).
  - Server routes added under `/kb/`: `query`, `facts/:id`, `facts/forget`, `browse`, `profile`, `audit`, `trash`, `trash/restore`.
  - Shared API types added to `mimir-api-types`: `FactQueryParams`, `FactDetailResponse`, `FactEditRequest`, `BrowseRequest`, `ProfileRequest`, `AuditQueryRequest`, `ForgetRequest`, `RestoreRequest`, `TrashListResponse`, and supporting row types.
  - New `update_fact` method in `mimir-knowledge` for structured field editing with transactional audit logging.
  - Server integration tests for all new routes.

### Changed

- CORS configuration now allows `PATCH` and `DELETE` methods.

## [0.38.0] — 2026-06-09

### Changed

- **Issue #112**: Switched chat context injection wording from `## Persistent Memory Context` to `Key facts I know about you:`.
  - Signals to the LLM that the injected memory is a curated subset, not an exhaustive record.
  - LLM should continue to use KG tools (`kg_query`, `kg_search`) for deeper or exhaustive queries.
- Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs` to use the new wording.
- Updated unit and integration tests in `mimir-core` to assert the new prompt text.

### Added

- Added server integration tests in `mimir-server/src/lib.rs`:
  - `test_chat_injects_kg_memory_into_system_prompt`: verifies blocking `/chat` injects KG condensed memory into the system prompt.
  - `test_chat_stream_injects_kg_memory_into_system_prompt`: verifies SSE `/chat/stream` injects KG condensed memory into the system prompt.


## [0.37.0] — 2026-06-08

### Removed

- **Issue #111**: Deleted the legacy `memory.md` file-backed memory system entirely.
  - Removed `mimir-core/src/memory/` directory (`MemoryManager`, `MemoryLoader`, `MemorySnapshot`).
  - Removed `MemoryTool` from `mimir-core/src/tools/builtins/`.
  - Removed `memory_manager` benchmark from `mimir-core`.
  - Cleaned stale `# path = "${CONFIG_DIR}/memory.md"` example comments from config TOML strings.

### Changed

- Memory is now exclusively knowledge-graph-backed via `mimir-knowledge`.
- `mimir-core` no longer exports a `memory` module; all memory access flows through `mimir-knowledge::KnowledgeGraph`.


## [0.36.0] — 2026-06-08

### Removed

- **Issue #110**: Removed all remaining file-based memory.md scaffolding.
  - memory.path and MIMIR_MEMORY_PATH env override removed from MemoryConfig.
  - MemoryTool unregistered from daemon and CLI tool list.
  - MemoryLoader::init() no longer called during mimir init.
  - AppState no longer carries memory_path or syncs memory.md on shutdown.
  - StatusResponse no longer includes memory_path.
  - mimir-core/src/paths.rs no longer exports memory_path().

### Changed

- mimir memory CLI and /memory server route now exclusively serve knowledge-graph-backed condensed memory.
- mimir status and chat REPL /status display no longer show the deprecated memory.md path.

### Added

- CLI parsing test for mimir memory --refresh flag.

### Documentation

- Updated docs/memory-system.md, docs/cli.md, docs/chat-server.md, docs/shutdown.md, docs/wiki/memory.md, docs/wiki/what-works-now.md, docs/wiki/cli-commands.md, docs/wiki/configuration.md, and docs/wiki/tools.md to remove memory.md references and describe the KG-backed system.

## [0.35.3] - 2026-06-08

### Fixed
- Fixed `sqlx::migrate!` not recognising `-- no-transaction` in migrations 031, 032, and 033 because the directive was preceded by comment headers. This caused those migrations to run inside transactions, which in turn caused `PRAGMA foreign_keys = OFF` to be ignored. Migration 033's `DROP TABLE relationship_types` then triggered an `ON DELETE CASCADE` that silently emptied `relationship_constraints`, breaking `test_predicate_validation`.


## [0.35.2] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback (CodeRabbit AI):
  - Removed duplicate 0.35.1 section from CHANGELOG.
  - Fixed oversize LLM output handling in memory condensation to use deterministic fallback instead of truncation, preventing underflow at `char_limit == 0`.
  - Recurring event output now uses the computed next occurrence date instead of the stored historical date.
  - Search failures during user entity resolution are now handled separately from "not found", preventing duplicate entity creation on transient errors.
  - Memory condensation job failures are now propagated to the job queue result instead of being silently swallowed.
  - Auto-trigger condensation loop is now skipped when no user entity is configured, preventing perpetual 30-second re-triggers.
  - `mimir init` now falls back to system identity when blank/whitespace input is provided.
  - `mimir memory --refresh` now surfaces server-side errors in the CLI output and exits with a non-zero status on failure.
  - Added client tests for `memory_refresh()` success and error paths.
  - Added server route tests for `/memory/refresh` non-loopback rejection, not-registered, and already-running cases.

## [0.35.1] — 2026-06-08

### Fixed
- Addressed PR #114 review feedback:
  - Status endpoint now reads live condensed memory and upcoming section from the knowledge graph instead of the deprecated `memory.md` file.
  - `condensation_dirty` flag now automatically triggers the memory condensation job via a background watcher in the daemon.
  - Removed unused `whoami` dependency from `mimir-core`.
  - Removed dead `condensation_queued` field from `AppState`.
  - Centralised `recurrence_type_id` to `RecurrenceType` mapping via `TryFrom<i16>` in the enums module.
  - Chat system prompt builder now logs warnings when knowledge graph memory queries fail.
  - DRYed the SQL query in `build_memory_schema_with_opts` by constructing it once with a conditional predicate.
  - Fixed budget truncation loop so facts in `exclude_from_budget` buckets are still collected after the character budget is exhausted.

## [0.35.0] — 2026-06-07

### Added
- **Live Memory System (Issue #109)** — Replaced static `memory.md` with an event-driven, knowledge-graph-backed memory block.
  - Stable facts are condensed by the LLM and cached in `system_state.condensed_memory`.
  - Upcoming events (entity dates + temporal facts) are rendered fresh on every request.
  - Regeneration triggers: fact mutations, explicit `mimir memory --refresh`, and nightly optimization completion.
  - Pure formatting LLM prompt with deterministic fallback on failure or oversized output.
  - Sensitive facts are excluded from the LLM condensation pipeline.
- **Identity configuration** — `mimir init` now prompts for full name and preferred name, stored in `[identity]` config section.
- **User entity auto-resolution** — Daemon resolves the user entity from config at startup, creating it in the KG if missing.

### Changed
- `/memory` HTTP route now returns the live condensed memory block instead of `memory.md`.
- Chat system prompt now injects the live memory block from the knowledge graph.
- `build_memory_schema` supports `exclude_buckets` and `exclude_sensitive` options.
- `OptimizationRunner` now supports an `on_complete` callback for post-optimization hooks.

### Deprecated
- `memory.md` file-based memory is deprecated. `MemoryTool` writes are now logged as warnings.

## [0.33.2] - 2026-06-05

## [0.34.2] - 2026-06-07

### Fixed

- **Addressed PR #113 review feedback** (CodeRabbit AI review round 2):
  - Added serde default for `memory_priority_id` in `Fact` model to preserve legacy trash payload deserialization.
  - Replaced magic priority ID fallback (`3`) with semantic SQL lookup against `memory_priorities` table.
  - Fixed fire-and-forget centrality cache updates by making `bump_centrality` and `drop_centrality` async.
  - Eliminated TOCTOU race in `build_memory_schema` cache population with a read-then-populate pattern.
  - Replaced hardcoded category ID lists in `determine_bucket` with named constants.
  - Fixed potential UTF-8 panic in `truncate_fact` with char-aware truncation.
  - Reformatted SQL strings across `trash.rs` and `inference_tests.rs` for readability.
  - Updated documentation version references and corrected incomplete sentences.

## [0.34.1] - 2026-06-06

### Fixed

- **Review fixes for PR #108**: addressed 3 critical review findings in fact ranking engine.
  - Wired up `memory_priority_id` from `relationship_types.default_memory_priority_id` during fact insertion (`queries/fact.rs`, `extract.rs`, `models/fact.rs`).
  - Moved `drop_centrality` cache decrements to occur **after** `forget_fact` database transaction succeeds (`lib.rs`), preventing permanent cache drift on DB errors.
  - Fixed `truncate_fact` budget edge case (`queries/memory.rs`) so that when remaining budget is smaller than `subject + relationship + 3` overhead, `object_display` is correctly truncated to `…` instead of silently exceeding the budget.


### Fixed

- **Review fixes for PR #107**: addressed 10 CodeRabbit review findings across knowledge graph, server, and CLI.
  - `extract.rs` prompt now includes sub-categories with indentation so the LLM can pick specific IDs.
  - `lib.rs` fact insertion now validates category IDs before `INSERT OR IGNORE`, failing loudly on non-existent categories.
  - `queries/category.rs` replaced magic `NOT IN (5, 6)` with bound `FactStatus::Superseded` / `Forgotten` parameters.
  - `kg_expand_catalogue.rs` now queries real `fact_count` for each child category instead of hard-coding `0`.
  - `integration_tests.rs` merge assertion tightened with `object_id` filter to avoid false positives.
  - `error.rs` no longer leaks raw internal KG error strings in `500` HTTP responses.
  - `lib.rs` (server) tool-registry tests now assert `expand_catalogue` and `get_facts_in_catalogue` are exported.
  - `chat.rs` only fetches the catalogue DB when a new session or incognito turn starts, avoiding hot-path latency.
  - `cli.rs` `category add` now exposes `--memory-weight` to match the server API.
  - `kb.rs` JSON decode failures are no longer swallowed with `unwrap_or_default()`; they now surface as fatal CLI errors.

## [0.33.1] - 2026-06-05

### Fixed

- **P2**: `get_facts_matching_all_categories` now deduplicates input category IDs before querying, preventing empty results when duplicate IDs are passed.
- **P3**: Removed unused `client` variable in `mimir/src/kb.rs` (`handle_kb_category`).
- **P3**: Simplified redundant closures in `mimir-server/src/routes/kb_categories.rs` (5 instances of `.map_err(|e| error::knowledge_error(e))?` → `.map_err(error::knowledge_error)?`).

## [0.32.2] - 2026-06-05

### Fixed

- **Review fixes for PR #92**: addressed 14 CodeRabbit review findings across job queue, optimization pipeline, documentation, and daemon routes.

## [0.32.1] - 2026-06-04

### Fixed

- **P1**: `optimization_pass_runs` now linked to parent `optimization_runs` via foreign key `run_id`. `OptimizationRunner` inserts a parent row at pipeline start and updates it on completion or failure. Failed passes are recorded with error text instead of being silently omitted.
- **P1**: `DailySchedule::next_after` now converts the stored naive local time to UTC using `chrono::Local`, fixing scheduling for non-UTC timezones.
- **P1**: `chat_stream_handler` now calls `state.record_user_activity()`, ensuring SSE stream interactions update `last_user_activity` and prevent premature job yielding.
- **P2**: `JobQueue::run_now` now rejects concurrent executions of the same job by checking for an existing `Running` row in `job_runs`.
- **P2**: `semantic_dedup` candidate query now includes `ORDER BY a.id, b.id` for deterministic candidate selection.
- **P2**: `semantic_dedup` now uses a structured LLM tool schema (`evaluate_dedup_candidates`) instead of relying on raw JSON parsing from a plain-text prompt.


## [0.32.0] - 2026-06-04

### Added

- JobQueue and nightly optimization pipeline (issue #58):
  - New `mimir-core::job_queue` with durable job definitions, runs, scheduling, and manual triggers.
  - `JobQueue` persisted in `jobs.db` with `Job`, `JobPriority`, `JobStatus`, `JobRunStatus`, `DailySchedule`, `JobContext`, and `JobRunSummary` public types.
  - Config support for `[knowledge.optimization]` defaults: `cpu_cores = 1`, `nice_level = 10`, `timeout_minutes = 120`, `schedule_time = "02:00"`.
  - Daemon tracks user activity in `AppState`; chat routes record interaction time.
  - System jobs yield between pass boundaries when user activity is inside the 5-minute idle window.
  - Daemon routes: `GET /kb/optimization/status` and `POST /kb/optimization/run-now` (loopback-only for run-now).
  - CLI commands: `mimir kb optimization --status` and `mimir kb optimization --run-now`.
  - Refactored `mimir-knowledge/src/optimization` into pass modules with 10 nightly passes (7 core optimization passes plus 3 cleanup steps):
    - Pass 1: deterministic dedup (exact triple merge).
    - Pass 1b: semantic dedup via LLM structured JSON; auto-merge >= 0.9 confidence, queue uncertain pairs.
    - Pass 2: contradiction resolution.
    - Pass 3: inference chain re-evaluation.
    - Pass 4: confidence recalculation.
    - Pass 5: dormant cleanup (old disputed non-user facts).
    - Pass 6: pattern consolidation stub.
    - Pass 7: compaction (FTS rebuild, ANALYZE, VACUUM).
    - Plus: pending confirmation cleanup (7-day TTL) and trash cleanup.
  - Pre-pass backup with `VACUUM INTO` to `~/.local/share/mimir/backups/knowledge-YYYY-MM-DD.db` with counter suffix for collisions.
  - Per-pass run recording in `optimization_pass_runs` table.
  - Integration tests for daemon routes and CLI client methods.

### Changed

- `run_nightly_optimization` compatibility wrapper now delegates to `OptimizationRunner::run_all`.
- `cascade_inner` in `confidence.rs` future is now `Send`-safe.

## [0.31.1] - 2026-06-04

### Fixed

- **P1**: `restore_all` now maps both child and parent IDs through `id_map` when rebuilding `fact_dependencies`, preventing FK violations on restored facts.
- **P1**: `restore_fact` now marks the trash row as restored, preventing duplicate restores and stale trash listings.
- **P1**: `hard_delete_all_facts` correctly reports the number of forgotten facts via `rows_affected()` instead of querying the now-empty table.
- **P1**: `create_backup` escapes single quotes in the backup path before interpolating into `VACUUM INTO`, preventing SQL injection/breakage from `XDG_DATA_HOME` paths containing apostrophes.
- **P2**: Restoration audit log now references the newly generated fact ID instead of the original deleted ID.

## [0.31.0] - 2026-06-04

### Added

- Phase 2: Forgetting system -- trash, cascade forget, restore, bulk operations (#57)
  - Bulk forget by predicate, entity, source, time range, and full reset.
  - Trash bin with 30-day expiry, restoration, and automatic nightly cleanup.
  - Cascade forget for inferred facts: orphan removal and confidence recalculation.
  - Bulk safeguards: >100 facts requires --yes, sensitive predicates require --confirm-sensitive, full reset requires typing DELETE EVERYTHING.
  - Full reset creates a timestamped SQLite backup via VACUUM INTO.
  - New CLI commands: mimir kb forget, mimir kb restore, mimir kb trash.
  - Extended TrashPayload with dependency chains so restored facts rebuild parent links.
  - Sensitive predicate flag (sensitive BOOLEAN) on predicates table with seeded defaults for medical/financial terms.


## [0.30.1] - 2026-06-04

### Fixed

- **P1**: `kg_query` and `kg_related` no longer mutate the database via `ensure_predicate` during read-only tool calls. Both now use the new read-only `get_predicate_id` lookup; missing predicates return empty results instead of silently inserting rows.
- **P2**: `AppState` knowledge graph and context database fallbacks now propagate `PathsError` instead of using a broken tilde (`~`) literal path.
- **P3**: `kg_search` now returns an explicit invalid-arguments error when an unrecognized `entity_type` is supplied, rather than silently ignoring the filter.

## [0.30.0] - 2026-06-04

### Added

- Phase 2: Knowledge Graph LLM tools — `kg_query`, `kg_related`, `kg_search` (#56)
  - Database migration `028_add_performance_indexes.sql` for tool query performance.
  - Query layer: `search_entities`, `traverse_graph`, `get_facts_by_subject_filtered`, `get_entity_names`.
  - Tool implementations in `mimir-knowledge/src/tools/` implementing `mimir_core::Tool`.
  - Server integration: `AppState` initialises `KnowledgeGraph` and registers all three tools.
  - Input sanitisation, FTS5 injection defence, and SQL-level exclusion of pending/superseded/forgotten facts.
  - Comprehensive unit and integration tests.


## [0.29.2] - 2026-06-03

### Fixed

- `mimir-knowledge/src/optimization/mod.rs`: `cleanup_stale_pending_confirmations` now deletes `fact_dependencies` rows before deleting the fact and wraps each deletion in a transaction, avoiding `ON DELETE RESTRICT` violations and ensuring atomic DB/cache state.

## [0.29.1] - 2026-06-03

### Fixed

- `mimir-knowledge/src/extract.rs`:
  - `confirm_fact` now cascades inferred facts instead of discarding them (P1).
  - `find_existing_fact` dedup query now matches pending-confirmation facts, preventing duplicate sensitive extractions (P1).
  - `handle_correction` retrospective loop is now atomic: all overlapping facts are marked `Corrected` and soft-deleted in a single transaction before child evaluation (P2).
- `mimir-knowledge/tests/extraction_test.rs`: corrected misleading comment in `test_casual_extraction` (P3).

## [0.29.0] - 2026-06-03

### Added

- Fact extraction pipeline (issue #55):
  - `mimir-knowledge/src/extract.rs`: full LLM → Rust validation → entity resolution → confidence assignment → sensitive confirmation → fact insertion pipeline.
  - LLM tool `remember`: structured schema for extracting subject-predicate-object triples with classification (Explicit / Casual / Correction), temporal bounds, and sensitivity flags.
  - Entity resolution: names matched via exact → alias → FTS5 fuzzy; new entities auto-created with LLM-provided type.
  - Confidence assignment: classification maps to `SourceType` → `confidence::initial()`; LLM hints are ignored.
  - Correction handling:
    - Temporal: `correction_scope` as ISO-8601 datetime closes the sole open-ended predecessor.
    - Retrospective: `correction_scope = "always"` marks overlapping facts as `Corrected`, moves them to trash, and inserts the new fact.
  - Sensitive fact confirmation flow:
    - Sensitive facts inserted as `Disputed` with `pending_confirmation = TRUE`.
    - In-memory `HashSet<i32>` cache rebuilt from DB on startup.
    - `confirm_fact`: flips to `Active`, confidence `1.0`, triggers inference.
    - `reject_fact`: hard-deletes with `Rejected` audit entry.
  - Corroboration stub for issue #79: duplicate facts returned in `ExtractionOutcome::corroborated` without insertion.
  - 11 integration tests covering explicit, casual, entity resolution, temporal/retrospective correction, sensitive confirmation/rejection, multiple facts, empty extraction, and invalid LLM output.

### Changed

- `facts` table: added `pending_confirmation BOOLEAN NOT NULL DEFAULT FALSE` (migration 026).
- `change_types` table: added `rejected` (migration 027).
- `Fact` model: added `pending_confirmation` field.
- `ChangeType` enum: added `Rejected = 8`.
- `ranges_overlap` in `queries/fact.rs`: made `pub` for reuse in extraction pipeline.


## [0.28.1] - 2026-06-02

### Fixed

- Review feedback on inference engine (issue #54):
  - `CHANGELOG.md`: reordered 0.28.0 section to top with markdownlint blank lines.
  - `docs/inference-engine.md`: explicit facts are detected by `!inferred` rather than `confidence == 1.0`.
  - `mimir-knowledge/src/inference/mod.rs`: streaming evaluation for `evaluate_batch` (pending — rule loop still materialises; moved to follow-up).
  - `contradiction.rs`: explicitness uses `!inferred`; status updates wrapped in atomic transactions via `set_status_tx`.
  - `threshold.rs`: DB errors propagated instead of `unwrap_or(0)`; stale preferences deleted when source fact missing; duplicate `StatusChange` audit entries deduplicated within 24h.
  - `transitivity.rs`: trigger queries include `FactStatus::Inferred`; inferred facts use temporal intersection of parent windows.
  - `lib.rs`: `ensure_predicate` insert is atomic with `ON CONFLICT`.
  - `NewFact`: removed `Default` impl; added `NewFact::new(subject_id, predicate)` constructor.
  - `optimization/mod.rs`: confidence cascade uses unlimited depth (`None`); operational errors propagated instead of swallowed.
  - Tests: predicate name roundtrip restored; unknown predicate test uses absent ID; contradiction relation type asserted; cycle-safety contract replaces brittle exact count.

## [0.28.0] - 2026-06-02

### Added

- Inference engine core with `InferenceRule` trait, `RuleEngine`, and `CascadeContext` for cycle-safe unbounded cascades.
- Transitivity rule: `visited`/`is_in` + `is_in` chain → inferred transitive facts with depth-tracked confidence.
- Contradiction rule: real-time `Disputed` status + bidirectional `Contradicts` edges; nightly batch auto-resolves explicit > inferred disputes.
- Threshold rule: 3+ `rejected_action` facts → `General` preference upsert; nightly re-count warns if threshold drops.
- `PredicateRegistry` with `ensure_predicate` and `predicate_name` for unlimited extensible predicates backed by the DB.
- Migrations 024 (Contradicts relation type) and 025 (rejected_action predicate).
- Nightly optimization orchestrator (`run_nightly_optimization`) wiring contradiction resolution, confidence propagation, and inference re-evaluation.
- Integration tests for transitivity, contradiction, threshold, cascade, and cycle safety.

### Changed

- Removed compile-time `Predicate` enum; `NewFact.predicate` is now a `String` resolved at runtime.
- `Fact::predicate()` removed; callers use `kg.predicate_name(fact.predicate_id)`.
- `KnowledgeGraph::insert_fact` automatically runs inference rules and cascades inferred facts.
- `NewFact` extended with `inferred`, `inference_depth`, `confidence`, and `parent_fact_ids` fields.

### Documentation

- Added `docs/inference-engine.md` with architecture, rule descriptions, confidence formulas, and cascade behavior.
- Added `docs/wiki/inference-rules.md` with user-facing examples and best practices.

## 0.27.1 (2026-06-02)

> Next-day hotfix release for 0.27.0.

### Fixed

- Atomic upsert: delete and insert now happen in a single transaction, preventing data loss on crash between commit and insert.
- Contextual lookup now correctly falls back to the default (zero-context) preference when no contexts match, instead of ranking by confidence.
- `preference_sources` now binds `extracted_at` explicitly for deterministic timestamps.
- `preference_audit_log` stores `NULL` for the `reason` column on creation events instead of an empty string.
- `get_preference` eliminates N+1 queries by fetching all contexts in a single query.
- Uniqueness checks in `insert_preference` and `upsert_preference` no longer clone the full context `HashSet`.
- Confidence validation now happens before acquiring a database write lock.
- Migration 023 now seeds `predicate_constraints` for `HasPreference` so `validate_predicate` does not fail.

## 0.27.2 (2026-06-02)

### Fixed

- Review feedback on preference system (issue #53):
  - `source_fact_id` is now nullable in `preferences` table and Rust types (`Option<i32>`).
  - Explicit preferences (`overridden_by_user = true`) now require `confidence = 1.0` at validation time.
  - `UpsertAction::Overwritten` now updates the existing preference row in-place instead of deleting and re-inserting, preserving the audit trail.
  - Clarified that the 11 seeded predicates in `predicate_constraints` are the complete set.

## 0.27.0 (2026-06-01)

### Added

- Preference system refactor (issue #53): behavioural index over the fact graph with contextual lookup and conflict resolution.
- New `Predicate::HasPreference = 11` seeded in `predicates` table.
- New lookup tables (re-seeded in migration 023):
  - `preference_categories`: 7 variants — CalendarBehavior, NotificationStyle, FoodPreference, TravelPreference, WorkStyle, CommunicationPreference, General.
  - `preference_source_types`: 3 variants — Interaction, Fact, UserEdit.
- New `PreferenceCategory` and `PreferenceSourceType` enums with `#[repr(i16)]` and `sqlx::Type`.
- New schema (migration 023):
  - `preferences` with `source_fact_id NOT NULL REFERENCES facts(id)`.
  - `preference_contexts` — normalized context conditions, no JSON.
  - `preference_sources` — provenance with `(preference_id, source_type_id, source_id)` unique constraint.
  - `preference_audit_log` — immutable history without FK to `preferences` (preserves history after deletion).
- Contextual lookup API: `get_preference(entity_id, key, query_context)` ranks by match count, confidence, and recency.
- Upsert API with conflict resolution:
  - Explicit overrides inferred.
  - Higher-confidence inferred wins.
  - Same confidence keeps existing.
  - `overridden_by_user = true` blocks inferred overwrites.
- Full audit logging on preference creation and overwrite.
- Source tracking for every preference.
- FK enforcement: non-existent `source_fact_id` is rejected.
- Comprehensive test suite in `mimir-knowledge/tests/preference_tests.rs` (15 tests).
- Technical documentation: `docs/preference-system.md`.
- User-facing documentation: `docs/wiki/preferences.md`.

### Changed

- **Breaking schema change:** old `preferences` and `preference_sources` tables dropped and recreated. No data migration attempted.

## 0.26.0 (2026-06-01)

### Added

- New built-in tool `get_weather` using wttr.in.
  - Fetches current conditions for any location (city name, airport code, or coordinates).
  - Returns structured JSON: temperature (°C/°F), feels-like, description, humidity, wind, UV index, visibility, and pressure.
  - Configurable base URL for testing (`GetWeatherTool::with_base_url`).

## 0.25.1 (2026-06-01)

### Fixed

- `get_active_facts_at` restored missing `AND fact_status_id = ?` filter so it again returns only active facts.
- `query_audit_log` switched from INNER JOINs to LEFT JOINs on `facts`, `entities`, and `predicates`, ensuring audit history remains visible after a fact is forgotten (hard-deleted).
- `mimir kb audit` now validates `--from` and `--to` datetime strings and exits with an error instead of silently ignoring malformed input.

## 0.25.0 (2026-06-01)

### Added

- Provenance audit refactor (issue #52): typed `change_type` and `changed_by` lookup tables with integer IDs.
- New lookup tables: `extraction_methods` (5 variants), `change_types` (7 variants), `changed_by_types` (4 variants).
- New `ExtractionMethod`, `ChangeType`, and `ChangedBy` enums with `#[repr(i16)]` and `sqlx::Type`.
- `mimir kb audit` CLI command for querying the fact audit log directly from the local SQLite database.
- `query_audit_log` API with filters: entity name, predicate name, datetime range, and change type.
- `add_source_to_fact` API for adding corroborating sources to an existing fact.
- `sources` unique constraint: `(fact_id, source_type_id, connector_id, raw_reference)`.
- Audit entries are now column-only JSON snapshots (e.g. `{"valid_until": ...}`) instead of full fact snapshots.

### Changed

- **Breaking schema change:** `source_types` remapped to 6 canonical variants: `UserEdit(1)`, `Connector(2)`, `Inference(3)`, `Interaction(4)`, `Import(5)`, `System(6)`. Old `Email`/`Calendar`/`Photo`/`Message` variants mapped to `Connector`; `CasualMention` mapped to `Interaction`.
- `fact_audit_log` recreated with `change_type_id`, `changed_by_id`, `reason`, and `changed_at` columns. Old action/performer strings migrated via best-effort mapping.
- `sources` recreated with `extraction_method_id INTEGER REFERENCES extraction_methods(id)`.
- `NewFact` expanded with `connector_id`, `connector_type`, `raw_reference`, and `extraction_method` fields.
- `update_fact_valid_until`, `update_fact_status`, and `forget_fact` now accept `ChangedBy` parameter.
- `forget.rs` deletes **all** `fact_dependencies` rows where the forgotten fact is parent or child (not just `InferredFrom`).
- Confidence cascade now writes `confidence_change` audit entries on child recalculation.

### Fixed

- Prevent duplicate edges when an already-superseded fact is superseded again by a third explicit fact.
- Correct `children` and `remaining_parents` queries in `forget.rs` after removal of relation_type filter from the DELETE query.

## 0.24.3 (2026-05-31)

### Added

- Structural confidence model (issue #51): confidence derived entirely from graph structure, zero LLM involvement, zero time-based decay.
- New `SourceType` variants: `CasualMention`, `Import`, `System`.
- New `ConnectorType` enum with SQLite lookup table and reliability tracking.
- `inference_confidence` formula: signed parent sum × chain penalty (0.8^depth) × breadth factor.
- `inference_depth` and `stale_confidence` columns on `facts` table.
- `is_positive` column on `fact_dependencies` for signed parent contributions.
- Per-connector reliability scores with feedback loop (`adjust_connector_reliability`).
- Eager bounded confidence cascade on parent removal.

### Changed

- `NewFact` no longer accepts caller-provided `confidence`; confidence is now computed in Rust (internal change; not public API).
- Connector-type source facts now use per-connector reliability scores instead of flat 0.80.
- Initial confidence values: `UserEdit`/`System` = 1.0, `CasualMention` = 0.30, `Import` = 0.80.

### Fixed

- Updated all test assertions and raw SQL to match new schema columns.

## 0.24.4 (2026-05-31)

### Fixed

- Build failure in `mimir-client`: replaced unsupported `reqwest` feature `rustls-tls-ring` with `rustls-native-certs` to align with `reqwest` 0.13 feature flags and `mimir-core` crate configuration.

### Documentation

- Added `docs/wiki/what-works-now.md`: comprehensive user-facing overview of all working features, current limitations, known bugs, and roadmap context.

## [0.33.0] - 2026-06-05

### Added

- **Category taxonomy system** (Dewey Decimal-style):
  - New `categories` table with hierarchical parent-child relationships.
  - `fact_categories` junction table allowing facts to belong to multiple categories.
  - Comprehensive seed taxonomy covering Identity (100), Food & Drink (200), Health (300), Relationships (400), Work (500), Home (600), Entertainment (700), Travel (800), and Schedule (900) with 2-3 levels of depth.
  - New KG tools: `expand_catalogue` and `get_facts_in_catalogue` for LLM-driven category browsing and fact retrieval.
  - System prompt injection of top-level catalogue so the LLM knows what knowledge domains exist.
  - CLI commands: `mimir kb category list`, `show`, `add`, `delete`.
  - Server routes: `GET /kb/categories`, `GET /kb/categories/{id}`, `POST /kb/categories`, `DELETE /kb/categories/{id}`.

- **Extraction pipeline category assignment**:
  - LLM suggests 1–3 category IDs per extracted fact via the `remember` tool.
  - Rust validates all suggested IDs against the database before insertion.

### Changed

- **Renamed `predicates` → `relationship_types`** and `predicate_constraints` → `relationship_constraints` across the entire codebase (DB schema, models, queries, tools, inference rules, tests).
- Updated all SQL queries, indexes, and foreign keys to use `relationship_type_id`.
- Updated `MemoryManager` and system prompt integration to read from the knowledge graph catalogue.

### Migration

- Migration `031_category_taxonomy_and_rename_predicates.sql` performs the rename and seeds the full category taxonomy.

## [0.34.0] - 2026-06-06

### Added

- **Issue #108**: Fact Ranking & Selection Engine (`mimir-knowledge`).
  - Introduced `memory_priorities` lookup table (Critical, High, Normal, Low) and `memory_priority_id` on `facts`.
  - Added `default_memory_priority_id` to `relationship_types` for automatic priority assignment at insertion.
  - Implemented scoring formula: `confidence × category.memory_weight × temporal_boost × priority_boost × centrality_boost`.
  - Temporal boost: `10.0 / sqrt(max(days, 0.5))` for future-dated facts (upcoming events, birthdays).
  - Centrality boost: entity connection count with in-memory `HashMap` cache, incrementally updated on mutation.
  - Budget fill algorithm: identity facts first (~200-char soft reservation), then greedy score-based fill to 2500-char limit.
  - Structured buckets: `identity`, `relationships`, `preferences`, `upcoming`, `general`.
  - Deterministic fallback renderer in Rust for when LLM condensation is unavailable.
  - `system_state` read/write queries for cached `condensed_memory`.
  - Unit and integration tests covering scoring, temporal boost, budget fill, renderer, and centrality cache.

## [0.38.1] — 2026-06-09

### Added

- **Issue #60**: Added explicit non-exhaustive note to context-injected system prompt.
  - When condensed memory is present, the system prompt now appends: "Note: This is not an exhaustive list. Use kg_query, kg_related, or kg_search tools if you need more information."
  - Signals the LLM that the injected memory is a curated subset, prompting tool use for deeper queries.
  - Completes the Layer 2 context injection design from Phase 2 Knowledge Graph architecture.
  - Updated `Personality::system_prompt()` in `mimir-core/src/personality.rs`.
  - Updated unit tests, integration tests, and server integration tests to assert the note is present.
  - Updated documentation in `docs/personality-system.md`, `docs/wiki/personality.md`, and `docs/wiki/what-works-now.md`.
