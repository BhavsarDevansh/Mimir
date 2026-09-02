//! Knowledge-base HTTP route handlers.
//!
//! Module layout by concern:
//!
//! - `optimization` — KB optimization status / run-now.
//! - `query` — fact queries.
//! - `detail` — fact show + edit.
//! - `browse` — browse, profile, and audit.
//! - `forget` — fact forgetting (soft delete).
//! - `trash` — trash list / restore / empty.
//! - `pending` — pending-confirmation list / confirm / reject.
//! - `merges` — entity merge-queue review: list / apply / keep.
//! - `staging` — unrecognized-predicate staging review: list / map / reject.
//! - `export` / `import` — Obsidian export and import (issue #62).
//! - `params` — query-string parameter structs.
//! - `helpers` — shared parsing and name-resolution helpers.

mod browse;
mod detail;
mod export;
mod forget;
mod heatmap;
mod helpers;
mod import;
mod merges;
mod optimization;
mod params;
mod pending;
mod query;
mod staging;
mod trash;

pub use browse::{kb_audit_handler, kb_browse_handler, kb_profile_handler};
pub use detail::{kb_edit_handler, kb_show_handler};
pub use export::kb_export_handler;
pub use forget::kb_forget_handler;
pub use heatmap::kb_heatmap_handler;
pub use import::kb_import_handler;
pub use merges::{kb_merge_apply_handler, kb_merge_keep_handler, kb_merges_handler};
pub use optimization::{kb_optimization_run_now_handler, kb_optimization_status_handler};
pub use params::{
    AuditQueryParams, BrowseQueryParams, ProfileQueryParams, QueryParams, TrashQueryParams,
};
pub use pending::{kb_confirm_fact_handler, kb_pending_handler, kb_reject_fact_handler};
pub use query::kb_query_handler;
pub use staging::{kb_staged_list_handler, kb_staged_map_handler, kb_staged_reject_handler};
pub use trash::{kb_trash_empty_handler, kb_trash_list_handler, kb_trash_restore_handler};
