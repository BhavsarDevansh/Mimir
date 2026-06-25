# Events & Reminders Subsystem (Issue #74)

> **Added:** v0.57.0
> **Crate:** `mimir-knowledge`
> **Related issues:** #74 (Phase A), #130 (Librarian Agent), #143 (Phase B — proactive notifications)

## Overview

Mimir surfaces approaching deadlines, appointments, birthdays, and tasks in the
**"Upcoming" memory section**. Rather than a parallel store, events are modelled
as a **lifecycle + recurrence overlay on facts**. A fact whose `valid_from` lies
in the future is a one-time event; a fact tagged with recurrence is a recurring
event; a fact flagged `requires_user_action` is a task/deadline.

The source fact is the source of truth — it already carries the temporal bound
and surfaces in `render_upcoming_section`. The `events` overlay only manages
**lifecycle status** and **recurrence advancement**.

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
| `auto_complete_policy_id` | INTEGER FK → `auto_complete_policies` | AutoCompleteOnDate, RequiresUserAction, RecurringYearly |
| `requires_user_action` | BOOLEAN | task/deadline flag |
| `addressed_at` | TIMESTAMP | set when Completed/Dismissed |
| `created_at` | TIMESTAMP | |

Rust enums `EventType`, `EventStatus`, `AutoCompletePolicy` (`#[repr(i16)]`)
mirror the seeded lookup IDs in `models/enums.rs`.

### Deprecation of `entity_dates`

`entity_dates` is superseded by the events overlay. Its recurrence helper
`next_occurrence` (plus `parse_base_datetime`, `is_leap_year`, `days_in_month`)
moved to `models/recurrence.rs`. The unused `entity_dates` and
`entity_date_types` tables are dropped by migration 040. No data migration was
required (the table held no rows).

## Event Detection (Deterministic)

Detection is pure Rust, no LLM classification:

- **Future-dated fact** (`valid_from > now`) → one-time event,
  `AutoCompleteOnDate`, `Reminder`.
- **Recurring** (LLM-emitted `recurrence` ≠ `none`) → `RecurringYearly`.
- **`requires_user_action`** → `RequiresUserAction`, `Task`.

The LLM only supplies structured data — the ISO-8601 `valid_from` (used as
`trigger_date`), an optional `recurrence` enum, and an optional
`requires_user_action` flag — consistent with the existing `temporal` and
`categories` emissions. No natural-language date parsing happens in Rust.

## Extraction Bridge

`extract.rs::event_from_extraction` builds an overlay for any inserted fact that
has a `valid_from` and is either future-dated, recurring, or action-required.
The overlay is created inside the extraction pipeline (DRY: one place), so the
Librarian Agent needs no event-specific logic — it calls
`extract_facts_with_context` as before.

## Scan Job — `events.upcoming_scan`

Registered in `AppState::from_config_with_llm` (`mimir-server/src/state.rs`),
one scheduled job per configured run time (default 06:00 and 18:00). Three
deterministic passes:

1. **Derive** — facts with a future `valid_from` and no overlay get a one-time
   `AutoCompleteOnDate` overlay. Catches facts inserted by non-extraction paths
   (connectors, `remember` tool, manual edits).
2. **Auto-complete** — one-time `AutoCompleteOnDate` events whose
   `trigger_date` has passed transition to `Completed`.
3. **Advance** — recurring events whose `trigger_date` has passed advance to
   their next occurrence via `next_occurrence`.

`RequiresUserAction` events are intentionally left untouched; past their
`trigger_date` they surface as **overdue** via `get_overdue_events`.

## Rendering

`queries::memory::render_upcoming_section` was refactored from an
`entity_dates` + category-900-999 query to an event-based query:

- **One-time:** facts with `valid_from` in `[now, now+horizon]`, LEFT JOIN
  `events`; included unless the overlay is `Completed`/`Dismissed`.
- **Recurring:** active recurring overlays with `trigger_date` in horizon,
  joined to facts for display.

Sorted by occurrence, capped at `limit`. Callers (`chat`, `memory`, `status`
routes) are signature-compatible.

> **Rendering scope:** the section is rendered for the configured user entity,
so only events whose `entity_id` is the user (the user's own deadlines,
appointments, and birthday) surface today. Surfacing related contacts' events
(e.g. a partner's birthday) is deferred to Phase B (#143), which owns the
relationship-aware upcoming view.

## Configuration

```toml
[knowledge.events]
schedule_times = ["06:00", "18:00"]  # daily run times (HH:MM)
horizon_days = 30                     # how far ahead the derive pass looks
```

## Key Files

- `mimir-knowledge/src/models/event.rs` — `Event`, `NewEvent`
- `mimir-knowledge/src/models/recurrence.rs` — `next_occurrence` + helpers
- `mimir-knowledge/src/queries/event.rs` — CRUD, active/overdue, scan helpers
- `mimir-knowledge/src/events.rs` — `run_upcoming_scan` + `ScanSummary`
- `mimir-knowledge/src/extract.rs` — `event_from_extraction`, `parse_recurrence`
- `mimir-knowledge/src/queries/memory.rs` — `render_upcoming_section`
- `mimir-knowledge/src/db/migrations/039_create_events.sql`
- `mimir-knowledge/src/db/migrations/040_drop_entity_dates.sql`

## Non-Goals (Phase A)

- Proactive notifications / toast alerts (Phase 5, issue #143)
- Context-aware smart completion (Phase 5)
- CLI commands `mimir events` / `mimir reminders` (Phase 5)
- Web UI for event management
