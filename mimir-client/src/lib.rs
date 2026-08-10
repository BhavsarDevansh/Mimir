#![deny(unsafe_code)]
//! Thin HTTP client for the Mimir daemon.
//!
//! - `transport` — client construction and low-level HTTP helpers.
//! - `chat` — chat (streaming and non-streaming) endpoints.
//! - `kb` — knowledge-graph endpoints (facts, browse, categories, pending).
//! - `connectors` — connector management endpoints.
//! - `system` — status, memory, sessions, and shutdown endpoints.
//! - `sse` — client-side SSE stream parsing.
//! - `error` — client error types.

mod chat;
mod connectors;
mod error;
mod kb;
mod sse;
mod system;
mod transport;

pub use error::ClientError;
pub use sse::{parse_sse_event_pub, parse_sse_stream};

/// A thin HTTP client for the Mimir daemon.
#[derive(Debug, Clone)]
pub struct MimirClient {
    pub(crate) base_url: String,
    pub(crate) client: reqwest::Client,
}
