# systemd Integration

## Overview

Mimir can install itself as a systemd **user service** so the daemon starts automatically on login and restarts on failure. The integration is driven by `mimir init` on Linux.

## Service File Generation

`mimir-core/src/systemd.rs::generate_service_file()` produces a `.service` unit with the following properties:

- **Absolute paths** — `ExecStart` uses the resolved path of the current binary at generation time (`std::env::current_exe()`). `ReadWritePaths` lists the absolute paths of the config, data, and cache directories.
- **Restart policy** — `Restart=on-failure` with `RestartSec=5`.
- **Security hardening**:
  - `NoNewPrivileges=true`
  - `ProtectSystem=full`
  - `ProtectHome=read-only`
  - `ReadWritePaths=<config> <data> <cache>`
  - `PrivateTmp=true`
- **Logging** — `StandardOutput=journal` and `StandardError=journal` so logs appear in `journalctl --user -u mimir`.

### Generated File Location

`install_service_file()` writes the content to:

```
~/.config/systemd/user/mimir.service
```

Parent directories are created automatically.

## `SystemdRunner` Trait

```rust
#[async_trait]
pub trait SystemdRunner: Send + Sync {
    async fn daemon_reload(&self) -> Result<(), SystemdError>;
    async fn enable_now(&self, service: &str) -> Result<(), SystemdError>;
}
```

- `RealSystemdRunner` spawns `systemctl --user daemon-reload` and `systemctl --user enable --now <service>` via `tokio::process::Command`.
- `MockSystemdRunner` records each call in a `Vec<String>` for test assertions.

## Orchestration in `mimir init`

After config and memory files are created, `handle_init()` (Linux only):

1. Prompts the user: `Install systemd user service for auto-start? [y/N]:`
2. On **yes**:
   - Calls `generate_and_install_service_file()`.
   - Invokes `RealSystemdRunner::daemon_reload()`.
   - Invokes `RealSystemdRunner::enable_now("mimir")`.
   - Prints success and the `loginctl enable-linger $USER` suggestion.
3. On **no**, EOF, or any `systemctl` failure:
   - Prints manual `systemctl` fallback instructions.
   - `mimir init` still exits successfully because config and memory are already created.

## Platform Behaviour

| Platform | Behaviour |
|----------|-----------|
| Linux    | Prompt for systemd installation |
| macOS    | Print launchd note (future phase) |
| Windows  | Skip silently |

## Important Notes

- If the `mimir` binary is moved after running `mimir init`, the service file still points to the old absolute path. Re-run `mimir init` to regenerate it.
- The unit is a **user service** (`--user`), so it does not require root privileges.
