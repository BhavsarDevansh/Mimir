//! Shared normalize → insert boundary (Phase 3 F4 / issue #181).
//!
//! Both conversational `remember` extraction and connector ingestion funnel
//! through [`normalize_and_insert`]: a single deterministic Rust pipeline that
//! resolves entities, assigns confidence, runs the sensitivity gate, and inserts
//! facts (inheriting corroboration / supersession / inference from
//! [`crate::queries::fact::insert_fact_in_tx`]). Provenance is supplied once per
//! batch; per-fact content (including the native `raw_reference`) rides on each
//! [`NormalizedFact`].
//!
//! # Confidence
//!
//! Confidence is `confidence::initial(source_type, connector_type)` — the
//! per-source-type / per-connector reliability score. There is **no
//! extraction-method discount**: a structurally-parsed calendar fact and an
//! LLM-extracted email fact of the same source type start at the same score.
//!
//! # Sensitivity
//!
//! The same Rust `AND`-gate as conversational facts: a fact the producer flags
//! `is_sensitive` lands as `pending_confirmation` (Disputed) and surfaces via
//! `kb audit`. Rust can only narrow the flag, never widen it.
//!
//! # Module layout
//!
//! - `types` — batch provenance, normalized fact/location types, outcome
//!   summaries.
//! - `process` — per-fact orchestration (resolve → confidence → gate →
//!   insert).
//! - `overlay` — entity-locations geocode/upsert background worker.
//! - `entities` — entity resolution decision policy.
//! - `events` — event-subsystem overlay derivation.
//! - `corrections` — conversational correction scopes.
//! - `sensitive` — sensitive-fact (Disputed) insertion.

mod corrections;
mod entities;
mod events;
mod overlay;
mod process;
mod sensitive;
mod types;

pub(crate) use overlay::{OverlayJob, start_location_overlay_worker};
pub use process::normalize_and_insert;
pub use types::{ExtractionOutcome, NormalizedFact, NormalizedLocation, PendingFact, Provenance};
