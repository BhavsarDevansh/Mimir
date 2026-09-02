//! Knowledge-graph client commands (`kb …`), grouped by concern:
//!
//! - [`optimization`] — optimization status + run-now.
//! - [`query`] — fact query / show / edit / browse / profile / audit.
//! - [`lifecycle`] — forget / restore / trash / pending / confirm / reject.
//! - [`merges`] — entity merge-queue review: list / apply / keep.
//! - [`staging`] — unrecognized-predicate staging review: list / map / reject.
//! - [`categories`] — category list / show / create / delete.
//! - [`obsidian`] — Obsidian export and import (issue #62).

mod categories;
mod lifecycle;
mod merges;
mod obsidian;
mod optimization;
mod query;
mod staging;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
