# Config Hot-Reload

## Overview

Mimir supports hot-reloading non-sensitive configuration settings at runtime. The design uses a `ReloadableConfig` wrapper, a file watcher, and a `SIGHUP` handler.

## Architecture

```
┌──────────────────────────────────────────────┐
│              ReloadableConfig                 │
│  ┌─────────────────────────────────────────┐ │
│  │  Arc<RwLock<Config>>                    │ │
│  │  PathBuf (config file path)             │ │
│  └─────────────────────────────────────────┘ │
│                                              │
│  snapshot() → Config (clone under read lock) │
│  reload()  → Result<(), ConfigReloadError>   │
└──────────────────────────────────────────────┘
         ▲                    ▲
         │                    │
   ┌─────┴──────┐    ┌───────┴──────┐
   │ File       │    │ SIGHUP       │
   │ Watcher    │    │ Handler      │
   │ (notify +  │    │ (tokio       │
   │  debouncer)│    │  signal)     │
   └────────────┘    └──────────────┘
```

### ReloadableConfig

Located in `mimir-core/src/config/reload.rs`. Wraps the live `Config` behind `Arc<tokio::sync::RwLock<Config>>` and stores the config file path.

**`snapshot()`**: Acquires a read lock, clones the config, and releases the lock. The clone is cheap because `Config` is a small struct (~10 fields). This avoids holding the lock across await points.

**`reload()`**: 
1. Reads the file from disk (`tokio::fs::read_to_string`).
2. Parses the TOML into a `Config`.
3. Compares sensitive fields (`llm.endpoint`, `llm.api_key`, `llm.model`, `server.bind_addr`, `server.socket_path`) against the current snapshot.
4. If any sensitive field changed, returns `ConfigReloadError::SensitiveFieldChanged { field }` and leaves the current config untouched.
5. On success, acquires a write lock, replaces the config, drops the lock, and logs `Config reloaded from {path}`.

### File Watcher

Uses `notify` 8.2.0 with `notify-debouncer-full` 0.7.0:
- Watches the **parent directory** of `config.toml` in `RecursiveMode::NonRecursive`. This handles editors that write to a temp file and rename (Vim, etc.).
- 1-second debounce timeout prevents rapid successive reloads.
- Events are bridged from `spawn_blocking` to a tokio channel.
- Config file changes are filtered by filename (`ends_with("config.toml")`).
- **`Access` events are ignored.** `notify` reports `Access` (open/read/close)
  events, and `reload()` itself reads the file — without filtering, those reads
  fed a self-reload loop that reloaded the config ~once per second even when it
  never changed, flooding the journal.
- **Metadata dedupe.** Before signalling a reload, the watcher compares the
  file's `(mtime, size)` signature against the last reload it requested. Events
  with an unchanged signature are skipped silently, preventing repeated reloads
  when the file metadata has not changed between debounce windows.
- On shutdown, an `AtomicBool` stop flag signals the blocking task to exit within 250ms. The flag is set deterministically by the server's shutdown broadcast (see `docs/shutdown.md`), not left to an `AppState`-drop race.

### SIGHUP Handler (Unix only)

Uses `tokio::signal::unix::signal(SignalKind::hangup())`:
- Spawned as a tokio task that loops on `sighup.recv().await`.
- On each signal, calls `config.reload().await` and logs success or warning.
- Exits when the shutdown watch channel fires.

### Atomicity Guarantees

- **Reads are consistent**: `snapshot()` acquires a read lock and clones in one step. No partial configs are observed.
- **Reloads are atomic**: The write lock is held only while swapping the inner `Config`. The TOML parse and sensitive-field check happen before acquiring the write lock.
- **Parse errors are safe**: If the file is corrupt, the old config is retained and a warning is logged.
- **Sensitive fields are gated**: Changing `llm.endpoint`, `llm.api_key`, `llm.model`, `server.bind_addr`, or `server.socket_path` via hot-reload is rejected. These require a restart.

## Sensitive Field Gate

The following fields are considered sensitive and are not reloadable:

| Field | Rationale |
|-------|-----------|
| `llm.endpoint` | Changing the LLM provider mid-flight could cause authentication issues |
| `llm.api_key` | API key rotation should be explicit, not silent |
| `llm.model` | Model changes can affect token limits, costs, and behaviour unpredictably |
| `server.bind_addr` | Changing the bind address requires a new listener (restart) |
| `server.socket_path` | Changing the socket path requires a new listener (restart) |

## Performance

- `snapshot()` acquires a read lock for the duration of a `Config::clone()`. Config is ~10 fields, all strings/ints/bools, so cloning takes microseconds.
- Multiple concurrent readers can hold the read lock simultaneously.
- The write lock is held only for the assignment (`*write_guard = new_config`), not during file I/O or TOML parsing.

## Tests

Unit tests in `mimir-core/src/config/tests.rs`:
- `test_reloadable_applies_non_sensitive_change`: Changes `personality.preset` and `memory.char_limit`, verifies new values.
- `test_reloadable_rejects_sensitive_change`: Changes `llm.model`, verifies `SensitiveFieldChanged` error and old model retained.
- `test_reloadable_rejects_invalid_toml`: Corrupts the file, verifies `Parse` error and old config retained.
