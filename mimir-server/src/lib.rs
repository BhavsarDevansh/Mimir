#![deny(unsafe_code)]
//! HTTP server library for the Mimir daemon.
//!
//! - [`app`] — axum router assembly, the bearer-token auth middleware, and the loopback guard middleware.
//! - [`shutdown`] — shutdown trigger sources, OS-signal handling, and the
//!   bounded graceful-drain lifecycle.
//! - [`server`] — daemon startup: state initialisation, background tasks, and
//!   serving.
//! - [`routes`], [`state`], [`types`], [`error`] — route handlers, shared
//!   application state, wire-type re-exports, and error helpers.

pub mod app;
pub mod error;
pub mod routes;
pub mod server;
pub mod shutdown;
pub mod state;
pub mod types;

#[cfg(test)]
mod test_utils;

pub use app::build_app;
pub use server::{start_server, start_server_with_llm, start_server_with_llm_and_listener};
pub(crate) use shutdown::ShutdownSource;
