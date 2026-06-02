# Preferences

Mimir learns what you like and adapts its behaviour accordingly. Preferences are stored in the knowledge graph as first-class data — every preference is backed by a fact, with full provenance and confidence scoring.

---

## What Is a Preference?

A preference is a learned behavioural rule such as:

- "I prefer dark mode in the evening."
- "I want proactivity level set to `important_only`."
- "Notify me silently for calendar reminders."

Each preference has:

- **Key** — a short identifier (e.g. `theme`, `proactivity_level`).
- **Value** — a scalar value stored as text (e.g. `dark`, `important_only`).
- **Category** — CalendarBehaviour, NotificationStyle, FoodPreference, TravelPreference, WorkStyle, CommunicationPreference, or General.
- **Confidence** — 0.0 to 1.0, reflecting how certain Mimir is.
- **Context** — optional conditions (e.g. `time_of_day = evening`) that make the preference apply only in specific situations.
- **Source fact** — every preference must reference a fact that justifies it.

---

## How Preferences Are Created

1. **Fact extraction** — Mimir observes something about you (from a message, calendar event, or direct statement) and records it as a fact with `Predicate::HasPreference`.
2. **Preference inference** — The fact is turned into a preference row, linking back to the source fact.
3. **User override** — You can explicitly set a preference. This marks it as `overridden_by_user = true` and blocks future inferred overwrites.

---

## Contextual Preferences

Preferences can be situational. For example:

| Key | Value | Context |
|-----|-------|---------|
| `theme` | `dark` | `time_of_day = evening` |
| `theme` | `light` | `time_of_day = morning` |
| `theme` | `auto` | *(no context = default)* |

When Mimir looks up a preference, it counts how many context conditions match the current situation. The most specific match wins. If nothing matches, the default (no context) is used.

---

## Confidence and Conflict Resolution

When two preferences compete for the same key, Mimir resolves the conflict automatically:

- **User overrides always win.** If you explicitly set a preference, inferred values cannot overwrite it.
- **Higher confidence wins.** Between two inferred preferences, the one with higher confidence is chosen.
- **Same confidence keeps the oldest.** If confidence is equal, the existing preference stays.

All changes are recorded in the preference audit log, so you can see why a preference was overwritten.

---

## Inspecting and Editing Preferences

Preferences are stored in the local SQLite database (`~/.local/share/mimir/knowledge.db`). You can query them directly or use the API:

- `get_preference(entity_id, key, context)` — contextual lookup.
- `get_preference_by_id(id)` — fetch a single preference.
- `get_preference_contexts(preference_id)` — view context conditions.
- `get_preference_sources(preference_id)` — see where a preference came from.
- `get_preference_audit_log(preference_id)` — review the full change history.

---

## Best Practices

- **Create facts first.** The preference API requires a `source_fact_id`. Always create the underlying fact before creating a preference.
- **Use scalar values.** Store simple strings, numbers, or booleans as text. Avoid JSON blobs.
- **Keep context minimal.** One or two context keys per preference is usually enough. Too many conditions make matching fragile.
- **Set `overridden_by_user = true` sparingly.** Once a preference is user-overridden, inference engines cannot update it. Use this for things you are certain about.
