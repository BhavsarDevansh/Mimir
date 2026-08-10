//! Fact forgetting (trash + cascade) for the knowledge graph.
//!
//! Public entry points live in `trash` (single/bulk/connector forget with
//! matching, backup, and audit), and the recursive child cascade lives in
//! `cascade`.

use chrono::{DateTime, Utc};
use std::path::PathBuf;

mod cascade;
#[cfg(test)]
mod tests;
mod trash;

pub(crate) use cascade::{evaluate_children, forget_fact_tx};
pub use trash::{forget_fact, forget_facts, forget_facts_for_connector, hard_delete_expired_trash};

#[derive(Debug, Clone, Default)]
pub struct ForgetFilters {
    pub fact_id: Option<i32>,
    pub predicate: Option<String>,
    pub subject: Option<String>,
    pub entity: Option<String>,
    pub source: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub all: bool,
}

impl ForgetFilters {
    pub fn is_full_reset(&self) -> bool {
        self.all
    }
}

#[derive(Debug, Clone, Default)]
pub struct ForgetOptions {
    pub yes: bool,
    pub confirm_sensitive: bool,
    pub confirmation_phrase: Option<String>,
    pub archive: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ForgetResult {
    pub forgotten_count: u64,
    pub backup_path: Option<PathBuf>,
}
