//! IMAP email connector (Phase 3 C5 / #199), gated by the `email` feature.
//!
//! - `config` — connector configuration types, defaults, and cursor encoding.
//! - `connector` — the [`EmailConnector`] implementation and its two-step
//!   ingestion pipeline.
//! - `envelope` — the message-context surface (dates, sender, recipients,
//!   spam signals) shared by every extraction layer.
//! - `factory` — [`EmailConnectorFactory`] registration.
//! - [`crate::email::imap`] — the IMAP transport (LOGIN / XOAUTH2, IDLE, UID FETCH).
//! - `jsonld`, `llm` — cascade layers 2 and 3 of email fact extraction.

pub(crate) mod envelope;
pub mod imap;
pub(crate) mod jsonld;
pub(crate) mod llm;

mod config;
mod connector;
mod factory;

pub use config::{EmailAuthMethod, EmailConfigDto, EmailSyncMode};
pub use connector::{EmailConnector, EmailConnectorDeps};
pub use factory::EmailConnectorFactory;
pub use llm::EmailExtractionHook;
