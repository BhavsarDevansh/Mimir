//! Transitivity rule: A-[visited/is_in]-B + B-is_in-C → A-[visited/is_in]-C.

use async_trait::async_trait;

use crate::KnowledgeGraph;
use crate::confidence;
use crate::inference::InferenceRule;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};

const RELATIONSHIP_TYPE_VISITED: &str = "visited";
const RELATIONSHIP_TYPE_IS_IN: &str = "is_in";

pub struct TransitivityRule;

#[async_trait]
impl InferenceRule for TransitivityRule {
    async fn evaluate(
        &self,
        fact: &Fact,
        kg: &KnowledgeGraph,
    ) -> Result<Vec<NewFact>, crate::KnowledgeError> {
        let relationship_type_name =
            match kg.relationship_type_name(fact.relationship_type_id).await {
                Some(name) => name,
                None => return Ok(Vec::new()),
            };

        // Only run for predicates we care about.
        if relationship_type_name != RELATIONSHIP_TYPE_VISITED
            && relationship_type_name != RELATIONSHIP_TYPE_IS_IN
        {
            return Ok(Vec::new());
        }

        let is_in_id = match kg.ensure_relationship_type(RELATIONSHIP_TYPE_IS_IN).await {
            Ok(id) => id,
            Err(e) => return Err(e),
        };

        let mut results = Vec::new();

        if relationship_type_name == RELATIONSHIP_TYPE_VISITED {
            // Forward: A-visited-B inserted, look for B-is_in-C → infer A-visited-C.
            if let Some(object_id) = fact.object_id {
                let parent_facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
                    "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                     valid_from, valid_until, confidence, fact_status_id, inferred, \
                     inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
                     FROM facts \
                     WHERE subject_id = ? AND relationship_type_id = ? AND fact_status_id IN (?, ?)",
                )
                .bind(object_id)
                .bind(is_in_id)
                .bind(FactStatus::Active as i16)
                .bind(FactStatus::Inferred as i16)
                .fetch_all(kg.pool())
                .await?;

                for parent in parent_facts {
                    if let Some(parent_object_id) = parent.object_id {
                        // Guard against self-referential garbage in cycles.
                        if fact.subject_id == parent_object_id {
                            continue;
                        }
                        let depth = std::cmp::max(fact.inference_depth, parent.inference_depth) + 1;
                        let conf = confidence::inference_confidence(
                            &[(fact.confidence, true), (parent.confidence, true)],
                            depth,
                            2,
                        );
                        let (valid_from, valid_until) = intersect_windows(
                            fact.valid_from,
                            fact.valid_until,
                            parent.valid_from,
                            parent.valid_until,
                        );
                        if let (Some(from), Some(until)) = (valid_from, valid_until) {
                            if from >= until {
                                continue;
                            }
                        }
                        results.push(NewFact {
                            subject_id: fact.subject_id,
                            relationship_type: relationship_type_name.clone(),
                            object_id: Some(parent_object_id),
                            object_literal: None,
                            valid_from,
                            valid_until,
                            source_type: SourceType::Inference,
                            connector_id: None,
                            connector_type: None,
                            raw_reference: None,
                            extraction_method: Some(ExtractionMethod::InferenceRule),
                            inferred: true,
                            inference_depth: depth,
                            confidence: Some(conf),
                            parent_fact_ids: vec![fact.id, parent.id],
                            category_ids: Vec::new(),
                        });
                    }
                }
            }
        } else if relationship_type_name == RELATIONSHIP_TYPE_IS_IN {
            // We do NOT do forward is_in→is_in lookup to prevent cyclic garbage.
            // (Backward is_in→visited lookup is handled below.)
        }

        if relationship_type_name == RELATIONSHIP_TYPE_IS_IN {
            // Backward: B-is_in-C inserted, look for A-visited-B → infer A-visited-C.
            // We do NOT do backward lookup for is_in-to-is_in to avoid self-disputing chains.
            let visited_id = match kg.ensure_relationship_type(RELATIONSHIP_TYPE_VISITED).await {
                Ok(id) => id,
                Err(e) => return Err(e),
            };

            let trigger_facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
                "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                 valid_from, valid_until, confidence, fact_status_id, inferred, \
                 inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
                 FROM facts \
                 WHERE object_id = ? AND relationship_type_id = ? AND fact_status_id IN (?, ?)",
            )
            .bind(fact.subject_id)
            .bind(visited_id)
            .bind(FactStatus::Active as i16)
            .bind(FactStatus::Inferred as i16)
            .fetch_all(kg.pool())
            .await?;

            for trigger in trigger_facts {
                if let Some(trigger_object_id) = fact.object_id {
                    // Guard against self-referential garbage in cycles.
                    if trigger.subject_id == trigger_object_id {
                        continue;
                    }
                    let depth = std::cmp::max(fact.inference_depth, trigger.inference_depth) + 1;
                    let conf = confidence::inference_confidence(
                        &[(trigger.confidence, true), (fact.confidence, true)],
                        depth,
                        2,
                    );
                    let (valid_from, valid_until) = intersect_windows(
                        trigger.valid_from,
                        trigger.valid_until,
                        fact.valid_from,
                        fact.valid_until,
                    );
                    if let (Some(from), Some(until)) = (valid_from, valid_until) {
                        if from >= until {
                            continue;
                        }
                    }
                    results.push(NewFact {
                        subject_id: trigger.subject_id,
                        relationship_type: RELATIONSHIP_TYPE_VISITED.to_string(),
                        object_id: Some(trigger_object_id),
                        object_literal: None,
                        valid_from,
                        valid_until,
                        source_type: SourceType::Inference,
                        connector_id: None,
                        connector_type: None,
                        raw_reference: None,
                        extraction_method: Some(ExtractionMethod::InferenceRule),
                        inferred: true,
                        inference_depth: depth,
                        confidence: Some(conf),
                        parent_fact_ids: vec![trigger.id, fact.id],
                        category_ids: Vec::new(),
                    });
                }
            }
        }

        Ok(results)
    }
}

/// Compute the intersection of two optional validity windows.
/// Returns (valid_from, valid_until) where:
/// - valid_from = max(start1, start2)
/// - valid_until = min(end1, end2)
fn intersect_windows(
    a_from: Option<chrono::DateTime<chrono::Utc>>,
    a_until: Option<chrono::DateTime<chrono::Utc>>,
    b_from: Option<chrono::DateTime<chrono::Utc>>,
    b_until: Option<chrono::DateTime<chrono::Utc>>,
) -> (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
) {
    let from = match (a_from, b_from) {
        (Some(a), Some(b)) => Some(if a > b { a } else { b }),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    let until = match (a_until, b_until) {
        (Some(a), Some(b)) => Some(if a < b { a } else { b }),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    (from, until)
}
