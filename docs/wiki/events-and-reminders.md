# Events & Reminders

> **Available since:** v0.57.0

## What it is

Mimir automatically tracks upcoming events and reminders — birthdays, appointments, deadlines, tasks — and surfaces the ones approaching soon in your **"Upcoming" memory section**. You don't have to manage a separate to-do list; Mimir learns events from what you say in chat and keeps them in front of you until they're done or their date passes.

## How it works

When you tell Mimir something time-bound (e.g. *"I have a letter to post by tomorrow 5 pm"* or *"Priya's birthday is 15 June"*), it records the fact with a date and attaches an **event** to it. There are three kinds of behaviour:

- **Reminders / one-off events** — auto-complete when the date passes (e.g. a flight, an appointment).
- **Recurring events** — never complete; they roll forward to the next occurrence (e.g. a yearly birthday).
- **Tasks / deadlines** — stay active past their date and show up as **overdue** until you mark them done or dismiss them.

Events also arrive from **connectors**: the CalDAV Calendar connector (#198) turns your synced events into `Appointment`-typed facts (a future-dated appointment auto-completes when it passes; a recurring one rolls forward), so calendar and conversational events share the same Upcoming view. The event kind is a typed hint carried with the fact — chat-derived events stay the `Task`/`Reminder` defaults above.

A background scan runs a couple of times a day (by default 06:00 and 18:00) to discover new upcoming facts, auto-complete past reminders, and advance recurring events to their next date.

## Use cases

- **Birthdays & anniversaries** — say *"X's birthday is 15 June"*; Mimir reminds you each year as the date approaches.
- **Appointments & travel** — mention a future date; it appears in Upcoming and drops off once the date passes.
- **Tasks & deadlines** — say *"I have to post a letter by tomorrow 5pm"*; Mimir keeps it active and flags it overdue if the deadline passes without action.

## Configuration

In `config.toml`:

```toml
[knowledge.events]
schedule_times = ["06:00", "18:00"]  # when the scan runs daily
horizon_days = 30                     # how far ahead to surface upcoming items
```

You can also set these via environment variables, which override the file:

- `MIMIR_KNOWLEDGE_EVENTS_SCHEDULE_TIMES` — comma-separated daily run times, e.g. `07:30,19:45`.
- `MIMIR_KNOWLEDGE_EVENTS_HORIZON_DAYS` — how many days ahead to look, e.g. `30`.

## Sensitive facts

When a fact is sensitive (e.g. a medical appointment) Mimir asks you to confirm it before storing it permanently. The reminder/task overlay is created once you **confirm** the fact: Mimir remembers the recurrence and action metadata from the original extraction and rebuilds the overlay from it on confirmation, so:

- a confirmed **recurring** sensitive reminder (e.g. a yearly checkup) keeps recurring and advances to its next occurrence after the date passes;
- a confirmed sensitive **task/deadline** keeps requiring you to act and stays overdue until you dismiss or complete it;
- a confirmed **one-time** sensitive reminder appears in Upcoming and drops off after its date passes, just like any other reminder.

Until you confirm a sensitive fact it is not stored permanently and never surfaces in Upcoming.

## Best practices

- Include a concrete date when you want something tracked ("by tomorrow 5pm", "on 15 June"); Mimir does **not** guess dates from loose phrasing.
- Mark a task as done by dismissing it once you've handled it; otherwise it will continue to surface as overdue.

## Correcting an event

If you correct a date Mimir already tracks (e.g. *"my anniversary is actually 15 February"*), the old event is retired and only the corrected date keeps surfacing in Upcoming — the old date stops rolling forward and disappears from the section.

## What's not included yet

Proactive pop-up notifications, a dedicated `mimir events` CLI, and smart location-aware completion are planned for a later phase.
