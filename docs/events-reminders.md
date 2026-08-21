# Events & Reminders Subsystem (Issue #74)

> **Added:** v0.57.0
>
> **Crate:** `mimir-knowledge`
>
> **Related issues:** #74 (Phase A), #130 (Librarian Agent), #143 (Phase B — proactive notifications)

## Overview

Mimir surfaces approaching deadlines, appointments, birthdays, and tasks in the **"Upcoming" memory section**. Rather than a parallel store, events are modelled as a **lifecycle + recurrence overlay on facts**. A fact whose `valid_from` lies in the future is a one-time event; a fact tagged with recurrence is a recurring event; a fact flagged `requires_user_action` is a task/deadline.

The source fact is the source of truth — it already carries the temporal bound and surfaces in `render_upcoming_section`. The `events` overlay only manages **lifecycle status** and **recurrence advancement**.

## Data Model

### `events` table (migration 039)

| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER PK | |
| `fact_id` | INTEGER UNIQUE FK → `facts(id)` ON DELETE CASCADE | one overlay per fact |
| `entity_id` | INTEGER FK → `entities(id)` ON DELETE CASCADE | denormalized subject for queries |
| `trigger_date` | TIMESTAMP | when the event occurs / occurred |
| `recurrence_type_id` | INTEGER FK → `recurrence_types` | reuses the existing recurrence lookup |
| `event_type_id` | INTEGER FK → `event_types` | birthday, appointment, deadline, task, reminder, custom |
| `status_id` | INTEGER FK → `event_statuses` | Pending, Active, Completed, Dismissed, Snoozed |
| `auto_complete_policy_id` | INTEGER FK → `auto_complete_policies` | AutoCompleteOnDate, RequiresUserAction, Recurring |
| `requires_user_action` | BOOLEAN | task/deadline flag |
| `addressed_at` | TIMESTAMP | set when Completed/Dismissed |
| `created_at` | TIMESTAMP | |

Rust enums `EventType`, `EventStatus`, `AutoCompletePolicy` (`#[repr(i16)]`) mirror the seeded lookup IDs in `models/enums.rs`.

### Deprecation of `entity_dates`

`entity_dates` is superseded by the events overlay. Its recurrence helper `next_occurrence` (plus `parse_base_datetime`, `is_leap_year`, `days_in_month`) moved to `models/recurrence.rs`. The unused `entity_dates` and `entity_date_types` tables are dropped by migration 040. No data migration was required (the table held no rows).

## Event Detection (Deterministic)

Detection is pure Rust, no LLM classification:

- **Future-dated fact** (`valid_from > now`) → one-time event, `AutoCompleteOnDate`, `Reminder`.
- **Recurring** (LLM-emitted `recurrence` ≠ `none`) → `Recurring`.
- **`requires_user_action`** → `RequiresUserAction`, `Task`.

The LLM only supplies structured data — the ISO-8601 `valid_from` (used as `trigger_date`), an optional `recurrence` enum, and an optional `requires_user_action` flag — consistent with the existing `temporal` and `categories` emissions. No natural-language date parsing happens in Rust.

## Extraction Bridge

`extract.rs::event_from_extraction` builds an overlay for any inserted fact that has a `valid_from` and is either future-dated, recurring, or action-required. The overlay is created inside the extraction pipeline (idempotent insert), so the Librarian Agent needs no event-specific logic — it calls `extract_facts_with_context` as before. `event_type` is limited to `Task`/ `Reminder` in Phase A; the remaining `EventType` variants are seeded for later phases that derive richer typing.

Sensitive facts return `Pending` before reaching the event block, so the event shape computed by `event_from_extraction` is persisted in the `pending_event_meta` table (migration 041) at extraction time. On confirmation, `extract.rs::confirm_fact` rebuilds the overlay from that persisted shape (recurrence / `event_type` / `auto_complete_policy` / `requires_user_action`), so a confirmed sensitive recurring reminder keeps recurring and a confirmed sensitive task/deadline keeps requiring user action. The rebuilt overlay is idempotent (`ON CONFLICT(fact_id) DO NOTHING`); the consumed `pending_event_meta` row is deleted on confirm and cascade-deleted on reject. Legacy pending facts that predate the table fall back to a one-time `Reminder` overlay for future-dated facts.

## Scan Job — `events.upcoming_scan`

Registered in `AppState::from_config_with_llm` (`mimir-server/src/state/`), one scheduled job per configured run time (default 06:00 and 18:00). Three deterministic passes:

1. **Derive** — facts with a future `valid_from` and no overlay get a one-time `AutoCompleteOnDate` overlay. Catches facts inserted by non-extraction paths (connectors, `remember` tool, manual edits). The insert is idempotent (`INSERT ... ON CONFLICT(fact_id) DO NOTHING`), so a concurrent extraction cannot trip the `fact_id` unique constraint; `derived` counts only actual inserts. The derive query applies the same `confidence >= 0.5` gate as the Upcoming render, so overlays are only created for facts that will surface.
2. **Auto-complete** — one-time `AutoCompleteOnDate` events whose `trigger_date` has passed transition to `Completed`.
3. **Advance** — recurring events whose `trigger_date` has passed advance to their next occurrence via `next_occurrence`. `get_active_recurring(pool, now)` filters in SQL to `Recurring`-policy events with `requires_user_action = 0` **and** `trigger_date < now`, so only rows that can actually advance are loaded and sorted. A recurring deadline/task that requires user action stays put and surfaces as overdue instead.

Both the auto-complete and advance queries join `facts` and exclude overlays whose fact is `Superseded` or `Forgotten` (`fact_status_id NOT IN (5, 6)`, issue #413), so a stale overlay can never be auto-completed or advanced even if a supersession path forgot to retire it.

`RequiresUserAction` events are intentionally left untouched; past their `trigger_date` they surface as **overdue** via `get_overdue_events`.

## Supersession & the overlay lifecycle

The overlay is a derived view of the fact, so every fact mutation that invalidates the fact must also retire its overlay (issue #413). When a fact transitions to `Superseded`, `queries::fact::status::set_status_tx` dismisses any active overlay (`status_id = Dismissed`, `addressed_at` set) and deletes any persisted `pending_event_meta` row for the fact. Because the retirement lives in the shared status transition, every supersession path stays in sync: the insert pipeline's overlap resolution (`queries::fact/conflict.rs`), the inference engine's contradiction rule, and user status edits via `update_fact_status`. The corrected fact then gets its own overlay through the normal extraction/derive path, so a corrected recurring event surfaces exactly once.

The Upcoming render's recurring branch applies the same `fact_status_id NOT IN (Superseded, Forgotten)` filter as a second line of defense, so a stale overlay never surfaces even if a future supersession path forgets to retire it.

## Rendering

`queries::memory::render_upcoming_section` was refactored from an `entity_dates` + category-900-999 query to an event-based query:

- **One-time:** facts with `valid_from` in `[now, now+horizon]`, LEFT JOIN `events`; included unless the overlay is `Completed`/`Dismissed`.
- **Recurring:** active recurring overlays with `trigger_date` in horizon, joined to facts for display.

Sorted by occurrence, capped at `limit`. Callers (`chat`, `memory`, `status` routes) are signature-compatible.

> **Rendering scope:** the section is rendered for the configured user entity, so only events whose `entity_id` is the user (the user's own deadlines, appointments, and birthday) surface today. Surfacing related contacts' events (e.g. a partner's birthday) is deferred to Phase B (#143), which owns the relationship-aware upcoming view.

## Configuration

```toml
[knowledge.events]
schedule_times = ["06:00", "18:00"]  # daily run times (HH:MM)
horizon_days = 30                     # how far ahead the derive pass looks
```

Both settings honour the standard env-override precedence:

| Env var | Format | Default |
|---------|--------|---------|
| `MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES` | comma-separated `HH:MM` list | `06:00,18:00` |
| `MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS` | integer days | `30` |

The render path labels each item with a calendar-day-relative suffix (`today`, `in 1 day`, `in N days`) computed from `date_naive()` so an event early the next calendar day is never mislabelled `today`.

## Key Files

- `mimir-knowledge/src/models/event.rs` — `Event`, `NewEvent`
- `mimir-knowledge/src/models/recurrence.rs` — `next_occurrence` + helpers
- `mimir-knowledge/src/queries/event.rs` — CRUD, active/overdue, scan helpers
- `mimir-knowledge/src/events.rs` — `run_upcoming_scan` + `ScanSummary`
- `mimir-knowledge/src/extract/` — `event_from_extraction`, `parse_recurrence`
- `mimir-knowledge/src/queries/memory/render.rs` — `render_upcoming_section`
- `mimir-knowledge/src/db/migrations/039_create_events.sql`
- `mimir-knowledge/src/db/migrations/040_drop_entity_dates.sql`

## Non-Goals (Phase A)

- Proactive notifications / toast alerts (Phase 5, issue #143)
- Context-aware smart completion (Phase 5)
- CLI commands `mimir events` / `mimir reminders` (Phase 5)
- Web UI for event management
