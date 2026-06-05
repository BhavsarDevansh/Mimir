//! Threshold rule: 3+ rejected_action facts → upsert a preference.

use async_trait::async_trait;

use crate::KnowledgeGraph;
use crate::inference::InferenceRule;
use crate::models::audit_log::ChangedBy;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::preference::{NewPreference, PreferenceCategory, UpsertPreferenceInput};

pub(crate) const RELATIONSHIP_TYPE_REJECTED_ACTION: &str = "rejected_action";
const KEY_PREFIX: &str = "reject_";

pub struct ThresholdRule;

#[async_trait]
impl InferenceRule for ThresholdRule {
    async fn evaluate(
        &self,
        _fact: &Fact,
        _kg: &KnowledgeGraph,
    ) -> Result<Vec<NewFact>, crate::KnowledgeError> {
        // Threshold side-effects are handled explicitly in insert_fact_internal
        // so that the InferenceRule trait remains a pure inference abstraction.
        Ok(Vec::new())
    }
}

impl ThresholdRule {
    /// Check whether a rejected_action fact has reached the threshold (3+)
    /// and upsert a preference if so.
    pub(crate) async fn check_threshold(
        fact: &Fact,
        kg: &KnowledgeGraph,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<Option<crate::models::preference::UpsertPreferenceInput>, crate::KnowledgeError>
    {
        let relationship_type_name =
            match kg.relationship_type_name(fact.relationship_type_id).await {
                Some(name) => name,
                None => return Ok(None),
            };

        if relationship_type_name != RELATIONSHIP_TYPE_REJECTED_ACTION {
            return Ok(None);
        }

        let object_value = match fact.object_id {
            Some(oid) => match kg.get_entity(oid).await {
                Ok(Some(e)) => e.name,
                _ => return Ok(None),
            },
            None => match &fact.object_literal {
                Some(lit) => lit.clone(),
                None => return Ok(None),
            },
        };

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM facts \
             WHERE subject_id = ? AND relationship_type_id = ? \
               AND fact_status_id = ? \
               AND (object_id IS ?) AND (object_literal IS ?)",
        )
        .bind(fact.subject_id)
        .bind(fact.relationship_type_id)
        .bind(FactStatus::Active as i16)
        .bind(fact.object_id)
        .bind(&fact.object_literal)
        .fetch_one(&mut **tx)
        .await?;

        if count >= 3 {
            let key = format!("{}{}", KEY_PREFIX, object_value);
            let input = UpsertPreferenceInput {
                preference: NewPreference {
                    entity_id: Some(fact.subject_id),
                    category: PreferenceCategory::General,
                    key,
                    value: "true".to_string(),
                    confidence: 0.70,
                    overridden_by_user: false,
                    source_fact_id: Some(fact.id),
                },
                changed_by: ChangedBy::InferenceEngine,
                contexts: Vec::new(),
                sources: Vec::new(),
            };
            return Ok(Some(input));
        }

        Ok(None)
    }
}

impl ThresholdRule {
    /// Nightly re-count: warn if threshold no longer met for preferences created by this rule.
    pub async fn evaluate_batch(kg: &KnowledgeGraph) -> Result<(), crate::KnowledgeError> {
        let pool = kg.pool();

        let prefs: Vec<(i32, i32)> = sqlx::query_as(
            "SELECT id, source_fact_id FROM preferences \
             WHERE key LIKE 'reject_%' AND source_fact_id IS NOT NULL",
        )
        .fetch_all(pool)
        .await?;

        for (pref_id, source_fact_id) in prefs {
            let source: Option<Fact> = sqlx::query_as::<_, Fact>(
                "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                 valid_from, valid_until, confidence, fact_status_id, inferred, \
                 inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
                 FROM facts WHERE id = ?",
            )
            .bind(source_fact_id)
            .fetch_optional(pool)
            .await?;

            let Some(source) = source else {
                // Stale preference: source fact no longer exists; remove it.
                sqlx::query("DELETE FROM preferences WHERE id = ?")
                    .bind(pref_id)
                    .execute(pool)
                    .await?;
                tracing::info!(
                    "removed stale preference {} because source fact {} no longer exists",
                    pref_id,
                    source_fact_id
                );
                continue;
            };

            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM facts \
                 WHERE subject_id = ? AND relationship_type_id = ? \
                   AND fact_status_id = ? \
                   AND (object_id IS ?) AND (object_literal IS ?)",
            )
            .bind(source.subject_id)
            .bind(source.relationship_type_id)
            .bind(FactStatus::Active as i16)
            .bind(source.object_id)
            .bind(&source.object_literal)
            .fetch_one(pool)
            .await?;

            if count < 3 {
                let now = kg.now();

                // Deduplicate: skip if an identical StatusChange audit by NightlyOptimization
                // already exists within the last 24 hours.
                let recent_dup: Option<i64> = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM preference_audit_log \
                     WHERE preference_id = ? \
                       AND change_type_id = ? \
                       AND changed_by_id = ? \
                       AND reason = ? \
                       AND changed_at >= ?",
                )
                .bind(pref_id)
                .bind(crate::models::audit_log::ChangeType::StatusChange as i16)
                .bind(ChangedBy::NightlyOptimization as i16)
                .bind("threshold no longer met; review recommended")
                .bind(now - chrono::Duration::hours(24))
                .fetch_one(pool)
                .await?;

                if recent_dup.unwrap_or(0) == 0 {
                    sqlx::query(
                        "INSERT INTO preference_audit_log \
                         (preference_id, change_type_id, old_value, new_value, changed_at, changed_by_id, reason) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(pref_id)
                    .bind(crate::models::audit_log::ChangeType::StatusChange as i16)
                    .bind(None::<&str>)
                    .bind(None::<&str>)
                    .bind(now)
                    .bind(ChangedBy::NightlyOptimization as i16)
                    .bind(Some("threshold no longer met; review recommended"))
                    .execute(pool)
                    .await?;
                }
            }
        }

        Ok(())
    }
}
