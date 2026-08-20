//! The [`KnowledgeGraph`] facade: SQLite-backed entity/fact storage with
//! temporal queries, provenance, and FTS5 search.
//!
//! The facade's methods are grouped into per-concern impl modules:
//!
//! - [`lifecycle`] — initialisation, geocoder injection, condensation state.
//! - [`predicates`], [`relationships`] — predicate registry and relationship DAG.
//! - [`centrality`], [`memory`] — centrality cache and memory-schema delegates.
//! - [`entities`], [`locations`], [`events`] — entity/event CRUD delegates.
//! - [`facts`], [`sources`], [`audit`] — fact/source/audit CRUD delegates.
//! - [`connectors`], [`preferences`] — connector registry and preference delegates.
//! - [`extraction`], [`categories`] — extraction pipeline and category delegates.

mod audit;
mod categories;
mod centrality;
mod connectors;
mod entities;
mod events;
mod extraction;
mod facts;
mod lifecycle;
mod locations;
mod memory;
mod predicates;
mod preferences;
mod relationships;
mod sources;

pub(crate) use predicates::is_favourite_family_predicate;
pub use predicates::{CANONICAL_PREDICATES, MULTI_VALUED_PREDICATES};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mimir_core::geocoder::Geocoder;
use sqlx::SqlitePool;
use tokio::sync::{Mutex, Notify, RwLock, mpsc};

use crate::clock::Clock;
use crate::inference::RuleEngine;
use crate::normalize::OverlayJob;
use crate::normalize_alias;

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    #[error("Database pool creation failed: {0}")]
    Pool(#[from] sqlx::Error),

    #[error("I/O error preparing database path: {0}")]
    Io(#[from] std::io::Error),

    #[error("Migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("Entity has {0} fact(s) and cannot be deleted")]
    EntityHasFacts(i64),

    #[error("Relationship type {0} does not allow this subject/object entity-type combination")]
    InvalidRelationshipConstraint(i16),

    #[error("Entity {0} not found")]
    EntityNotFound(i32),

    #[error("Duplicate entity detected")]
    DuplicateEntity,

    #[error("Fact {0} not found")]
    FactNotFound(i32),

    #[error("Temporal conflict: {0}")]
    TemporalConflict(String),

    #[error("Immutable field cannot be updated")]
    ImmutableField,

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not yet implemented")]
    NotYetImplemented,

    #[error("Duplicate preference detected")]
    DuplicatePreference,

    #[error("Category {0} not found")]
    CategoryNotFound(i32),

    #[error("Connector {0} not found")]
    ConnectorNotFound(i32),

    #[error("Connector slug `{0}` already exists with a different connector type")]
    ConnectorTypeMismatch(String),

    #[error("connector slug `{0}` already exists")]
    ConnectorSlugConflict(String),

    #[error("Relationship type hierarchy cycle detected")]
    RelationshipTypeCycle,
}

/// In-memory cache for relationship_type name ↔ id lookups.
struct RelationshipTypeCache {
    name_to_id: HashMap<String, i16>,
    id_to_name: HashMap<i16, String>,
    alias_to_id: HashMap<String, i16>,
}

impl RelationshipTypeCache {
    fn new() -> Self {
        Self {
            name_to_id: HashMap::new(),
            id_to_name: HashMap::new(),
            alias_to_id: HashMap::new(),
        }
    }
}

/// Normalise an English alias before storage or lookup.
///
/// Returns `None` if the alias is empty or whitespace-only after normalisation.
/// alias-to-canonical resolution.
async fn canonical_name_conflicts_with_alias<'a, E>(
    executor: E,
    name: &str,
) -> Result<bool, KnowledgeError>
where
    E: sqlx::Executor<'a, Database = sqlx::Sqlite>,
{
    let Some(normalized) = normalize_alias(name) else {
        return Ok(false);
    };
    let row: Option<(i16,)> = sqlx::query_as(
        "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
    )
    .bind(&normalized)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

/// Returns `true` if `alias` normalises to an existing relationship-type name.
///
/// Used to keep aliases from shadowing canonical names.
async fn alias_conflicts_with_canonical_name<'a, E>(
    executor: E,
    alias: &str,
) -> Result<bool, KnowledgeError>
where
    E: sqlx::Executor<'a, Database = sqlx::Sqlite>,
{
    let Some(normalized) = normalize_alias(alias) else {
        return Ok(false);
    };
    let row: Option<(i16,)> = sqlx::query_as("SELECT id FROM relationship_types WHERE name = ?")
        .bind(&normalized)
        .fetch_optional(executor)
        .await?;
    Ok(row.is_some())
}

/// The public API for the knowledge graph.
///
/// Holds a SQLite connection pool and a clock for deterministic timestamps
/// in tests.
pub struct KnowledgeGraph {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    relationship_type_cache: Arc<RwLock<RelationshipTypeCache>>,
    rule_engine: RuleEngine,
    pending_confirmations: Arc<RwLock<HashSet<i32>>>,
    centrality_cache: Arc<RwLock<HashMap<i32, f32>>>,
    condensation_dirty: AtomicBool,
    condensation_notify: Arc<Notify>,
    /// Pluggable geocoder used by the entity-locations write path (Phase 3
    /// S3 / #193) to fill the missing half of a location (address -> coords
    /// or coords -> address). `None` until the server injects a backend
    /// (the Nominatim default lives in `mimir-connectors`); a location fact
    /// processed with no geocoder is stored with whatever data it carries.
    geocoder: Option<Arc<dyn Geocoder>>,
    /// Sender for the location-overlay background worker (Phase 3 S3 / #193).
    /// Location overlays are enqueued here instead of awaited inline so a
    /// connector batch of location facts is not gated on the geocoder's
    /// rate limit. The worker processes jobs in FIFO order, preserving
    /// move/supersession semantics; see [`crate::normalize::OverlayJob`].
    location_overlay_tx: mpsc::UnboundedSender<OverlayJob>,
    /// Serialises all knowledge-graph *write* transactions so the background
    /// location-overlay worker cannot commit a write in the middle of an
    /// ingestion caller's read-then-write transaction (issue #236). In WAL
    /// mode a deferred transaction that reads a snapshot, then has another
    /// connection commit, then writes returns `SQLITE_BUSY` immediately —
    /// `busy_timeout` cannot wait it out because the snapshot is stale. The
    /// lock is held only across write transactions; reads stay concurrent.
    write_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for KnowledgeGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KnowledgeGraph")
            .field("pool", &self.pool)
            .field("rule_engine", &"...")
            .finish_non_exhaustive()
    }
}
