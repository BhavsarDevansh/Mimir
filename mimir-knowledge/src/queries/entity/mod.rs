//! Entity queries: CRUD, search, and dedup.
//!
//! Module layout by concern:
//!
//! - `crud` — entity CRUD and name/alias search.
//! - `predicates` — predicate-string validation.
//! - `dedup` — duplicate detection and merging.
//! - `names` — bulk entity-name lookups.
//!
//! Location queries previously re-exported from this module moved to
//! [`crate::queries::location`] (0.102.0, issue #231). Import them from
//! `queries::location` instead: `insert_location`, `upsert_location`,
//! `get_locations`, `update_location`, `close_prior_open_locations_in_tx`,
//! `ensure_place_coordinates`, `insert_location_in_tx`, the pending-meta
//! helpers and `PendingLocationMeta`, and `queries::location::find_nearby`
//! for the nearby search.

mod crud;
mod dedup;
mod names;
mod predicates;

pub use crud::{
    AliasSearchResult, MatchKind, add_alias, create_entity, delete_entity, get_by_id, get_by_name,
    get_by_name_typed, remove_alias, search, update_entity,
};
pub use dedup::{
    auto_merge_pair, enqueue_semantic_dedup, find_exact_duplicates, find_overlapping_aliases,
    flag_overlapping_aliases,
};
pub use names::get_entity_names;
pub use predicates::validate_predicate;
