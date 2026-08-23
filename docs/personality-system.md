# Personality System

## Overview

The personality system defines how Mimir communicates. It provides built-in presets and supports custom presets via plain Markdown files. The active preset is composed with persistent memory context into a single system prompt that is injected into every conversation session.

## Module Design

### `Personality`

The central struct lives in `mimir-core/src/personality.rs`. It holds:

- `active_name`: the currently selected preset name.
- `registry`: a `HashMap<String, PresetEntry>` of preset names to their prompt text plus discovery metadata (source and optional description).
- `warnings`: non-fatal diagnostics collected while scanning custom preset files and resolving the active preset (issue #387).

Construction (`Personality::new`) scans the user personalities directory (`~/.config/mimir/personalities/`) and merges custom presets with built-ins. Custom presets override built-ins when names collide.

### Built-In Presets

Built-in presets are hardcoded as private helper methods on `Personality`:

| Preset | Key Traits |
|--------|-----------|
| `transparent` (default) | Warm, efficient, shows work, admits uncertainty, collaborator tone |
| `concise` | Minimal words, bullet points, no reasoning unless asked |
| `warm` | Conversational, acknowledges context, companion-like |
| `formal` | Neutral, structured, full sentences, no contractions, precise |

### Custom Presets

Users create files named `<name>.personality.md` in `~/.config/mimir/personalities/`. The file stem (without the `.personality` suffix) becomes the preset name and the file body is used verbatim as the system prompt. Custom presets override built-ins when names collide, and the file's own description wins on collision.

Descriptions are optional and use minimal YAML frontmatter at the top of the file, delimited by standalone `---` lines; the remainder of the file stays verbatim prompt text:

```markdown
---
description: Cheerful and upbeat companion
---
You are Mimir. You are upbeat, optimistic, and encouraging.
```

Only the `description` key is supported. Unknown frontmatter keys (for example the stale `style`/`verbosity` tone knobs) are ignored with a warning and the preset still loads, multi-line descriptions are collapsed to a single line, and files without frontmatter are treated exactly as before. Files that do not end in `.personality.md` are ignored by design, not warned about. A file that starts with a standalone `---` line but never closes the block, contains invalid YAML, is unreadable, or is not valid UTF-8 is malformed: it is skipped with a warning that names the file and the reason (issue #387).

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

All diagnostics are non-fatal and collected as data (`Personality::warnings`): the daemon logs them via `tracing::warn`, and `mimir personality list` prints them to stderr, so a missing or malformed preset is never silently ignored.

### Preset Scan Caching (issue #453)

`Personality::new`/`from_path` scan the personalities directory on every call, so a naive per-request construction would re-read and re-parse every custom preset file on the hot chat path. The daemon therefore owns a `PersonalityCache` (one instance in `AppState`, shared by every request): `resolve(&PersonalityConfig)` returns a fully resolved `Personality` but only performs the directory scan when a cheap fingerprint says the directory changed.

- The fingerprint is computed from directory and per-file metadata only — file names, sizes, mtimes, and entry kinds for files matching `<name>.personality.md` — never from file contents. Symlinks are followed, so edits to a symlinked preset's target invalidate the cache. On a fingerprint match the cached scan (custom presets + diagnostics) is reused and the active preset is re-resolved from the fresh config, so per-request `personality_preset` overrides still resolve against the cached registry.
- Invalidation covers file content changes (size/mtime), added and removed preset files, and the directory itself being created after startup. An unreadable directory has no stable fingerprint and therefore always rescans, so transient errors cannot pin a stale cache.
- Scan diagnostics are logged only when a scan actually runs, so a persistently malformed preset no longer re-logs on every request; per-request resolution diagnostics (an unknown `personality_preset` falling back to `transparent`) are still logged per request, as before.
- Custom preset files above `MAX_PRESET_FILE_SIZE` (1 MiB, matching the skill-file cap in `mimir-core/src/skills/markdown.rs`) still load, but the scan emits a size-advisory warning because every rescan reads the file in full.
- The one-shot paths are unchanged: `Personality::new` (CLI-side resolution) and `Personality::from_path` (used by `mimir personality list`, which scans once per invocation) keep scanning every call.

`PersonalityCache` is public and documented in `mimir-core/src/personality.rs`; `scan_count()` exposes the number of scans performed and is used by the unit tests to prove cache hits do not rescan.

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
- `Personality::list_presets() -> Vec<PresetInfo>` — name, source (`Builtin`/`Custom`), optional description, sorted by name; `PresetInfo` is `Serialize` for the future `/v1/models` surface (issue #388)
- `Personality::warnings() -> &[PresetWarning]` — non-fatal diagnostics (malformed files, unknown configured preset)
- `Personality::active_name() -> &str`
- `PersonalityCache::resolve(config: &PersonalityConfig) -> Personality` — cache-backed resolution for the daemon's chat path; rescans only when the presets directory fingerprint changed
- `PersonalityCache::resolve_from_path(presets_dir: &Path, preset_name: &str) -> Personality` — path-based variant used by tests
- `PersonalityCache::scan_count() -> u64` — number of directory scans performed (observability/tests)

## CLI: `mimir personality list`

`mimir personality list` renders every available preset (built-in + custom) as a table with `NAME`, `SOURCE`, and `DESCRIPTION` columns, sorted by name. Custom presets without a description show `-`. The command runs entirely in the CLI process — presets are local files — so it works without a daemon, and it prints the same non-fatal diagnostics as the daemon to stderr while still exiting successfully.
