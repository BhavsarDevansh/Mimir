//! `mimir-knowledge` — SQLite-based knowledge graph for Mimir.
//!
//! Provides entity and fact storage, temporal queries, provenance tracking,
//! and full-text search via SQLite FTS5.

pub mod clock;
pub mod confidence;
pub mod db;
pub mod extract;
pub mod forget;
pub mod inference;
pub mod models;
pub mod optimization;
pub mod queries;

use clock::{Clock, RealClock};
use sqlx::SqlitePool;
use std::path::Path;
use std::sync::Arc;

/// Errors that can occur during knowledge graph initialization or operation.
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

    #[error("Invalid predicate id {0}")]
    InvalidPredicate(i16),

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
}

/// The public API for the knowledge graph.
///
/// Holds a SQLite connection pool and a clock for deterministic timestamps
/// in tests.
pub struct KnowledgeGraph {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
}

impl KnowledgeGraph {
    /// Initialise the knowledge graph: ensure parent directories exist, open
    /// the SQLite pool (WAL + foreign keys), and run pending migrations.
    pub async fn init(db_path: &Path) -> Result<Self, KnowledgeError> {
        Self::init_with_clock(db_path, Arc::new(RealClock)).await
    }

    /// Initialise with a custom clock (used in tests for determinism).
    pub async fn init_with_clock(
        db_path: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, KnowledgeError> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let pool = db::create_pool(db_path).await?;
        sqlx::migrate!("src/db/migrations").run(&pool).await?;

        Ok(Self { pool, clock })
    }

    /// Access the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Current timestamp according to the configured clock.
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }

    // ------------------------------------------------------------------
    // Entity CRUD delegates
    // ------------------------------------------------------------------

    /// Create an entity (returns existing on exact duplicate).
    pub async fn create_entity(
        &self,
        name: &str,
        entity_type: models::entity::EntityType,
        aliases: &[&str],
    ) -> Result<models::entity::Entity, KnowledgeError> {
        queries::entity::create_entity(&self.pool, name, entity_type, aliases).await
    }

    /// Get entity by ID.
    pub async fn get_entity(
        &self,
        id: i32,
    ) -> Result<Option<models::entity::Entity>, KnowledgeError> {
        queries::entity::get_by_id(&self.pool, id).await
    }

    /// Update entity name and type.
    pub async fn update_entity(
        &self,
        id: i32,
        name: &str,
        entity_type: models::entity::EntityType,
    ) -> Result<models::entity::Entity, KnowledgeError> {
        queries::entity::update_entity(&self.pool, id, name, entity_type as i16).await
    }

    /// Delete entity (rejected if referenced by facts).
    pub async fn delete_entity(&self, id: i32) -> Result<(), KnowledgeError> {
        queries::entity::delete_entity(&self.pool, id).await
    }

    /// Search entities by name/alias.
    pub async fn search_entities(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<queries::entity::AliasSearchResult>, KnowledgeError> {
        queries::entity::search(&self.pool, query, limit).await
    }

    /// Add an alias to an entity.
    pub async fn add_alias(&self, entity_id: i32, alias: &str) -> Result<(), KnowledgeError> {
        queries::entity::add_alias(&self.pool, entity_id, alias).await
    }

    /// Remove an alias from an entity.
    pub async fn remove_alias(&self, entity_id: i32, alias: &str) -> Result<(), KnowledgeError> {
        queries::entity::remove_alias(&self.pool, entity_id, alias).await
    }

    // ------------------------------------------------------------------
    // Entity dates delegates
    // ------------------------------------------------------------------

    /// Insert a date for an entity.
    pub async fn insert_entity_date(
        &self,
        entity_id: i32,
        date_type: models::enums::EntityDateType,
        date_value: &str,
        recurrence: models::enums::RecurrenceType,
        label: Option<&str>,
        confidence: f32,
    ) -> Result<models::entity_date::EntityDate, KnowledgeError> {
        queries::entity::insert_entity_date(
            &self.pool,
            entity_id,
            date_type as i16,
            date_value,
            recurrence as i16,
            label,
            confidence,
        )
        .await
    }

    /// List all dates for an entity.
    pub async fn get_entity_dates(
        &self,
        entity_id: i32,
    ) -> Result<Vec<models::entity_date::EntityDate>, KnowledgeError> {
        queries::entity::get_dates_for_entity(&self.pool, entity_id).await
    }

    /// Return dates that recur within the next `days_ahead` days.
    pub async fn get_upcoming_dates(
        &self,
        entity_id: i32,
        days_ahead: i64,
    ) -> Result<Vec<models::entity_date::EntityDate>, KnowledgeError> {
        queries::entity::get_upcoming_dates(&self.pool, entity_id, days_ahead, self.now()).await
    }

    // ------------------------------------------------------------------
    // Entity locations delegates (stubs)
    // ------------------------------------------------------------------

    /// Insert a location for an entity.
    pub async fn insert_location(
        &self,
        entity_id: i32,
        location_type: models::enums::LocationType,
        address: Option<&str>,
        latitude: Option<f64>,
        longitude: Option<f64>,
        timezone: Option<&str>,
    ) -> Result<models::entity_location::EntityLocation, KnowledgeError> {
        queries::entity::insert_location(
            &self.pool,
            entity_id,
            location_type as i16,
            address,
            latitude,
            longitude,
            timezone,
        )
        .await
    }

    /// Get locations for an entity.
    pub async fn get_locations(
        &self,
        entity_id: i32,
    ) -> Result<Vec<models::entity_location::EntityLocation>, KnowledgeError> {
        queries::entity::get_locations(&self.pool, entity_id).await
    }

    // ------------------------------------------------------------------
    // Fact CRUD delegates
    // ------------------------------------------------------------------

    /// Insert a new fact with temporal overlap handling and provenance.
    pub async fn insert_fact(
        &self,
        new_fact: models::fact::NewFact,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        queries::fact::insert_fact(&self.pool, &new_fact, self.now()).await
    }

    /// Get a fact by ID.
    pub async fn get_fact(&self, id: i32) -> Result<Option<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_by_id(&self.pool, id).await
    }

    /// List facts for a subject entity.
    pub async fn get_facts_by_subject(
        &self,
        subject_id: i32,
        limit: i64,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_by_subject(&self.pool, subject_id, limit).await
    }

    /// List facts for a predicate.
    pub async fn get_facts_by_predicate(
        &self,
        predicate: models::enums::Predicate,
        limit: i64,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_by_predicate(&self.pool, predicate as i16, limit).await
    }

    /// List facts for an object entity.
    pub async fn get_facts_by_object(
        &self,
        object_id: i32,
        limit: i64,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_by_object(&self.pool, object_id, limit).await
    }

    /// Return facts active at a specific point in time.
    pub async fn get_active_facts_at(
        &self,
        subject_id: i32,
        predicate: models::enums::Predicate,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_active_facts_at(&self.pool, subject_id, predicate as i16, at).await
    }

    /// Update a fact's valid-until timestamp.
    pub async fn update_fact_valid_until(
        &self,
        id: i32,
        valid_until: Option<chrono::DateTime<chrono::Utc>>,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        queries::fact::update_valid_until(&self.pool, id, valid_until, self.now(), changed_by).await
    }

    /// Update a fact's lifecycle status.
    pub async fn update_fact_status(
        &self,
        id: i32,
        status: models::fact::FactStatus,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        queries::fact::set_status(&self.pool, id, status, self.now(), changed_by).await
    }

    /// Soft-delete a fact to trash, cascading to inferred children.
    pub async fn forget_fact(
        &self,
        id: i32,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<(), KnowledgeError> {
        forget::forget_fact(&self.pool, id, changed_by, self.now()).await
    }

    /// Retrieve audit log entries for a fact.
    pub async fn get_audit_log(
        &self,
        fact_id: i32,
    ) -> Result<Vec<models::audit_log::AuditLogEntry>, KnowledgeError> {
        queries::fact::get_audit_log(&self.pool, fact_id).await
    }

    // ------------------------------------------------------------------
    // Source CRUD delegates
    // ------------------------------------------------------------------

    /// Retrieve all sources linked to a fact.
    pub async fn get_sources_for_fact(
        &self,
        fact_id: i32,
    ) -> Result<Vec<models::source::Source>, KnowledgeError> {
        queries::source::get_sources_for_fact(&self.pool, fact_id).await
    }

    /// Add a new source to an existing fact and write a `source_added` audit entry.
    pub async fn add_source_to_fact(
        &self,
        request: queries::source::AddSourceRequest,
    ) -> Result<models::source::Source, KnowledgeError> {
        let input = queries::source::SourceInput {
            fact_id: request.fact_id,
            source_type_id: request.source_type as i16,
            connector_id: request.connector_id,
            connector_type_id: request.connector_type.map(|c| c as i16),
            raw_reference: request.raw_reference,
            extraction_method_id: request.extraction_method.map(|e| e as i16),
        };
        queries::source::add_source_to_fact(&self.pool, &input, self.now(), request.changed_by)
            .await
    }

    // ------------------------------------------------------------------
    // Audit log delegates
    // ------------------------------------------------------------------

    /// Query the audit log with optional filters.
    pub async fn query_audit_log(
        &self,
        filter: queries::audit::AuditLogFilter,
    ) -> Result<Vec<queries::audit::AuditLogRow>, KnowledgeError> {
        queries::audit::query_audit_log(&self.pool, &filter).await
    }

    // ------------------------------------------------------------------
    // Connector reliability
    // ------------------------------------------------------------------

    /// Adjust a connector's reliability score.
    pub async fn adjust_connector_reliability(
        &self,
        connector: models::enums::ConnectorType,
        delta: f32,
    ) -> Result<(), KnowledgeError> {
        confidence::adjust_connector_reliability(&self.pool, connector, delta).await
    }

    /// Read a connector's current reliability score.
    pub async fn connector_reliability(
        &self,
        connector: models::enums::ConnectorType,
    ) -> Result<f32, KnowledgeError> {
        confidence::connector_reliability(&self.pool, connector).await
    }
}
