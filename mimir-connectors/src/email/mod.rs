//! IMAP email connector (Phase 3 C5 / #199), gated by the `gmail` feature.
//!
//! - `config` — connector configuration types, defaults, and cursor encoding.
//! - `connector` — the [`EmailConnector`] implementation and its two-step
//!   ingestion pipeline.
//! - `factory` — [`EmailConnectorFactory`] registration.
//! - [`crate::email::imap`] — the IMAP transport (LOGIN / XOAUTH2, IDLE, UID FETCH).
//! - `jsonld`, `llm` — cascade layers 2 and 3 of email fact extraction.

pub mod imap;
pub(crate) mod jsonld;
pub(crate) mod llm;

mod config;
mod connector;
mod factory;

pub use config::{EmailAuthMethod, EmailConfigDto, EmailSyncMode};
pub use connector::EmailConnector;
pub use factory::EmailConnectorFactory;
pub use llm::EmailExtractionHook;
