# Personality

## What Is a Personality?

A personality shapes how Mimir speaks — how much detail it gives, how warm or formal it sounds, and how it handles uncertainty. You can choose from four built-in presets or create your own.

## Built-In Presets

### Transparent (Default)
Mimir shows its work briefly, admits uncertainty, and speaks as a collaborator. Good for most users.

### Concise
Short answers, bullet points, no reasoning unless asked. Good when you want speed.

### Warm
More conversational and companion-like. Acknowledges context naturally. Good when you want an emotional connection.

### Formal
Structured, full sentences, no contractions, precise terminology. Good for professional contexts.

## How to Select a Preset

Edit `~/.config/mimir/config.toml`:

```toml
[personality]
preset = "formal"
```

Or set an environment variable:

```bash
export MIMIR_PERSONALITY_PRESET="concise"
```

## Creating a Custom Personality

1. Create the personalities directory if it does not exist:
   ```bash
   mkdir -p ~/.config/mimir/personalities
   ```

2. Write a file named `<name>.personality.md`:
   ```bash
   cat > ~/.config/mimir/personalities/cheery.personality.md << 'PROMPT'
   You are Mimir. You are upbeat, optimistic, and encouraging. You celebrate small wins and keep things light.
   PROMPT
   ```

3. Reference it in your config:
   ```toml
   [personality]
   preset = "cheery"
   ```

Custom presets override built-ins with the same name. The file body is used verbatim as the system prompt — no special syntax required.

## Examples

| Situation | Suggested Preset |
|-----------|-----------------|
| Daily quick checks | `concise` |
| End-of-day reflection | `warm` |
| Shared workspace / professional | `formal` |
| General use | `transparent` |
