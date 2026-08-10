//! Knowledge-graph client commands (`kb …`), grouped by concern:
//!
//! - [`optimization`] — optimization status + run-now.
//! - [`query`] — fact query / show / edit / browse / profile / audit.
//! - [`lifecycle`] — forget / restore / trash / pending / confirm / reject.
//! - [`categories`] — category list / show / create / delete.

mod categories;
mod lifecycle;
mod optimization;
mod query;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
