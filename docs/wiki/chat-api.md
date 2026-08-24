# Chat API

Mimir exposes a local HTTP chat API on `http://127.0.0.1:8080`. It is designed for single-user, local-first use.

## Starting a Conversation

Send a `POST /chat` request with no `session_id` to create a new session:

```bash
curl -X POST http://127.0.0.1:8080/chat \\
  -H "Content-Type: application/json" \\
  -d '{"message": "Hello, Mimir!"}'
```

**Response:**
```json
{
  "session_id": 42,
  "response": "Hello! How can I help you today?",
  "usage": {
    "prompt_tokens": 12,
    "completion_tokens": 8,
    "total_tokens": 20
  }
}
```

## Continuing a Conversation

Include the `session_id` from the previous response:

```bash
curl -X POST http://127.0.0.1:8080/chat \\
  -H "Content-Type: application/json" \\
  -d '{"session_id": 42, "message": "What is the weather like?"}'
```

## Streaming Responses

For real-time token streaming, use `/chat/stream`:

```bash
curl -X POST http://127.0.0.1:8080/chat/stream \\
  -H "Content-Type: application/json" \\
  -d '{"message": "Tell me a story"}'
```

The server returns `text/event-stream`. Each line of output is a token chunk. The final event is named `usage` and contains token statistics.

## Listing Sessions

```bash
curl http://127.0.0.1:8080/sessions
```

Returns a JSON array of session summaries ordered by most-recently updated.

## Resuming a Session

```bash
curl http://127.0.0.1:8080/sessions/<session-id>/messages
```

Returns the full message history from the last compaction point (or all messages if never compacted).

## Error Handling

- **400** — Invalid JSON body.
- **404** — Unknown `session_id`.
- **503** — Server is busy. Retry after 5 seconds.
- Streaming failures emit a terminal `event: error` frame whose data explains the cause — for example an upstream LLM provider that is temporarily overloaded (`503`). The message tells you which model or provider to check; a provider overload usually clears by itself, or you can switch models with `/model` in the CLI or the `model` key in `config.toml`.

## OpenAI-Compatible API

Mimir also speaks the OpenAI chat-completions API on `/v1/chat/completions` (with `/v1/models` for the model list), so any OpenAI-compatible app can use Mimir as its LLM provider. See [Using Mimir as Your LLM Provider](llm-provider.md).
