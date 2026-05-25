# systemd Setup

## What `mimir init` Does

When you run `mimir init` on Linux, after creating the default config and memory files, you are asked:

```
Install systemd user service for auto-start? [y/N]:
```

Answering **y** or **yes** will:

1. Create a systemd user service file at `~/.config/systemd/user/mimir.service`.
2. Run `systemctl --user daemon-reload`.
3. Run `systemctl --user enable --now mimir` so the daemon starts immediately and on future logins.
4. Print:
   ```
   Enabled mimir user service.
   Run the following to keep it active when not logged in:
     loginctl enable-linger $USER
   ```

## Manual Fallback

If you answer **no**, or if any `systemctl` command fails, `mimir init` prints:

```
To enable auto-start manually, run:
  systemctl --user daemon-reload
  systemctl --user enable --now mimir
  loginctl enable-linger $USER
```

You can also copy these commands and run them later.

## macOS and Windows

- **macOS**: `mimir init` prints a note about future launchd support.
- **Windows**: No auto-start integration is provided yet; the step is skipped silently.

## Moving the Binary

The service file records the absolute path to the `mimir` binary at the time `mimir init` runs. If you move the binary later (e.g. after a `cargo install` update), re-run `mimir init` to regenerate the service file with the new path.
