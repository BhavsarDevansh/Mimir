# Tool Call Visibility

## Overview

Mimir now displays tool calls in chat and ask output, and supports an agentic tool loop that allows the LLM to make multiple rounds of tool calls.

## Implementation Details

### SSE Protocol Extension

A new SSE event type `tool_call` is emitted during streaming chat. The data payload is a JSON `ToolCallInfo` object:

```json
{
  "name": "get_current_time",
  "display_name": "Get Current Time",
  "result": "2025-05-30T12:00:00Z"
}
```

### Agentic Tool Loop

Both blocking (`/chat`) and streaming (`/chat/stream`) handlers now loop when the LLM issues tool calls:

1. LLM responds with tool calls → execute each tool → append results to conversation
2. Call LLM again with updated context
3. Repeat until LLM responds without tool calls or `max_tool_rounds` is reached

The `max_tool_rounds` setting defaults to 100 and is configurable via `[agent] max_tool_rounds` in `config.toml`.

### Display Name Resolution

Each tool has a `display_name` derived from its `name()` via `snake_to_title_case` (e.g. `get_current_time` → `Get Current Time`). The display name is stored in `ToolMetadata` at registration time.

### Terminal Rendering

Tool calls are rendered in the CLI using `colored` crate with `.dimmed().italic()` styling:

```
🔧 Get Current Time → 2025-05-30T12:00:00Z
```

### Error Handling

When a tool execution fails during the agentic loop:
- The error is logged via `tracing::error!`
- The error text is sent back to the LLM as the tool result
- The loop continues (the LLM can decide how to handle the error)

## Configuration

```toml
[agent]
max_tool_rounds = 100  # Maximum agentic tool-call rounds
```

## API Changes

- `ChatResponse` now includes a `tool_calls: Vec<ToolCallInfo>` field
- `mimir-api-types::StreamItem` has a new `ToolCall(ToolCallInfo)` variant
- New `ToolCallInfo` struct with `name`, `display_name`, and `result` fields
- `ToolCallInfo::truncate_result()` limits result summaries to 80 characters
