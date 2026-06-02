//! Transitivity rule: A-[visited/is_in]-B + B-is_in-C → A-[visited/is_in]-C.

use async_trait::async_trait;

use crate::KnowledgeGraph;
use crate::confidence;
use crate::inference::InferenceRule;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};

const PREDICATE_VISITED: &str = "visited";
const PREDICATE_IS_IN: &str = "is_in";

pub struct TransitivityRule;

#[async_trait]
impl InferenceRule for TransitivityRule {
    async fn evaluate(&self, fact: &Fact, kg: &KnowledgeGraph) -> Vec<NewFact> {
        let predicate_name = match kg.predicate_name(fact.predicate_id).await {
            Some(name) => name,
            None => return Vec::new(),
        };

        let is_in_id = match kg.ensure_predicate(PREDICATE_IS_IN).await {
            Ok(id) => id,
            Err(_) => return Vec::new(),
        };

        let mut results = Vec::new();

        if predicate_name == PREDICATE_VISITED || predicate_name == PREDICATE_IS_IN {
            // Forward: A-P-B inserted, look for B-is_in-C → infer A-P-C.
            if let Some(object_id) = fact.object_id {
                let parent_facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
                    "SELECT id, subject_id, predicate_id, object_id, object_literal, \
                     valid_from, valid_until, confidence, fact_status_id, inferred, \
                     inference_depth, stale_confidence, created_at, updated_at \
                     FROM facts \
                     WHERE subject_id = ? AND predicate_id = ? AND fact_status_id = ?",
                )
                .bind(object_id)
                .bind(is_in_id)
                .bind(FactStatus::Active as i16)
                .fetch_all(kg.pool())
                .await
                .unwrap_or_default();

                for parent in parent_facts {
                    if let Some(parent_object_id) = parent.object_id {
                        let depth = std::cmp::max(fact.inference_depth, parent.inference_depth) + 1;
                        let conf = confidence::inference_confidence(
                            &[(fact.confidence, true), (parent.confidence, true)],
                            depth,
                            2,
                        );
                        results.push(NewFact {
                            subject_id: fact.subject_id,
                            predicate: predicate_name.clone(),
                            object_id: Some(parent_object_id),
                            object_literal: None,
                            valid_from: fact.valid_from,
                            valid_until: fact.valid_until,
                            source_type: SourceType::Inference,
                            connector_id: None,
                            connector_type: None,
                            raw_reference: None,
                            extraction_method: Some(ExtractionMethod::InferenceRule),
                            inferred: true,
                            inference_depth: depth,
                            confidence: Some(conf),
                            parent_fact_ids: vec![fact.id, parent.id],
                        });
                    }
                }
            }
        }

        if predicate_name == PREDICATE_IS_IN {
            // Backward: B-is_in-C inserted, look for A-visited-B → infer A-visited-C.
            // We do NOT do backward lookup for is_in-to-is_in to avoid self-disputing chains.
            let visited_id = match kg.ensure_predicate(PREDICATE_VISITED).await {
                Ok(id) => id,
                Err(_) => return results,
            };

            let trigger_facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
                "SELECT id, subject_id, predicate_id, object_id, object_literal, \
                 valid_from, valid_until, confidence, fact_status_id, inferred, \
                 inference_depth, stale_confidence, created_at, updated_at \
                 FROM facts \
                 WHERE object_id = ? AND predicate_id = ? AND fact_status_id = ?",
            )
            .bind(fact.subject_id)
            .bind(visited_id)
            .bind(FactStatus::Active as i16)
            .fetch_all(kg.pool())
            .await
            .unwrap_or_default();

            for trigger in trigger_facts {
                if let Some(trigger_object_id) = fact.object_id {
                    let depth = std::cmp::max(fact.inference_depth, trigger.inference_depth) + 1;
                    let conf = confidence::inference_confidence(
                        &[(trigger.confidence, true), (fact.confidence, true)],
                        depth,
                        2,
                    );
                    results.push(NewFact {
                        subject_id: trigger.subject_id,
                        predicate: PREDICATE_VISITED.to_string(),
                        object_id: Some(trigger_object_id),
                        object_literal: None,
                        valid_from: trigger.valid_from,
                        valid_until: trigger.valid_until,
                        source_type: SourceType::Inference,
                        connector_id: None,
                        connector_type: None,
                        raw_reference: None,
                        extraction_method: Some(ExtractionMethod::InferenceRule),
                        inferred: true,
                        inference_depth: depth,
                        confidence: Some(conf),
                        parent_fact_ids: vec![trigger.id, fact.id],
                    });
                }
            }
        }

        results
    }
}
