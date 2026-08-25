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
//! Confidence is the per-source-type / per-connector reliability score:
//! connector provenance reads the `connector_reliability` table via
//! `confidence::connector_reliability` (falling back to the seeded defaults
//! when no row exists), while other source types use
//! `confidence::initial` directly. There is **no extraction-method discount**:
//! a structurally-parsed calendar fact and an LLM-extracted email fact of the
//! same source type start at the same score.
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

pub(crate) use entities::{pick_resolution, resolve_or_create};
pub(crate) use overlay::{
    LocationOverlayApply, OverlayJob, apply_location_overlay, start_location_overlay_worker,
};
pub use process::normalize_and_insert;
pub use types::{ExtractionOutcome, NormalizedFact, NormalizedLocation, PendingFact, Provenance};
