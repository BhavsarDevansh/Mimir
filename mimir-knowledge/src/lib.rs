#![deny(unsafe_code)]
//! `mimir-knowledge` — SQLite-based knowledge graph for Mimir.
//!
//! Provides entity and fact storage, temporal queries, provenance tracking,
//! and full-text search via SQLite FTS5.

pub mod clock;
pub mod condensation;
pub mod confidence;
pub mod db;
pub mod events;
pub mod extract;
pub mod forget;
pub mod geo;
pub mod inference;
pub mod librarian;
mod llm_tool;
pub mod models;
pub mod normalize;
pub mod obsidian;
pub mod optimization;
pub mod queries;
pub mod retrieval;
pub mod sensitivity;
pub mod tools;

mod graph;

pub(crate) use graph::is_favourite_family_predicate;
pub use graph::{
    CANONICAL_PREDICATES, CONNECTOR_EMITTED_PREDICATES, MULTI_VALUED_PREDICATES,
    is_canonical_predicate_name,
};
pub use graph::{KnowledgeError, KnowledgeGraph};

pub(crate) fn normalize_alias(alias: &str) -> Option<String> {
    let normalized = alias.trim().to_lowercase().replace(' ', "_");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Returns `true` if `name` is already registered as a relationship-type alias.
///
/// Used to keep canonical names from shadowing aliases, which would break
// Re-export knowledge graph tools.
pub use tools::{
    KgExpandCatalogueTool, KgFactsInCatalogueTool, KgQueryTool, KgRelatedTool, KgSearchTool,
    RetrieveContextTool,
};
