# Tool Calls in Chat

## What's New

When Mimir uses a tool during a conversation, you'll now see it in the chat output. Tool calls appear in a subtle dimmed/italic style so they're visible but don't distract from the main response.

## How It Looks

When Mimir starts executing a tool, you'll first see a "working" indicator:

```text
🔧 Get Current Time…
```

Once the tool finishes, the result replaces it:

```text
🔧 Get Current Time → 2025-05-30T12:00:00Z
```

This appears before the assistant's text response.

## Agentic Tool Loop

Mimir can now make multiple rounds of tool calls in a single conversation turn. This means it can:

1. Call a tool to gather information
2. Use that result to decide what to do next
3. Call another tool if needed
4. Repeat until it has enough information to respond

There's a safety limit of 100 rounds by default (configurable).

## Configuration

You can adjust the maximum number of tool-call rounds in your `config.toml`:

```toml
[agent]
max_tool_rounds = 100  # Default: 100
```

- Lower values make Mimir respond faster but may limit its ability to research complex questions
- Higher values allow deeper investigation but may take longer

## How It Works

1. You send a message
2. Mimir's LLM decides which tools to call (if any)
3. A "working" indicator is shown as each tool starts
4. Each tool executes and its result is shown in the chat
5. Mimir's LLM sees the results and decides if it needs more information
6. Steps 2-5 repeat until Mimir has enough context to answer
7. Mimir sends its final response
