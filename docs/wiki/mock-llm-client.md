# Mock LLM Client

## What Is It?

The `MockLlmClient` is a programmable stand-in for the real OpenAI-compatible HTTP client. It lets you write fast, deterministic tests that do not touch the network.

## How It Works

You queue up responses in the order you expect them to be consumed:

```rust
let mock = MockLlmClient::builder()
    .push_chat("Hello!", Usage::default())
    .push_stream(vec![
        Ok(StreamItem::Text("Hi")),
        Ok(StreamItem::Usage(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 })),
    ])
    .user_queue_depth(3)
    .build();
```

When the application calls `mock.chat(messages)`, the mock records the messages and pops the next queued response. If the queue is empty it returns `LlmError::RetryExhausted` so tests fail loudly instead of silently succeeding.

## Why Use It?

- **Speed**: no HTTP round-trips, no TLS, no DNS.
- **Determinism**: the same test always produces the same result.
- **Observability**: `mock.chat_calls()` returns every message vector that was sent, so you can assert on the exact prompt the backend received.

## Availability

`MockLlmClient` is compiled when `mimir-core` is built for tests, or when the `mock-llm` Cargo feature is enabled. It is **not** included in release builds of the main `mimir` binary.
