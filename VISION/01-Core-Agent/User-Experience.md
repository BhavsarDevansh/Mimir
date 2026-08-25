# Core Agent — User Experience

## Interaction Modes

### CLI Mode
The primary interface for power users. The agent runs as a persistent background daemon with a CLI client.

```bash
# Start the daemon (runs in foreground; systemd manages backgrounding)
$ mimir start

# Ask a question (talks to daemon via the Unix socket, TCP fallback — see #25)
$ mimir ask "When was I last in Rome?"

# Chat mode (interactive, talks to daemon)
$ mimir chat
> When was I last in Rome?
Investigating your calendar, photos, and emails...
You were last in Rome from May 3–7, 2025. I found a Colosseum tour on May 5th.

# Stop the daemon
$ mimir stop
```

If the daemon is not running when a CLI command is issued:
```
$ mimir ask "hello"
Error: Mimir is not running.
Start the server now? [y/N]: y
Starting mimir... done.
[response streams]
```

### Chat Interface
A local web-based chat UI (or TUI) for conversational interaction.
- Persistent conversation history
- Streaming responses
- Ability to interrupt or redirect reasoning mid-stream
- Toggle "verbose mode" to see reasoning steps

### Configuration
All settings live in `~/.config/mimir/config.toml`:
```toml
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-5"

[agent]
name = "Mimir"
proactivity = "important_only"  # never | important_only | always
verbose_reasoning = false

[server]
bind_addr = "127.0.0.1:8080"                    # TCP listener (remote clients, web UI)
# socket_path = "~/.local/share/mimir/mimir.sock"  # Optional override; defaults to ~/.local/share/mimir/mimir.sock
```

## Personality
The agent should feel like a competent, understated assistant — not overly chatty, not robotic. It communicates clearly, admits uncertainty, and asks clarifying questions when needed. Over time it may adapt tone to match user preference.

## Transparency Controls
- **Silent mode:** Acts without asking (for low-risk actions)
- **Confirm mode:** Asks before acting (for medium-risk)
- **Audit mode:** Shows reasoning trail before answering
