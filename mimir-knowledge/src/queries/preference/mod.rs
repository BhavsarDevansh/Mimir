//! Preference CRUD, contextual lookup, conflict resolution, and audit logging.
//!
//! Write paths (insert/upsert/conflict resolution/audit) live in `write`;
//! read paths (lookup by entity, context, source, audit trail) in `read`.

mod read;
mod write;

pub use read::{
    get_contexts_for_preference, get_preference, get_preference_audit_log, get_preference_by_id,
    get_sources_for_preference,
};
pub use write::{insert_preference, upsert_preference};
