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

## Memory Context

When Mimir has learned facts about you from the knowledge graph, it injects a short memory section into the system prompt before each chat turn:

```
{preset tone text}

Operating principles:
- Do not invent facts about the user. If you do not know the answer, say so.
- If you need more information, use the `retrieve_context` tool to dispatch a
  retrieval agent that investigates the knowledge graph and conversation
  history. If its findings are still not enough, refine the task and dispatch
  again. Continue until you have a confident answer or have confirmed the
  information is not in your knowledge base.

Core facts about the user (condensed subset — not a complete picture; treat
as starting context, not exhaustive):
[condensed memory text]
```

The **operating principles** are appended to every preset — built-in or custom — so the behavioural contract (honesty and retrieval) holds regardless of personality tone. They tell the LLM that the injected facts are a curated subset, not a complete record: when the core facts are insufficient it should dispatch a retrieval agent via the `retrieve_context` tool rather than inventing answers. Learning is handled by the server-side `remember.chat` background hook, so the prompt carries no learning directive and the model cannot be steered into or out of remembering. The lower-level `kg_query`/`kg_search`/`kg_related` tools are the retrieval agent's internal tools and are not surfaced to the core LLM.

If no memory facts exist yet, the core-facts block is omitted — but the operating principles are still appended so the "do not invent facts" rule always applies.

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
