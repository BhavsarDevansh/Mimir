//! Contradiction rule: auto-resolve explicit vs. inferred disputes in batch passes.

use async_trait::async_trait;

use crate::KnowledgeGraph;
use crate::inference::InferenceRule;
use crate::models::audit_log::ChangedBy;
use crate::models::fact::{Fact, FactStatus, NewFact};

pub struct ContradictionRule;

#[async_trait]
impl InferenceRule for ContradictionRule {
    async fn evaluate(
        &self,
        _fact: &Fact,
        _kg: &KnowledgeGraph,
    ) -> Result<Vec<NewFact>, crate::KnowledgeError> {
        // Real-time contradiction handling is done inside queries::fact::insert_fact.
        Ok(Vec::new())
    }
}

impl ContradictionRule {
    /// Batch auto-resolution: explicit (non-inferred) beats inferred.
    pub async fn evaluate_batch(kg: &KnowledgeGraph) -> Result<(), crate::KnowledgeError> {
        let pool = kg.pool();
        let now = kg.now();

        // Find all disputed fact pairs linked by Contradicts.
        // Deduplicate bidirectional edges with MIN/MAX so each pair is processed once.
        let rows: Vec<(i32, i32)> = sqlx::query_as(
            "SELECT MIN(fd.parent_fact_id, fd.child_fact_id) AS a, \
                    MAX(fd.parent_fact_id, fd.child_fact_id) AS b \
             FROM fact_dependencies fd \
             JOIN facts f1 ON f1.id = fd.parent_fact_id \
             JOIN facts f2 ON f2.id = fd.child_fact_id \
             WHERE fd.relation_type_id = ? \
               AND f1.fact_status_id = ? \
               AND f2.fact_status_id = ? \
             GROUP BY a, b",
        )
        .bind(crate::models::enums::RelationType::Contradicts as i16)
        .bind(FactStatus::Disputed as i16)
        .bind(FactStatus::Disputed as i16)
        .fetch_all(pool)
        .await?;

        for (id_a, id_b) in rows {
            let fact_a: Option<Fact> = sqlx::query_as::<_, Fact>(
                "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                 valid_from, valid_until, confidence, fact_status_id, inferred, \
                 inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
                 FROM facts WHERE id = ?",
            )
            .bind(id_a)
            .fetch_optional(pool)
            .await?;

            let fact_b: Option<Fact> = sqlx::query_as::<_, Fact>(
                "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                 valid_from, valid_until, confidence, fact_status_id, inferred, \
                 inference_depth, stale_confidence, pending_confirmation, created_at, updated_at \
                 FROM facts WHERE id = ?",
            )
            .bind(id_b)
            .fetch_optional(pool)
            .await?;

            let (Some(a), Some(b)) = (fact_a, fact_b) else {
                continue;
            };

            let a_explicit = !a.inferred;
            let b_explicit = !b.inferred;
            let a_inferred = a.inferred;
            let b_inferred = b.inferred;

            if a_explicit && b_inferred {
                let mut tx = pool.begin().await?;
                crate::queries::fact::set_status_tx(
                    &mut tx,
                    b.id,
                    FactStatus::Superseded,
                    now,
                    ChangedBy::NightlyOptimization,
                )
                .await?;
                crate::queries::fact::set_status_tx(
                    &mut tx,
                    a.id,
                    FactStatus::Active,
                    now,
                    ChangedBy::NightlyOptimization,
                )
                .await?;
                tx.commit().await?;
            } else if b_explicit && a_inferred {
                let mut tx = pool.begin().await?;
                crate::queries::fact::set_status_tx(
                    &mut tx,
                    a.id,
                    FactStatus::Superseded,
                    now,
                    ChangedBy::NightlyOptimization,
                )
                .await?;
                crate::queries::fact::set_status_tx(
                    &mut tx,
                    b.id,
                    FactStatus::Active,
                    now,
                    ChangedBy::NightlyOptimization,
                )
                .await?;
                tx.commit().await?;
            }
            // If both inferred or both explicit, leave Disputed for user intervention.
        }

        Ok(())
    }
}
