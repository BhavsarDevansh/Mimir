# OpenAI-Compatible Provider Surface

## Overview

The Mimir daemon exposes an OpenAI-compatible provider surface so any app or device that speaks the OpenAI chat-completions API can point at one central Mimir server instead of an upstream LLM. Requests are mapped onto Mimir's existing session, personality, and worker-pool infrastructure, so every device's conversations feed one shared profile (VISION/08-Architecture/Multi-Device.md). This is the server-side mirror of the OpenAI wire contract the `mimir` binary already consumes as a client (issue #3).

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/models` | List personality presets as OpenAI models |
| `POST` | `/v1/chat/completions` | Blocking or streaming chat completion |

Both routes sit behind the same bearer-token auth middleware as every other route except `GET /health` (issue #281): present `Authorization: Bearer <token>` with the daemon's `api_token`.

## Session Mapping

Mimir is single-tenant, so there is no device identity. The OpenAI `user` field is a conversation key — the same job as `session_id` in the native API:

- A request with a fixed `user` value resumes one ongoing conversation in the central profile. The mapping is backed by a nullable unique `user_key` column on the `sessions` table; the first request for a key creates the session (race-safe via a partial unique index), and later requests resume it.
- The client-supplied `messages` array is a stateless echo of history Mimir already stores. Only the last user message is appended as the new turn, and Mimir's stored history stays authoritative — exactly like `mimir chat`. Trailing `tool` messages after the last user message are the client's tool results and continue the in-flight turn; trailing assistant messages are ignored because the server already persisted them.
- Requests without `user` behave like incognito: they still pull the user's memory context (condensed memory, upcoming events, catalogue) into the system prompt, but persist nothing and trigger no learning hooks. The client's trailing segment (from the last user message onward) is the ephemeral conversation.

## Model Mapping

The request `model` is resolved in two ways:

- A name matching a personality preset (built-in or custom, issue #387) selects that preset for the system prompt.
- Any other name passes through to the upstream LLM config as a model override (the existing `model` behaviour) with the configured default personality.

`GET /v1/models` lists the available presets with their descriptions. `description` is a Mimir extension; `created` is always `0` because presets have no upstream creation time.

## Tools

Client-supplied `tools` schemas are merged with Mimir's own server-side tools:

- Server tools are always available and execute server-side; on a name collision the server-side tool wins and the client's definition is silently dropped.
- When the LLM calls a client tool, the tool call is returned to the client (`finish_reason: "tool_calls"`, `message.tool_calls` / streamed `delta.tool_calls`), the assistant tool-call message and any server tool results are persisted into the session, and the client's follow-up `tool` messages continue the turn.
- `remember` stays a server-side hook (issue #386): it fires only when the turn actually completes, and never for incognito turns.
- Mimir's internal tool activity is invisible on the v1 surface; surfacing it in chat output is tracked separately (issue #464).

## Sampling

- Per-request `temperature` wins over the configured temperature.
- Per-request `max_tokens` (or `max_completion_tokens`) applies only when the client sends it — by default no `max_tokens` is forced. This is implemented as `LlmBackend::with_max_tokens_override`, mirroring `with_temperature_override`.

## Streaming

`stream: true` returns an SSE stream of OpenAI chunks (`object: "chat.completion.chunk"`): the first chunk carries `delta.role`, content arrives as `delta.content`, tool calls as `delta.tool_calls`, the final chunk carries `finish_reason`, and the stream terminates with `data: [DONE]`. `stream_options.include_usage: true` appends a final usage chunk with empty `choices` and the accumulated `usage`. A mid-stream upstream failure terminates the stream without `[DONE]` (matching OpenAI's connection-drop behaviour); the pre-flight queue-capacity check surfaces a full queue as `503` before the stream starts.

## Errors

`/v1` routes return the OpenAI error JSON shape `{"error": {"message", "type", "param", "code"}}`. Malformed requests — invalid JSON, no user message, an empty user message, or a `tool` message without `tool_call_id` — map to `400 invalid_request_error`. A full worker-pool queue maps to `503 Service Unavailable` with `Retry-After: 5` and `code: "queue_full"`. The mapping is defensive today: the chat path currently bypasses the worker pool when temperature/model overrides are applied (issue #465), so queue-full backpressure is dead code on the hot path until that bypass is fixed.

## Implementation Notes

- Wire types live in `mimir-api-types/src/chat.rs` (`OpenAiChatRequest`, `OpenAiChatResponse`, `OpenAiStreamChunk`, `OpenAiModelList`, `OpenAiErrorBody`, …).
- Routes live in `mimir-server/src/routes/openai.rs` and share the memory-context, system-prompt, and tool-execution helpers with the native chat routes (`mimir-server/src/routes/chat.rs`).
- `ContextManager` gained the `user_key` column (with migration), `resolve_openai_session` (lookup-or-create, race-safe), tool-message persistence (`add_tool_message`, `add_assistant_tool_calls_message`), and turn-based trimming so tool messages are never orphaned when old turns are trimmed.
- `LlmBackend::with_max_tokens_override` applies a per-request `max_tokens` without forcing a default cap.

## Testing

Unit tests cover the wire-type round-trips (`mimir-api-types`), the session mapping, tool-message persistence, migration, and turn-based trimming (`mimir-core/src/context/tests.rs`), and the `max_tokens` override (`mimir-core/src/llm/client/tests.rs`). Server integration tests in `mimir-server/tests/openai_tests.rs` cover model listing, blocking and streaming shapes, session resumption, incognito behaviour, preset selection, client-tool round-trips, server-tool collision and execution, auth, and the 503 error shape.
