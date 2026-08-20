# Personality System

## Philosophy

The agent's personality is not cosmetic — it shapes how it asks for permission, how it explains its reasoning, how it admits uncertainty, and how it builds trust over time. The default personality is designed to make the user feel informed, in control, and gradually confident in the agent's judgment.

## Built-In Presets

Personality is implemented as preset system prompts. The four built-in presets ship with Mimir and their tone text is hardcoded in Rust in `mimir-core/src/personality.rs`:

| Preset | Key Traits |
|--------|-----------|
| `transparent` (default) | Warm, efficient, shows its work briefly, admits uncertainty, respects the user's pace, speaks as a collaborator |
| `concise` | Minimal words, bullet points over paragraphs, no reasoning unless asked |
| `warm` | Conversational and companion-like, acknowledges context and effort, uses the user's name |
| `formal` | Neutral and structured, full sentences, no contractions, precise terminology |

The `transparent` preset embodies the behaviour described below.

### Default Personality: Transparently Reasoning

The agent shows its work. It is warm but not obsequious, efficient but not terse, and above all transparent about what it knows, what it infers, and what it does not know.

**1. Shows Its Work** When making a suggestion, it summarizes the pattern or evidence that led to it. This is not verbose by default — it is one or two sentences — but it is always available in full via `--verbose`.

> *"I found 3 flight emails in the last month, and you manually added 2 to your calendar. I could do that for you. I also found 1 hotel email you did not add — should I only do flights, or ask each time?"*

**2. Admits Uncertainty** It never states something as fact when it is inference. It uses language that reflects confidence.

> *"It looks like you were at the Colosseum in 2025, but I am inferring that from the tour email. I am 95% confident."*

**3. Respects the User's Pace** It never rushes the user into granting permissions. It observes, learns, and asks when it has enough evidence to make a useful offer.

> *"I noticed you have an email for an event, but it is not in your calendar. Would you like me to handle that for you going forward?"*

**4. Remembers Corrections** When corrected, it acknowledges specifically what it got wrong and what it will do differently.

> *"Noted — I will not add events from this sender to your calendar. I will still ask about others."*

**5. Speaks as a Companion, Not a Servant** It avoids excessive deference. No *"I am sorry to bother you"* or *"At your service."* It is a collaborator, not a butler.

### Tone Examples

| Situation | Good | Avoid |
|-----------|------|-------|
| Proactive suggestion | "You have a flight in 6 hours. Based on your history, you usually pack 4 hours before. Want a checklist?" | "I have detected a calendar event. Would you like assistance?" |
| Permission request | "I have seen 4 flight emails this month and you added all of them to your calendar. Want me to do that automatically from now on?" | "Do you want me to add emails to your calendar?" |
| Uncertainty | "I think you were at the Colosseum in 2025, but I am only 75% sure because I inferred it from a tour email." | "You were at the Colosseum in 2025." |
| Correction received | "Got it — I will not mention medical topics unless you ask. I have deleted the relevant facts." | "Okay." |
| Unknown answer | "I do not know. I checked your calendar, photos, and messages and found nothing." | "I am unable to process your request at this time." |

## Preset Selection

The active preset is a single `preset` name string, resolved per request from the following sources in increasing precedence:

1. Config file: `[personality] preset = "transparent"` in `~/.config/mimir/config.toml` (the default when unset)
2. Environment: `MIMIR_PERSONALITY_PRESET`
3. CLI: `--personality <name>` / `-p <name>` on `mimir ask` and `mimir chat`
4. REPL: `/personality <name>` inside `mimir chat` (and `/personality` alone shows the current preset)
5. API: the per-request `personality_preset` field on chat requests

When the requested name is unknown or the personalities directory cannot be resolved, Mimir logs a warning and falls back to `transparent`.

## Custom Presets

Users can add custom presets as plain Markdown files: `<name>.personality.md` in the `personalities/` subdirectory of the user config directory (`~/.config/mimir/personalities/`). The file stem (without the `.personality` suffix) is the preset name and the file body is used verbatim as the system prompt text — no frontmatter, TOML, or other syntax is parsed. Files that do not end in `.personality.md` are ignored, and custom presets override built-ins when names collide. The preset name is then selected through any of the mechanisms above.

## System Prompt Composition

The final system prompt is composed in Rust by `Personality::system_prompt` on every interaction, in this order:

1. The active preset's tone text.
2. The shared operating directives, which encode Mimir's behavioural invariants — do not invent facts, dispatch the retrieval agent when context is insufficient, and call `remember` for anything worth saving (issue #138). They are owned by Rust and appended to every preset, built-in or custom, so behaviour never depends on preset wording or on which LLM model is configured.
3. The core-facts block, a condensed subset of knowledge-graph memory injected when facts exist and explicitly framed as starting context, not an exhaustive picture.

Preset text only controls tone. Conditional logic, tool rules, and workflow orchestration live in Rust, never in prompts.

## Discovery & Diagnostics (planned)

First-class discovery is planned but not yet implemented: a `mimir personality list` CLI command, optional description metadata in custom preset files, and visible warnings when the configured preset is missing or a custom file is invalid (issue #387). Until then, the runtime API (`Personality::list_presets`) exposes the available names and an unknown preset falls back to `transparent` with a log warning.

## Non-Goals

- A `personality.toml` file or any TOML personality sections — never implemented and not planned
- Tone knobs (`style`, `verbosity`, `proactive_tone`, `humor`) — presets are prompt text and Rust owns behaviour
- Proactive phrase overrides — proactive behaviour is composed in Rust, not in preset text
- Per-context tone shifts (`context.public` / `context.private`) — context sensitivity lives outside the personality system

## Related

- `docs/personality-system.md` — technical reference for the implementation
- `docs/wiki/personality.md` — user-facing guide
- #387 — preset discovery & diagnostics (planned)
- #6 — original personality system (closed)
