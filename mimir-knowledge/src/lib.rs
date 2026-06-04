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
pub mod tools;

use clock::{Clock, RealClock};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::inference::rules::contradiction::ContradictionRule;
use crate::inference::rules::threshold::{PREDICATE_REJECTED_ACTION, ThresholdRule};
use crate::inference::rules::transitivity::TransitivityRule;
use crate::inference::{CascadeContext, RuleEngine};
use crate::models::enums::RelationType;
use crate::models::fact::NewFact;
use crate::models::source::{ExtractionMethod, SourceType};

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

    #[error("Duplicate preference detected")]
    DuplicatePreference,
}

/// In-memory cache for predicate name ↔ id lookups.
struct PredicateCache {
    name_to_id: HashMap<String, i16>,
    id_to_name: HashMap<i16, String>,
}

impl PredicateCache {
    fn new() -> Self {
        Self {
            name_to_id: HashMap::new(),
            id_to_name: HashMap::new(),
        }
    }
}

/// The public API for the knowledge graph.
///
/// Holds a SQLite connection pool and a clock for deterministic timestamps
/// in tests.
pub struct KnowledgeGraph {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    predicate_cache: Arc<RwLock<PredicateCache>>,
    rule_engine: RuleEngine,
    pending_confirmations: Arc<RwLock<HashSet<i32>>>,
}

impl std::fmt::Debug for KnowledgeGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KnowledgeGraph")
            .field("pool", &self.pool)
            .field("rule_engine", &"...")
            .finish_non_exhaustive()
    }
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

        let mut engine = RuleEngine::new();
        engine.register(Box::new(TransitivityRule));
        engine.register(Box::new(ContradictionRule));
        engine.register(Box::new(ThresholdRule));

        let pending_ids: Vec<i32> =
            sqlx::query_scalar("SELECT id FROM facts WHERE pending_confirmation = TRUE")
                .fetch_all(&pool)
                .await?;

        let pending: HashSet<i32> = pending_ids.into_iter().collect();

        Ok(Self {
            pool,
            clock,
            predicate_cache: Arc::new(RwLock::new(PredicateCache::new())),
            rule_engine: engine,
            pending_confirmations: Arc::new(RwLock::new(pending)),
        })
    }

    /// Access the rule engine.
    pub(crate) fn rule_engine(&self) -> &RuleEngine {
        &self.rule_engine
    }

    /// Access the pending-confirmation in-memory cache.
    pub fn pending_confirmations(&self) -> &Arc<RwLock<HashSet<i32>>> {
        &self.pending_confirmations
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ------------------------------------------------------------------
    // Predicate registry
    // ------------------------------------------------------------------

    /// Ensure a predicate exists in the database, returning its stable id.
    /// Creates the row silently if missing.
    pub async fn ensure_predicate(&self, name: &str) -> Result<i16, KnowledgeError> {
        {
            let cache = self.predicate_cache.read().await;
            if let Some(&id) = cache.name_to_id.get(name) {
                return Ok(id);
            }
        }

        let row: Option<(i16,)> = sqlx::query_as("SELECT id FROM predicates WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

        let id = match row {
            Some((id,)) => id,
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO predicates (name, description) VALUES (?, ?) ON CONFLICT (name) DO UPDATE SET name = predicates.name RETURNING id",
                )
                .bind(name)
                .bind(format!("Auto-created predicate: {}", name))
                .fetch_one(&self.pool)
                .await?;
                id as i16
            }
        };

        let mut cache = self.predicate_cache.write().await;
        cache.name_to_id.insert(name.to_string(), id);
        cache.id_to_name.insert(id, name.to_string());
        Ok(id)
    }

    /// Look up a predicate id by name without creating it.
    pub async fn get_predicate_id(&self, name: &str) -> Result<Option<i16>, KnowledgeError> {
        {
            let cache = self.predicate_cache.read().await;
            if let Some(&id) = cache.name_to_id.get(name) {
                return Ok(Some(id));
            }
        }

        let row: Option<(i16,)> = sqlx::query_as("SELECT id FROM predicates WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

        if let Some((id,)) = row {
            let mut cache = self.predicate_cache.write().await;
            cache.name_to_id.insert(name.to_string(), id);
            cache.id_to_name.insert(id, name.to_string());
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Reverse lookup: get the predicate name for a given id.
    pub async fn predicate_name(&self, id: i16) -> Option<String> {
        {
            let cache = self.predicate_cache.read().await;
            if let Some(name) = cache.id_to_name.get(&id) {
                return Some(name.clone());
            }
        }

        let row: Option<(String,)> =
            match sqlx::query_as("SELECT name FROM predicates WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("predicate_name lookup failed for id {}: {}", id, e);
                    return None;
                }
            };

        if let Some((ref name,)) = row {
            let mut cache = self.predicate_cache.write().await;
            cache.name_to_id.insert(name.clone(), id);
            cache.id_to_name.insert(id, name.clone());
        }

        row.map(|r| r.0)
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

    /// Insert a new fact, running inference rules and cascading inferred facts.
    pub async fn insert_fact(
        &self,
        new_fact: models::fact::NewFact,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        self.insert_fact_internal(new_fact, &mut CascadeContext::new())
            .await
    }

    pub(crate) fn insert_fact_internal<'a>(
        &'a self,
        mut new_fact: NewFact,
        ctx: &'a mut CascadeContext,
    ) -> Pin<Box<dyn Future<Output = Result<models::fact::Fact, KnowledgeError>> + Send + 'a>> {
        Box::pin(async move {
            // Resolve predicate name to id.
            let predicate_id = self.ensure_predicate(&new_fact.predicate).await?;

            // Cycle detection: skip duplicate triples in the same cascade.
            if ctx.contains(
                new_fact.subject_id,
                predicate_id,
                new_fact.object_id,
                new_fact.object_literal.as_deref(),
            ) {
                return Err(KnowledgeError::Validation(
                    "inference cycle detected".to_string(),
                ));
            }
            ctx.insert(
                new_fact.subject_id,
                predicate_id,
                new_fact.object_id,
                new_fact.object_literal.clone(),
            );

            // Determine confidence.
            let confidence = if let Some(conf) = new_fact.confidence {
                conf
            } else if new_fact.inferred {
                confidence::initial(SourceType::Inference, None)
            } else if let Some(ct) = new_fact.connector_type {
                if new_fact.connector_id.is_none()
                    || new_fact.raw_reference.is_none()
                    || new_fact.extraction_method.is_none()
                {
                    return Err(KnowledgeError::Validation(
                        "Connector provenance requires connector_id, raw_reference, and extraction_method"
                            .to_string(),
                    ));
                }
                let db_score: Option<f32> = sqlx::query_scalar(
                    "SELECT score FROM connector_reliability WHERE connector_type_id = ?",
                )
                .bind(ct as i16)
                .fetch_optional(&self.pool)
                .await?;
                db_score.unwrap_or_else(|| confidence::default_connector_score(ct))
            } else {
                confidence::initial(new_fact.source_type, None)
            };

            // Ensure inferred facts use Inference source type.
            if new_fact.inferred {
                new_fact.source_type = SourceType::Inference;
                new_fact.extraction_method = Some(ExtractionMethod::InferenceRule);
            }

            let mut tx = self.pool.begin().await?;

            let fact = queries::fact::insert_fact_in_tx(
                &mut tx,
                &new_fact,
                predicate_id,
                confidence,
                self.now(),
            )
            .await?;

            // Write InferredFrom dependencies for inferred facts.
            for parent_id in &new_fact.parent_fact_ids {
                sqlx::query(
                    "INSERT INTO fact_dependencies \
                     (parent_fact_id, child_fact_id, relation_type_id, is_positive) \
                     VALUES (?, ?, ?, ?)",
                )
                .bind(*parent_id)
                .bind(fact.id)
                .bind(RelationType::InferredFrom as i16)
                .bind(true)
                .execute(&mut *tx)
                .await?;
            }

            // Side-effect: check rejected_action thresholds (decoupled from InferenceRule trait).
            let threshold_input = if new_fact.predicate == PREDICATE_REJECTED_ACTION {
                ThresholdRule::check_threshold(&fact, self, &mut tx).await?
            } else {
                None
            };

            tx.commit().await?;

            if let Some(input) = threshold_input {
                if let Err(e) = self.upsert_preference(input).await {
                    tracing::warn!("threshold preference upsert failed: {}", e);
                }
            }

            // Run inference rules and cascade inferred facts.
            match self.rule_engine.evaluate_insert(&fact, self, ctx).await {
                Ok(inferred) => {
                    for mut inferred_fact in inferred {
                        inferred_fact.inferred = true;
                        inferred_fact.source_type = SourceType::Inference;
                        inferred_fact.extraction_method = Some(ExtractionMethod::InferenceRule);
                        if let Err(e) = self.insert_fact_internal(inferred_fact, ctx).await {
                            tracing::warn!("inference cascade failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("inference evaluation failed: {}", e);
                }
            }

            Ok(fact)
        })
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
        predicate_id: i16,
        limit: i64,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_by_predicate(&self.pool, predicate_id, limit).await
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
        predicate_id: i16,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_active_facts_at(&self.pool, subject_id, predicate_id, at).await
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

    /// Bulk forget facts with filters and safeguards.
    pub async fn forget_facts(
        &self,
        filters: forget::ForgetFilters,
        opts: forget::ForgetOptions,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<forget::ForgetResult, KnowledgeError> {
        forget::forget_facts(&self.pool, filters, opts, changed_by, self.now()).await
    }

    /// Restore a single fact from trash.
    pub async fn restore_fact(
        &self,
        trash_id: i32,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        queries::trash::restore_fact(&self.pool, trash_id, changed_by, self.now()).await
    }

    /// Restore all facts from trash.
    pub async fn restore_all(
        &self,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::trash::restore_all(&self.pool, changed_by, self.now()).await
    }

    /// List trash contents.
    pub async fn list_trash(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<models::trash::TrashListItem>, KnowledgeError> {
        queries::trash::list_trash(&self.pool, limit, offset).await
    }

    /// Empty the trash.
    pub async fn empty_trash(&self) -> Result<u64, KnowledgeError> {
        queries::trash::empty_trash(&self.pool).await
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

    // ------------------------------------------------------------------
    // Preference delegates
    // ------------------------------------------------------------------

    /// Insert a new preference with context and sources.
    pub async fn insert_preference(
        &self,
        input: models::preference::UpsertPreferenceInput,
    ) -> Result<models::preference::Preference, KnowledgeError> {
        queries::preference::insert_preference(&self.pool, &input, self.now()).await
    }

    /// Upsert a preference with conflict resolution.
    pub async fn upsert_preference(
        &self,
        input: models::preference::UpsertPreferenceInput,
    ) -> Result<
        (
            models::preference::Preference,
            models::preference::UpsertAction,
        ),
        KnowledgeError,
    > {
        queries::preference::upsert_preference(&self.pool, &input, self.now()).await
    }

    /// Contextual preference lookup.
    pub async fn get_preference(
        &self,
        entity_id: Option<i32>,
        key: &str,
        query_context: &[(String, String)],
    ) -> Result<Option<models::preference::Preference>, KnowledgeError> {
        queries::preference::get_preference(&self.pool, entity_id, key, query_context).await
    }

    /// Get preference by ID.
    pub async fn get_preference_by_id(
        &self,
        id: i32,
    ) -> Result<Option<models::preference::Preference>, KnowledgeError> {
        queries::preference::get_preference_by_id(&self.pool, id).await
    }

    /// Get contexts for a preference.
    pub async fn get_preference_contexts(
        &self,
        preference_id: i32,
    ) -> Result<Vec<models::preference::PreferenceContext>, KnowledgeError> {
        queries::preference::get_contexts_for_preference(&self.pool, preference_id).await
    }

    /// Get sources for a preference.
    pub async fn get_preference_sources(
        &self,
        preference_id: i32,
    ) -> Result<Vec<models::preference::PreferenceSource>, KnowledgeError> {
        queries::preference::get_sources_for_preference(&self.pool, preference_id).await
    }

    /// Get audit log for a preference.
    pub async fn get_preference_audit_log(
        &self,
        preference_id: i32,
    ) -> Result<Vec<models::preference::PreferenceAuditLogEntry>, KnowledgeError> {
        queries::preference::get_preference_audit_log(&self.pool, preference_id).await
    }

    // ------------------------------------------------------------------
    // Fact extraction pipeline delegates
    // ------------------------------------------------------------------

    /// Extract facts from a user message via LLM, validate, and insert.
    pub async fn extract_facts(
        &self,
        llm: &Arc<dyn mimir_core::llm::backend::LlmBackend>,
        user_message: &str,
    ) -> Result<extract::ExtractionOutcome, KnowledgeError> {
        extract::extract_facts(self, llm, user_message).await
    }

    /// Confirm a pending sensitive fact: flip to Active with confidence 1.0.
    pub async fn confirm_fact(&self, fact_id: i32) -> Result<models::fact::Fact, KnowledgeError> {
        extract::confirm_fact(self, fact_id).await
    }

    /// Reject a pending sensitive fact: hard-delete with audit trail.
    pub async fn reject_fact(&self, fact_id: i32) -> Result<(), KnowledgeError> {
        extract::reject_fact(self, fact_id).await
    }
}

// Re-export knowledge graph tools.
pub use tools::{KgQueryTool, KgRelatedTool, KgSearchTool};
