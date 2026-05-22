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
  "session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
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
  -d '{"session_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890", "message": "What is the weather like?"}'
```

## Streaming Responses

For real-time token streaming, use `/chat/stream`:

```bash
curl -X POST http://127.0.0.1:8080/chat/stream \\
  -H "Content-Type: application/json" \\
  -d '{"message": "Tell me a story"}'
```

The server returns `text/event-stream`. Each line of output is a token chunk. The final event is named `usage` and contains token statistics.

## Error Handling

- **400** — Invalid JSON body.
- **404** — Unknown `session_id`.
- **503** — Server is busy. Retry after 5 seconds.
