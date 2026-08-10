//! Entity queries: CRUD, search, locations, nearby, and dedup.
//!
//! Module layout by concern:
//!
//! - `crud` — entity CRUD and name/alias search.
//! - `predicates` — predicate-string validation.
//! - `locations` — entity-location persistence.
//! - `nearby` — geographic near-by search.
//! - `dedup` — duplicate detection and merging.
//! - `names` — bulk entity-name lookups.

mod crud;
mod dedup;
mod locations;
mod names;
mod nearby;
mod predicates;

pub use crud::{
    AliasSearchResult, MatchKind, add_alias, create_entity, delete_entity, get_by_id, get_by_name,
    get_by_name_typed, remove_alias, search, update_entity,
};
pub use dedup::{
    auto_merge_pair, enqueue_semantic_dedup, find_exact_duplicates, find_overlapping_aliases,
    flag_overlapping_aliases,
};
pub use locations::{
    close_prior_open_locations_in_tx, ensure_place_coordinates, get_locations, insert_location,
    insert_location_in_tx, update_location, upsert_location,
};
pub use names::get_entity_names;
pub use nearby::find_nearby;
pub use predicates::validate_predicate;
