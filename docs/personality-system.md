# Personality System

## Overview

The personality system defines how Mimir communicates. It provides built-in presets and supports custom presets via plain Markdown files. The active preset is composed with persistent memory context into a single system prompt that is injected into every conversation session.

## Module Design

### `Personality`

The central struct lives in `mimir-core/src/personality.rs`. It holds:

- `active_name`: the currently selected preset name.
- `registry`: a `HashMap<String, String>` of all available preset names to their raw system prompt text.

Construction (`Personality::new`) scans the user personalities directory (`~/.config/mimir/personalities/`) and merges custom presets with built-ins. Custom presets override built-ins when names collide.

### `PersonalityPreset`

Built-in presets are hardcoded as private helper methods on `Personality`:

| Preset | Key Traits |
|--------|-----------|
| `transparent` (default) | Warm, efficient, shows work, admits uncertainty, collaborator tone |
| `concise` | Minimal words, bullet points, no reasoning unless asked |
| `warm` | Conversational, acknowledges context, companion-like |
| `formal` | Neutral, structured, full sentences, no contractions, precise |

### Custom Presets

Users create files named `<name>.personality.md` in `~/.config/mimir/personalities/`. The file stem (without `.personality.md`) becomes the preset name, and the file body is used verbatim as the system prompt. No frontmatter or TOML parsing is performed.

Files that do not end in `.personality.md` are ignored.

### System Prompt Composition

```rust
pub fn system_prompt(&self, memory_content: &str) -> String
```

- Appends the shared **operating directives** to every preset (built-in or custom), so the behavioural contract — honesty and retrieval — is uniform across personalities:
  ```text
  {preset_system_prompt}

  Operating principles:
  - Do not invent facts about the user. If you do not know the answer, say so.
  - If you need more information, use the `retrieve_context` tool to dispatch a
    retrieval agent that investigates the knowledge graph and conversation
    history. If its findings are still not enough, refine the task and dispatch
    again. Continue until you have a confident answer or have confirmed the
    information is not in your knowledge base.
  ```
- If `memory_content` is non-empty, appends a core-facts block under the header `Core facts about the user (condensed subset — not a complete picture; treat as starting context, not exhaustive):` followed by the condensed memory.
- If `memory_content` is empty, the core-facts block is omitted but the operating directives are still appended.

Memory facts are injected automatically by the server from the knowledge graph condensation pipeline. The `kg_query`/`kg_search`/`kg_related` tools are the retrieval agent's internal tools and are deliberately not mentioned in the system prompt; the core LLM dispatches deeper retrieval via `retrieve_context`. Learning is hook-driven (issue #386): the `remember.chat` background hook extracts facts after each non-incognito turn, so the system prompt carries no learning directive and the model cannot be steered into or out of remembering.

This composition is the responsibility of `Personality`; the caller passes the resulting string to `ContextManager::create_session`.

### Preset Resolution Order

1. Start with built-in presets.
2. Scan custom directory and overlay matches.
3. If the requested preset name is not found, warn and fall back to `transparent`.

## Config Integration

```toml
[personality]
preset = "transparent"
```

Override via environment:
```bash
export MIMIR_PERSONALITY_PRESET="formal"
```

## API Summary

- `Personality::new(config: &PersonalityConfig) -> Self`
- `Personality::from_path(presets_dir: &Path, preset_name: &str) -> Self`
- `Personality::system_prompt(memory_content: &str) -> String`
- `Personality::list_presets() -> Vec<&str>`
- `Personality::active_name() -> &str`
