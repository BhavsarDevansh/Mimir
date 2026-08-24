# Tool Call Visibility

## Overview

Mimir now displays tool calls in chat and ask output, and supports an agentic tool loop that allows the LLM to make multiple rounds of tool calls.

## Implementation Details

### SSE Protocol Extension

Two SSE event types are emitted during streaming chat:

1. **`tool_call_start`** — sent before a tool executes, containing `name` and `display_name` only (no result yet) so the client can show a "working" indicator:

   ```json
   {"name": "get_current_time", "display_name": "Get Current Time"}
   ```

2. **`tool_call`** — sent after the tool completes, containing the full `ToolCallInfo` payload with the result:

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

Tool calls are rendered in the CLI using `colored` crate with `.dimmed().italic()` styling. When a tool starts executing, a "working" indicator is shown; once the result arrives, the full summary is displayed as a separate line:

```text
🔧 Get Current Time…
🔧 Get Current Time → 2025-05-30T12:00:00Z
```

### Nested Tool-Call Progress (retrieval agent)

`retrieve_context` spawns a multi-round retrieval agent that can run for a minute or more. To keep the client from looking frozen behind a single "Retrieve Context…" indicator, the agent's individual sub-tool calls are streamed as the same `tool_call_start` / `tool_call` events:

```text
🔧 Retrieve Context…
🔧 Kg Query…
🔧 Kg Query → {"entity":{"id":1,"name":"TraveLodge",...},"facts":[...],...}
🔧 Kg Search…
🔧 Kg Search → {"results":[...],...}
```

Mechanically, the streaming chat handler creates a `tokio::sync::mpsc` progress channel per tool call and passes the sender through `ToolContext::with_progress` into the registry factory, which rebuilds `RetrieveContextTool` with it. The retrieval agent emits `mimir_core::tools::ToolProgress::Started` before each sub-tool executes and `ToolProgress::Finished` after, and a spawned forwarding task converts those into SSE `tool_call_start` / `tool_call` events (results truncated to 80 chars via `ToolCallInfo::truncate_result`). Non-streaming paths (`/chat`, `/v1/chat/completions`) pass no channel, so the agent's progress is only surfaced on `/chat/stream`.

### Streaming Timeouts

The client's default 120s total request timeout is overridden per request on the chat endpoints, because a retrieval-heavy turn can legitimately run for minutes (issue #487):

- `POST /chat` (blocking) — 10-minute total timeout (`MimirClient::CHAT_TOTAL_TIMEOUT`).
- `POST /chat/stream` — 30-minute total backstop (`MimirClient::CHAT_STREAM_TOTAL_TIMEOUT`) plus a 60-second per-chunk read timeout (`MimirClient::CHAT_STREAM_READ_TIMEOUT`). The daemon emits SSE keep-alive comments every 10s, so the read timeout only fires when the stream is genuinely wedged; a wall-clock total timeout alone would kill long-but-healthy streams and surface as the misleading "error decoding response body" (reqwest wraps the mid-body timeout as a decode error).

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
- `mimir-api-types::StreamItem` variants:
  - `ToolCall(ToolCallInfo)` — emitted after a tool completes
  - `ToolCallStart(ToolCallStartInfo)` — emitted before a tool starts
- `ToolCallInfo` struct with `name`, `display_name`, and `result` fields
- `ToolCallStartInfo` struct with `name` and `display_name` fields
- `ToolCallInfo::truncate_result()` limits result summaries to 80 characters
- The client SSE parser handles both `tool_call_start` and `tool_call` events
