//! Entity queries: CRUD, search, and dedup.
//!
//! Module layout by concern:
//!
//! - `crud` — entity CRUD and name/alias search.
//! - `predicates` — predicate-string validation.
//! - `dedup` — duplicate detection and merging.
//! - `merge_queue` — entity merge-queue review surface (list / apply / keep).
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
mod merge_queue;
mod names;
mod predicates;

pub use crud::{
    AliasSearchResult, MatchKind, add_alias, create_entity, delete_entity, get_by_id, get_by_name,
    get_by_name_typed, get_exact_name, list_all, remove_alias, search, update_entity,
};
pub(crate) use dedup::ordered_pair;
pub use dedup::{
    auto_merge_pair, enqueue_semantic_dedup, find_exact_duplicates, find_overlapping_aliases,
    find_semantic_candidates, flag_overlapping_aliases,
};
pub use merge_queue::{EntityMergeQueueItem, apply_merge, keep_merge, list_pending_merges};
pub use names::get_entity_names;
pub use predicates::validate_predicate;
pub(crate) use predicates::validate_predicate_in_tx;
