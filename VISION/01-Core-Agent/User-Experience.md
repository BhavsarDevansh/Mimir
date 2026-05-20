# Core Agent — User Experience

## Interaction Modes

### CLI Mode
The primary interface for power users. The agent runs as a persistent background daemon with a CLI client.

```bash
# Start the daemon
$ agent start

# Ask a question
$ agent ask "When was I last in Rome?"

# Chat mode (interactive)
$ agent chat
> When was I last in Rome?
Investigating your calendar, photos, and emails...
You were last in Rome from May 3–7, 2025. I found a Colosseum tour on May 5th.

# Proactive notification
$ agent notify "You have a flight to Tokyo in 6 hours. It's long-haul and the forecast shows rain — pack an umbrella and your warmer jacket from storage."
```

### Chat Interface
A local web-based chat UI (or TUI) for conversational interaction.
- Persistent conversation history
- Streaming responses
- Ability to interrupt or redirect reasoning mid-stream
- Toggle "verbose mode" to see reasoning steps

### Configuration
All settings live in `~/.config/agent/config.toml`:
```toml
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-5"

[agent]
name = "Ariadne"
proactivity = "important_only"  # never | important_only | always
verbose_reasoning = false
```

## Personality
The agent should feel like a competent, understated assistant — not overly chatty, not robotic. It communicates clearly, admits uncertainty, and asks clarifying questions when needed. Over time it may adapt tone to match user preference.

## Transparency Controls
- **Silent mode:** Acts without asking (for low-risk actions)
- **Confirm mode:** Asks before acting (for medium-risk)
- **Audit mode:** Shows reasoning trail before answering
