//! Fact CRUD, temporal queries, overlap logic, and audit logging.
//!
//! Module layout by concern:
//!
//! - `insert` — the insert pipeline: temporal-overlap supersession,
//!   corroboration (#79), provenance, and audit.
//! - `corroboration` — the corroboration branch of the insert pipeline.
//! - `conflict` — overlap-conflict resolution (supersede / dispute).
//! - `update` — field updates with per-field audit entries.
//! - `read` — by-id / by-subject / by-predicate / by-object / point-in-time
//!   reads.
//! - `status` — temporal and status transitions + range-overlap predicate.
//! - `browse` — enriched reads: audit log, subject-filtered lists,
//!   relationship subtrees.
//! - `pending` — pending-confirmation fact listing.

mod browse;
mod conflict;
mod corroboration;
mod insert;
mod pending;
mod read;
mod status;
mod update;

pub use crate::MULTI_VALUED_PREDICATES;
pub use browse::{
    FactWithObjectName, FactWithSources, count_facts_by_relationship_subtree,
    count_facts_by_subject_filtered, get_audit_log, get_facts_by_relationship_subtree,
    get_facts_by_subject_filtered,
};
pub use insert::{insert_fact, insert_fact_in_tx};
pub use pending::{PendingFactRow, list_pending};
pub use read::{get_active_facts_at, get_by_id, get_by_object, get_by_predicate, get_by_subject};
pub use status::{ranges_overlap, set_status, set_status_tx, update_valid_until};
pub use update::update_fact;
