# LLM Backend Abstraction

## Overview

The `LlmBackend` trait (`mimir-core/src/llm/backend.rs`) decouples the rest of the
application from the concrete HTTP client implementation. This enables fast,
deterministic tests via `MockLlmClient` and leaves the door open for future
alternative backends (e.g. local models, different providers).

## Trait Design

```rust
#[async_trait]
pub trait LlmBackend: Send + Sync + Debug {
    async fn chat(&self, messages: Vec<Message>) -> Result<(String, Usage), LlmError>;
    async fn chat_stream_with_usage(&self, messages: Vec<Message>) -> Result<LlmStream, LlmError>;
    async fn chat_stream(&self, messages: Vec<Message>) -> Result<LlmTextStream, LlmError>;
    async fn fetch_model_context_window(&self) -> Result<Option<u32>, LlmError>;
    async fn user_queue_depth(&self) -> usize;
    async fn system_queue_depth(&self) -> usize;
    fn worker_threads(&self) -> u8;
    async fn user_queue_has_capacity(&self) -> bool;
}
```

- `chat_stream` has a **default implementation** that delegates to
  `chat_stream_with_usage` and filters out `StreamItem::Usage` events.
- Introspection methods (`user_queue_depth`, `system_queue_depth`,
  `worker_threads`, `user_queue_has_capacity`) have **sensible defaults** (zero / true)
  so simple backends do not need to implement them.
- `with_model_override(model)` and `with_temperature_override(temperature)` return
  `Option<Arc<dyn LlmBackend>>` (default `None`). The chat route applies the live
  config snapshot temperature per request via `with_temperature_override` so
  hot-reloaded `llm.temperature` changes take effect without restarting the
  daemon (issue #80).

## Type Aliases

- `LlmStream` — `Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>`
- `LlmTextStream` — `Pin<Box<dyn Stream<Item = Result<String, LlmError>> + Send>>`

These replace the `impl Stream<...> + Send` return types that existed on the
concrete `LlmClient` but cannot be expressed directly in a trait without RPITIT.

## Implementations

### `LlmClient`

`mimir-core/src/llm/client.rs` implements `LlmBackend` for `LlmClient`. Every
method delegates to the existing inherent method with the same name. There is
**no behavioural change**.

### `MockLlmClient`

`mimir-core/src/llm/mock.rs` provides a programmable test double:

- **Builder API**: `MockLlmClient::builder().push_chat(...).user_queue_depth(3).build()`
- **FIFO queues**: chat responses and stream items are queued and returned in order.
- **Call tracking**: `chat_calls()` and `stream_calls()` return the `Vec<Message>`
  vectors that were passed to the mock, enabling assertions on the exact prompt
  content.
- **Empty-queue behaviour**: returns `LlmError::RetryExhausted` so callers do not
  accidentally get a silent success.

`MockLlmClient` is compiled whenever `mimir-core` is built for tests, or when the
`mock-llm` feature is enabled. `mimir-server` enables this feature in its
`[dev-dependencies]` so that server integration tests can use the mock.

## AppState Refactor

`mimir-server/src/state.rs` changed:

```rust
pub llm_client: Arc<LlmClient>;      // old
pub llm_client: Arc<dyn LlmBackend>; // new
```

The `from_config` constructor uses `Arc::new(LlmClient::new(...))` which coerces
to `Arc<dyn LlmBackend>` automatically. All route handlers already called methods
that are now on the trait, so no further changes were needed.
