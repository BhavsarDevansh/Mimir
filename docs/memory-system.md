# memory.md System

## Overview

`memory.md` is Mimir's **working memory** — a small, curated, always-hot cache of critical facts that is injected into every system prompt. It acts as the agent's executive summary: not the full knowledge base, but the index card pointing to the most important chapters.

## Design Rationale

- **Speed**: Already in the prompt; no database query needed.
- **Cost**: ~900 tokens max, keeping inference cheap.
- **Cache performance**: Frozen per session to preserve LLM prefix-cache hits.
- **Hot-reloadable**: Writes to disk immediately, but active sessions see the snapshot taken at start.

## Architecture

```text
┌─────────────────────────────────────────────┐
│  System Prompt (injected memory.md snapshot)  │
└─────────────────────────────────────────────┘
              ↑
        MemorySnapshot (frozen at session start)
              ↑
        MemoryManager (live content + disk I/O)
              ↑
        MemoryLoader (read / create default)
              ↑
        ~/.config/mimir/memory.md
```

## Components

### `MemoryLoader`

Utility for reading `memory.md` from disk. Responsible for:
- Reading the file if it exists.
- Creating parent directories and writing the default template if missing.
- Returning the default template content.

### `MemoryManager`

The live memory controller. Owned by the core agent for the duration of a session.

| Method | Purpose |
|--------|---------|
| `new(path, char_limit)` | Load existing file or create default. |
| `content()` | Return the live in-memory string. |
| `current_chars()` | Unicode scalar count. |
| `remaining_chars()` | Capacity left. |
| `usage_pct()` | Usage 0.0–100.0. |
| `is_full()` | At or over limit. |
| `add(entry)` | Append entry, save to disk. Fails if over limit. |
| `replace(old, new)` | Single-match replace, save to disk. Fails on ambiguity or overflow. |
| `remove(old_text)` | Single-match removal, save to disk. Fails on ambiguity. |
| `snapshot()` | Clone current content into a `MemorySnapshot`. |
| `refresh()` | Reload from disk (for new sessions). |
| `save()` | Explicit disk persistence. |

### `MemorySnapshot`

An immutable clone of `memory.md` content taken at a specific point in time. Injected into the system prompt by the Context Manager (Task 5). Future mutations to `MemoryManager` do **not** affect an already-taken snapshot.

## Default Template

The default `memory.md` is a free-form agent scratchpad with no rigid sections:

```text
Mimir memory [0/2500]

No memories yet.
```

The agent writes compact, self-contained notes (one thought per line or bullet). It groups related facts organically — for example, placing the user's profession next to their location if it feels natural, or keeping birthday facts with relationship notes. There is no enforced structure.

Agents are instructed to be token-conscious (abbreviate, drop filler words, use shorthand) and to use `replace` on the exact existing note to avoid duplication.

## Capacity Management

- **Hard limit**: 2,500 characters (~900 tokens), configurable via `memory.char_limit` in `config.toml`.
- **Tracking**: `current_chars()`, `remaining_chars()`, `usage_pct()` provide accurate real-time metrics.
- **Overflow prevention**: `add()` and `replace()` fail before exceeding the limit.
- **Consolidation**: When memory is nearly full, the Reasoning Engine (Phase 4) can use `replace()` and `remove()` to merge or evict entries. The manager provides the primitives; the engine decides policy.

## File Location

```shell
~/.config/mimir/memory.md
```

Created automatically on first run if missing.

## Frozen Snapshot Behaviour

1. Session starts → `MemoryManager::new()` loads from disk.
2. Context Manager calls `snapshot()` → `MemorySnapshot` is cloned.
3. Snapshot is injected into the system prompt and remains stable.
4. Agent calls `add()` / `replace()` / `remove()` → changes write to disk immediately.
5. Snapshot is **not** updated mid-session.
6. Next session starts → `MemoryManager::new()` loads the updated file, new snapshot taken.

This mirrors the Hermes Agent pattern and preserves prefix-cache performance.

## MemoryTool (LLM Integration)

The `memory` built-in tool gives the LLM a mechanism to update `memory.md` autonomously whenever it learns something about the user that should persist across sessions.

### Actions

| Action | Parameters | Behaviour |
|--------|-----------|-----------|
| `add` | `content` (string) | Appends content to memory.md, inserting a newline separator if needed. Fails if the limit would be exceeded. |
| `replace` | `old_text` (string), `content` (string) | Finds the first (and only) occurrence of `old_text` and substitutes `content`. Fails if ambiguous or would overflow. |
| `remove` | `old_text` (string) | Deletes the first (and only) occurrence of `old_text`. Fails if ambiguous. |

### Tool Description (LLM-facing)

> Update Mimir's persistent working memory (memory.md). memory.md is your personal scratchpad — a free-form text file where you record facts about the user and context that should persist across sessions. Write compact, self-contained notes (one thought per line or bullet). Group related facts together, but do not use rigid sections or prefixes. Prefer 'replace' to update an existing note. Use 'add' for new observations. Use 'remove' to delete stale notes. Be token-conscious: abbreviate, drop filler words, use shorthand. The file has a 2500 character limit.

### Registration

`MemoryTool` is **not** part of `ToolRegistry::with_builtins()` because it requires runtime configuration (`memory.path` and `memory.char_limit`). It is registered explicitly in:
- `mimir-server/src/state.rs` — daemon startup, using the resolved config.
- `mimir/src/commands.rs` — CLI tool list, using the default platform path.

### Lazy Initialisation

The underlying `MemoryManager` is created on the first tool execution. This allows the tool to be registered synchronously while still performing async file I/O when actually used.

## Error Model

| Error | Cause |
|-------|-------|
| "Memory full" | `add()` or `replace()` would exceed `char_limit`. |
| "not found" | `replace()` or `remove()` target does not exist. |
| "matches N entries" | Target is ambiguous (occurs more than once). |

## Future Work

- **Structured parsing**: Currently treated as opaque text. Future iterations may parse sections for smarter consolidation.
- **LLM-driven consolidation**: The Reasoning Engine (Phase 4) will decide what to merge/evict.
- **Hot-reload watch**: `notify` crate integration to detect external user edits.
- **Obsidian sync**: Import/export with YAML frontmatter (Phase 2).
