//! Transitivity rule: A-[visited/located_in]-B + B-located_in-C → A-[visited/located_in]-C.

use async_trait::async_trait;

use crate::KnowledgeGraph;
use crate::confidence;
use crate::inference::InferenceRule;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::models::source::{ExtractionMethod, SourceType};

const RELATIONSHIP_TYPE_VISITED: &str = "visited";
const RELATIONSHIP_TYPE_LOCATED_IN: &str = "located_in";

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

        // Only run for predicates we care about. `is_in` was consolidated
        // into `located_in` by migration 051 (issue #403); the alias table
        // resolves any legacy `is_in` name to the same id.
        if relationship_type_name != RELATIONSHIP_TYPE_VISITED
            && relationship_type_name != RELATIONSHIP_TYPE_LOCATED_IN
        {
            return Ok(Vec::new());
        }

        let located_in_id = kg
            .resolve_emit_eligible_relationship_type(RELATIONSHIP_TYPE_LOCATED_IN)
            .await?
            .ok_or_else(|| {
                crate::KnowledgeError::Validation(format!(
                    "inferred predicate '{}' is not an emit-eligible taxonomy leaf",
                    RELATIONSHIP_TYPE_LOCATED_IN
                ))
            })?;

        let mut results = Vec::new();

        if relationship_type_name == RELATIONSHIP_TYPE_VISITED {
            // Forward: A-visited-B inserted, look for B-located_in-C → infer
            // A-visited-C.
            if let Some(object_id) = fact.object_id {
                let parent_facts: Vec<Fact> = sqlx::query_as::<_, Fact>(
                    "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
                     valid_from, valid_until, confidence, fact_status_id, inferred, \
                     inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
                     FROM facts \
                     WHERE subject_id = ? AND relationship_type_id = ? AND fact_status_id IN (?, ?)",
                )
                .bind(object_id)
                .bind(located_in_id)
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
                            connector_instance_id: None,
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
        } else if relationship_type_name == RELATIONSHIP_TYPE_LOCATED_IN {
            // We do NOT do forward located_in→located_in lookup to prevent
            // cyclic garbage. (Backward located_in→visited lookup is below.)
        }

        if relationship_type_name == RELATIONSHIP_TYPE_LOCATED_IN {
            // Backward: B-located_in-C inserted, look for A-visited-B → infer
            // A-visited-C. We do NOT do backward lookup for located_in-to-
            // located_in to avoid self-disputing chains.
            let visited_id = kg
                .resolve_emit_eligible_relationship_type(RELATIONSHIP_TYPE_VISITED)
                .await?
                .ok_or_else(|| {
                    crate::KnowledgeError::Validation(format!(
                        "inferred predicate '{}' is not an emit-eligible taxonomy leaf",
                        RELATIONSHIP_TYPE_VISITED
                    ))
                })?;

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
                        connector_instance_id: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};

    fn ts(minute: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::minutes(minute)
    }

    #[test]
    fn intersect_both_unbounded_is_unbounded() {
        let (f, u) = intersect_windows(None, None, None, None);
        assert_eq!(f, None);
        assert_eq!(u, None);
    }

    #[test]
    fn intersect_from_takes_max_of_bounded() {
        let (f, _) = intersect_windows(Some(ts(10)), None, Some(ts(20)), None);
        assert_eq!(f, Some(ts(20)));
        let (f, _) = intersect_windows(Some(ts(20)), None, Some(ts(10)), None);
        assert_eq!(f, Some(ts(20)));
    }

    #[test]
    fn intersect_from_takes_the_only_bounded_side() {
        let (f, _) = intersect_windows(None, None, Some(ts(5)), None);
        assert_eq!(f, Some(ts(5)));
        let (f, _) = intersect_windows(Some(ts(7)), None, None, None);
        assert_eq!(f, Some(ts(7)));
    }

    #[test]
    fn intersect_until_takes_min_of_bounded() {
        let (_, u) = intersect_windows(None, Some(ts(10)), None, Some(ts(20)));
        assert_eq!(u, Some(ts(10)));
        let (_, u) = intersect_windows(None, Some(ts(20)), None, Some(ts(10)));
        assert_eq!(u, Some(ts(10)));
    }

    #[test]
    fn intersect_until_takes_the_only_bounded_side() {
        let (_, u) = intersect_windows(None, None, None, Some(ts(9)));
        assert_eq!(u, Some(ts(9)));
        let (_, u) = intersect_windows(None, Some(ts(3)), None, None);
        assert_eq!(u, Some(ts(3)));
    }

    #[test]
    fn intersect_overlapping_windows_yields_intersection() {
        // a: [10, 40], b: [20, 30] => [20, 30]
        let (f, u) = intersect_windows(Some(ts(10)), Some(ts(40)), Some(ts(20)), Some(ts(30)));
        assert_eq!(f, Some(ts(20)));
        assert_eq!(u, Some(ts(30)));
    }

    #[test]
    fn intersect_disjoint_windows_still_computes_inverted_bounds() {
        // a: [10, 20], b: [30, 40] => from=30, until=20 (empty interval).
        // The function performs no emptiness check; callers must validate.
        let (f, u) = intersect_windows(Some(ts(10)), Some(ts(20)), Some(ts(30)), Some(ts(40)));
        assert_eq!(f, Some(ts(30)));
        assert_eq!(u, Some(ts(20)));
    }
}
