use crate::graph::KnowledgeGraph;
use crate::*;

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

use crate::inference::CascadeContext;
use crate::inference::rules::threshold::{RELATIONSHIP_TYPE_REJECTED_ACTION, ThresholdRule};
use crate::models::enums::{ConnectorType, RelationType};
use crate::models::fact::NewFact;
use crate::models::source::{ExtractionMethod, SourceType};

impl KnowledgeGraph {
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

    /// Insert multiple facts atomically in a single transaction.
    /// Skips rule-engine passes; callers should trigger them separately if needed.
    pub async fn insert_facts_batch(
        &self,
        facts: Vec<models::fact::NewFact>,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        use std::collections::HashSet;

        if facts.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = self.pool.begin().await?;
        let now = self.now();

        let referenced_ids: HashSet<i32> = facts
            .iter()
            .flat_map(|f| &f.category_ids)
            .copied()
            .collect();

        let valid_ids: HashSet<i32> = if referenced_ids.is_empty() {
            HashSet::new()
        } else {
            let mut builder =
                sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT id FROM categories WHERE id IN (");
            let mut first = true;
            for id in &referenced_ids {
                if !first {
                    builder.push(", ");
                }
                builder.push_bind(id);
                first = false;
            }
            builder.push(")");
            builder
                .build_query_scalar::<i32>()
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .collect()
        };

        let mut results = Vec::with_capacity(facts.len());
        for new_fact in &facts {
            for category_id in &new_fact.category_ids {
                if !valid_ids.contains(category_id) {
                    return Err(KnowledgeError::Validation(format!(
                        "Category {} does not exist",
                        category_id
                    )));
                }
            }
        }

        for new_fact in &facts {
            let relationship_type_id = self
                .ensure_relationship_type_in_tx(&mut tx, &new_fact.relationship_type)
                .await?;

            let confidence = if let Some(conf) = new_fact.confidence {
                conf
            } else {
                crate::confidence::initial(new_fact.source_type, None)
            };

            let fact = queries::fact::insert_fact_in_tx(
                &mut tx,
                new_fact,
                relationship_type_id,
                &new_fact.relationship_type,
                confidence,
                now,
            )
            .await?;

            if !new_fact.category_ids.is_empty() {
                for category_id in &new_fact.category_ids {
                    sqlx::query(
                        "INSERT OR IGNORE INTO fact_categories (fact_id, category_id) VALUES (?, ?)")
                        .bind(fact.id)
                        .bind(category_id)
                        .execute(&mut *tx)
                        .await?;
                }
            }

            results.push(fact);
        }

        tx.commit().await?;

        for fact in &results {
            self.bump_centrality(fact.subject_id).await;
            if let Some(object_id) = fact.object_id {
                self.bump_centrality(object_id).await;
            }
        }
        self.set_condensation_dirty();

        Ok(results)
    }

    pub(crate) fn insert_fact_internal<'a>(
        &'a self,
        mut new_fact: NewFact,
        ctx: &'a mut CascadeContext,
    ) -> Pin<Box<dyn Future<Output = Result<models::fact::Fact, KnowledgeError>> + Send + 'a>> {
        Box::pin(async move {
            // Resolve predicate name to id.
            let relationship_type_id = self
                .ensure_relationship_type(&new_fact.relationship_type)
                .await?;

            // Cycle detection: skip duplicate triples in the same cascade.
            if ctx.contains(
                new_fact.subject_id,
                relationship_type_id,
                new_fact.object_id,
                new_fact.object_literal.as_deref(),
            ) {
                return Err(KnowledgeError::Validation(
                    "inference cycle detected".to_string(),
                ));
            }
            ctx.insert(
                new_fact.subject_id,
                relationship_type_id,
                new_fact.object_id,
                new_fact.object_literal.clone(),
            );

            // Connector provenance validation — always enforced when
            // connector_instance_id is set, independent of whether confidence is
            // supplied explicitly. A registered connector instance is the identity:
            // require raw_reference and extraction_method, resolve the instance, and
            // enforce that the denormalised connector_type (if supplied) matches the
            // instance's registered type — or derive it from the instance when
            // omitted. Returns the connector-derived confidence score for use when
            // confidence is not supplied explicitly.
            let connector_confidence: Option<f32> = if let Some(instance_id) =
                new_fact.connector_instance_id
            {
                if new_fact.raw_reference.is_none() || new_fact.extraction_method.is_none() {
                    return Err(KnowledgeError::Validation(
                            "Connector provenance requires connector_instance_id, raw_reference, and extraction_method"
                                .to_string(),
                        ));
                }
                let instance_type_id: Option<i16> =
                    sqlx::query_scalar("SELECT connector_type_id FROM connectors WHERE id = ?")
                        .bind(instance_id)
                        .fetch_optional(&self.pool)
                        .await?;
                let instance_type_id = instance_type_id.ok_or_else(|| {
                    KnowledgeError::Validation(format!(
                        "connector instance {instance_id} not found"
                    ))
                })?;
                if let Some(ct) = new_fact.connector_type {
                    if (ct as i16) != instance_type_id {
                        return Err(KnowledgeError::Validation(format!(
                            "connector_instance_id {instance_id} has type {instance_type_id} but connector_type was supplied as {}",
                            ct as i16
                        )));
                    }
                } else {
                    // Derive the denormalised connector_type from the instance.
                    // If the instance's type id is outside the seeded ConnectorType
                    // enum (e.g. a connector_types row added before the enum was
                    // extended), surface a validation error rather than panicking.
                    new_fact.connector_type = match ConnectorType::try_from(instance_type_id) {
                        Ok(ct) => Some(ct),
                        Err(()) => {
                            return Err(KnowledgeError::Validation(format!(
                                "connector_instance_id {instance_id} has unknown connector_type_id {instance_type_id}"
                            )));
                        }
                    };
                }
                let resolved_ct = new_fact.connector_type.ok_or_else(|| {
                        KnowledgeError::Validation(format!(
                            "connector_instance_id {instance_id} has unknown connector_type_id {instance_type_id}"
                        ))
                    })?;
                let db_score: Option<f32> = sqlx::query_scalar(
                    "SELECT score FROM connector_reliability WHERE connector_type_id = ?",
                )
                .bind(instance_type_id)
                .fetch_optional(&self.pool)
                .await?;
                Some(db_score.unwrap_or_else(|| confidence::default_connector_score(resolved_ct)))
            } else {
                None
            };

            // Determine confidence. Explicit confidence takes precedence, then
            // inferred facts, then the connector-derived score, then the default
            // initial score for the source type. Connector provenance validation
            // above always runs when connector_instance_id is set, so an explicit
            // confidence can no longer bypass it.
            let confidence = if let Some(conf) = new_fact.confidence {
                conf
            } else if new_fact.inferred {
                confidence::initial(SourceType::Inference, None)
            } else if let Some(score) = connector_confidence {
                score
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
                relationship_type_id,
                &new_fact.relationship_type,
                confidence,
                self.now(),
            )
            .await?;

            // Validate category IDs and insert assignments.
            if !new_fact.category_ids.is_empty() {
                let valid_ids: HashSet<i32> = sqlx::query_scalar("SELECT id FROM categories")
                    .fetch_all(&mut *tx)
                    .await?
                    .into_iter()
                    .collect();
                for category_id in &new_fact.category_ids {
                    if !valid_ids.contains(category_id) {
                        return Err(KnowledgeError::Validation(format!(
                            "Category {} does not exist",
                            category_id
                        )));
                    }
                    sqlx::query("INSERT OR IGNORE INTO fact_categories (fact_id, category_id) VALUES (?, ?)")
                        .bind(fact.id)
                        .bind(*category_id)
                        .execute(&mut *tx)
                        .await?;
                }
            }

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
            let threshold_input = if new_fact.relationship_type == RELATIONSHIP_TYPE_REJECTED_ACTION
            {
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

            self.bump_centrality(fact.subject_id).await;
            if let Some(oid) = fact.object_id {
                self.bump_centrality(oid).await;
            }
            self.set_condensation_dirty();
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
    pub async fn get_facts_by_relationship_type(
        &self,
        relationship_type_id: i16,
        limit: i64,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_by_predicate(&self.pool, relationship_type_id, limit).await
    }

    /// List facts for an object entity.
    pub async fn get_facts_by_object(
        &self,
        object_id: i32,
        limit: i64,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_by_object(&self.pool, object_id, limit).await
    }

    /// List facts for a specific subject and predicate.
    pub async fn get_facts_by_subject_and_predicate(
        &self,
        subject_id: i32,
        relationship_type_id: i16,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        let facts: Vec<models::fact::Fact> = sqlx::query_as::<_, models::fact::Fact>(
            "SELECT id, subject_id, relationship_type_id, object_id, object_literal, valid_from, valid_until, confidence, fact_status_id, inferred, inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at FROM facts WHERE subject_id = ? AND relationship_type_id = ? ORDER BY id ASC"
        )
        .bind(subject_id)
        .bind(relationship_type_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(facts)
    }

    /// Query facts for a subject with optional predicate filter, confidence threshold,
    /// and pagination. Returns enriched rows with object names and sources.
    pub async fn query_facts(
        &self,
        subject_id: i32,
        relationship_type_id: Option<i16>,
        min_confidence: f32,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<queries::fact::FactWithSources>, KnowledgeError> {
        queries::fact::get_facts_by_subject_filtered(
            &self.pool,
            subject_id,
            relationship_type_id,
            min_confidence,
            offset,
            limit,
        )
        .await
    }

    /// Count facts for a subject with optional predicate filter and confidence threshold.
    pub async fn count_facts(
        &self,
        subject_id: i32,
        relationship_type_id: Option<i16>,
        min_confidence: f32,
    ) -> Result<i64, KnowledgeError> {
        queries::fact::count_facts_by_subject_filtered(
            &self.pool,
            subject_id,
            relationship_type_id,
            min_confidence,
        )
        .await
    }

    /// Retrieve facts for a subject whose relationship type is `root_type_id` or any
    /// descendant in the relationship-type DAG (recursive CTE traversal).
    ///
    /// Convenience wrapper around [`queries::fact::get_facts_by_relationship_subtree`]
    /// with `min_confidence = 0.0` (all matching facts, ranked by confidence).
    pub async fn get_facts_by_relationship_subtree(
        &self,
        entity_id: i32,
        root_type_id: i16,
        limit: i64,
    ) -> Result<Vec<queries::fact::FactWithSources>, KnowledgeError> {
        queries::fact::get_facts_by_relationship_subtree(
            &self.pool,
            entity_id,
            root_type_id,
            0.0,
            limit,
        )
        .await
    }

    /// Get dependency edges for a fact.
    pub async fn get_fact_dependencies(
        &self,
        fact_id: i32,
    ) -> Result<Vec<(i32, i32, i16)>, KnowledgeError> {
        let rows: Vec<(i32, i32, i16)> = sqlx::query_as(
            "SELECT parent_fact_id, child_fact_id, relation_type_id FROM fact_dependencies WHERE parent_fact_id = ? OR child_fact_id = ?"
        )
        .bind(fact_id)
        .bind(fact_id)
        .fetch_all(&self.pool)
        .await
        .map_err(KnowledgeError::Pool)?;
        Ok(rows)
    }

    /// Return facts active at a specific point in time.
    pub async fn get_active_facts_at(
        &self,
        subject_id: i32,
        relationship_type_id: i16,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        queries::fact::get_active_facts_at(&self.pool, subject_id, relationship_type_id, at).await
    }

    /// Update a fact's valid-until timestamp.
    pub async fn update_fact_valid_until(
        &self,
        id: i32,
        valid_until: Option<chrono::DateTime<chrono::Utc>>,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        let fact =
            queries::fact::update_valid_until(&self.pool, id, valid_until, self.now(), changed_by)
                .await?;
        self.set_condensation_dirty();
        Ok(fact)
    }

    /// Update a fact's lifecycle status.
    pub async fn update_fact_status(
        &self,
        id: i32,
        status: models::fact::FactStatus,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        let fact =
            queries::fact::set_status(&self.pool, id, status, self.now(), changed_by).await?;
        self.set_centrality_dirty().await;
        self.set_condensation_dirty();
        Ok(fact)
    }

    #[allow(clippy::too_many_arguments)]
    /// Update mutable fields on a fact in a single transaction.
    pub async fn update_fact(
        &self,
        id: i32,
        confidence: Option<f32>,
        valid_from: Option<chrono::DateTime<chrono::Utc>>,
        valid_until: Option<chrono::DateTime<chrono::Utc>>,
        object_literal: Option<String>,
        status: Option<models::fact::FactStatus>,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        // Fetch current fact to compare status
        let old_fact = self
            .get_fact(id)
            .await?
            .ok_or(KnowledgeError::FactNotFound(id))?;
        let old_status = old_fact.status();

        let fact = queries::fact::update_fact(
            &self.pool,
            id,
            confidence,
            valid_from,
            valid_until,
            object_literal,
            status,
            self.now(),
            changed_by,
        )
        .await?;

        // If status changed, invalidate centrality cache
        if let Some(new_status) = status {
            if old_status != Some(new_status) {
                self.set_centrality_dirty().await;
            }
        }

        self.set_condensation_dirty();
        Ok(fact)
    }

    /// Soft-delete a fact to trash, cascading to inferred children.
    pub async fn forget_fact(
        &self,
        id: i32,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<(), KnowledgeError> {
        let fact = self
            .get_fact(id)
            .await?
            .ok_or_else(|| KnowledgeError::FactNotFound(id))?;
        forget::forget_fact(&self.pool, id, changed_by, self.now()).await?;
        self.drop_centrality(fact.subject_id).await;
        if let Some(oid) = fact.object_id {
            self.drop_centrality(oid).await;
        }
        self.set_condensation_dirty();
        Ok(())
    }

    /// Bulk forget facts with filters and safeguards.
    pub async fn forget_facts(
        &self,
        filters: forget::ForgetFilters,
        opts: forget::ForgetOptions,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<forget::ForgetResult, KnowledgeError> {
        let result =
            forget::forget_facts(&self.pool, filters, opts, changed_by, self.now()).await?;
        self.set_condensation_dirty();
        Ok(result)
    }

    /// Soft-delete (trash) every fact sourced from a single connector instance
    /// (Phase 3 A2 / #203).
    ///
    /// The connector `forget` cascade: trashes all facts whose `sources` row
    /// carries `connector_instance_id = id` via the shared trash machinery, so
    /// they are recoverable from trash (30-day expiry). `sources` rows are
    /// cascade-deleted with their facts; the connector row and its stored
    /// secret are removed separately by the caller. Returns how many facts
    /// were trashed.
    pub async fn forget_connector_facts(
        &self,
        instance_id: i32,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<forget::ForgetResult, KnowledgeError> {
        let result =
            forget::forget_facts_for_connector(&self.pool, instance_id, changed_by, self.now())
                .await?;
        self.set_condensation_dirty();
        Ok(result)
    }

    /// Soft-delete (trash) the facts of one connector instance whose
    /// `sources.raw_reference` is in `raw_references` (issue #247).
    ///
    /// The server-side-deletion (tombstone) path: the supervisor drains a
    /// connector's reported removals after `sync` and hands them here, so a
    /// remote deletion trashes exactly the facts that instance authored for
    /// that raw item — recoverable from trash (30-day expiry), with the
    /// shared trash machinery (inferred-child cascade, audit) applied.
    /// Idempotent: re-reported raw references whose facts are already trashed
    /// (or that never existed) return a zero count without error.
    pub async fn forget_connector_facts_by_raw_reference(
        &self,
        instance_id: i32,
        raw_references: &[String],
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<forget::ForgetResult, KnowledgeError> {
        let result = forget::forget_facts_for_connector_raw_references(
            &self.pool,
            instance_id,
            raw_references,
            changed_by,
            self.now(),
        )
        .await?;
        self.set_condensation_dirty();
        Ok(result)
    }

    /// Restore a single fact from trash.
    pub async fn restore_fact(
        &self,
        trash_id: i32,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<models::fact::Fact, KnowledgeError> {
        let restored =
            queries::trash::restore_fact(&self.pool, trash_id, changed_by, self.now()).await?;
        self.bump_centrality(restored.subject_id).await;
        if let Some(oid) = restored.object_id {
            self.bump_centrality(oid).await;
        }
        self.set_condensation_dirty();
        Ok(restored)
    }

    /// Restore all facts from trash.
    pub async fn restore_all(
        &self,
        changed_by: models::audit_log::ChangedBy,
    ) -> Result<Vec<models::fact::Fact>, KnowledgeError> {
        let restored = queries::trash::restore_all(&self.pool, changed_by, self.now()).await?;
        self.set_condensation_dirty();
        Ok(restored)
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
}
