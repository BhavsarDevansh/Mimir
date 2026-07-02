#![deny(unsafe_code)]
//! `mimir-connectors` — service ingestion framework for Mimir.
//!
//! Connectors are background sync workers that fetch data from external
//! services (email, calendar, photos, …), normalize it, and insert it into the
//! knowledge graph through the *existing* [`mimir_knowledge`] fact pipeline.
//! They are not a parallel track: every connector funnels through the same
//! `normalize_and_insert` boundary as conversational `remember` calls.
//!
//! # Database access boundary
//!
//! DB access is mediated **exclusively** by the
//! [`mimir_knowledge::KnowledgeGraph`] facade. This crate never holds a
//! `sqlx` pool handle directly, and does not depend on `sqlx` itself.
//!
//! # Crate layout
//!
//! - [`connector`] — runtime [`Connector`] trait (stub; filled by F6).
//! - [`registry`] — [`ConnectorRegistry`] and multi-backend factory dispatch
//!   (stub; filled by F7).
//! - [`mock`] — always-compiled mock connector test harness (stub; filled by
//!   F13).
//!
//! # Feature flags
//!
//! `photos`, `calendar`, and `gmail` gate the per-type backends, which are
//! added in later Phase 3 issues (C1–C7). The framework core and the mock
//! connector are **always built**, so `--no-default-features` still compiles a
//! working framework + mock harness.

pub mod connector;
pub mod mock;
pub mod registry;

pub use connector::Connector;
pub use mock::MockConnector;
pub use registry::ConnectorRegistry;
