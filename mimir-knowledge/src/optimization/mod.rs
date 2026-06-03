//! Nightly optimization orchestrator.

use crate::KnowledgeGraph;
use crate::inference::rules::contradiction::ContradictionRule;
use crate::inference::rules::threshold::ThresholdRule;
use crate::models::fact::NewFact;

/// Check whether any fact with the same triple already exists.
async fn fact_already_exists(
    kg: &KnowledgeGraph,
    new_fact: &NewFact,
    predicate_id: i16,
) -> Result<bool, crate::KnowledgeError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM facts \
         WHERE subject_id = ? AND predicate_id = ? \
           AND (object_id IS ?) AND (object_literal IS ?)",
    )
    .bind(new_fact.subject_id)
    .bind(predicate_id)
    .bind(new_fact.object_id)
    .bind(&new_fact.object_literal)
    .fetch_one(kg.pool())
    .await?;
    Ok(count > 0)
}

/// Run the nightly optimization passes in order.
///
/// 1. Contradiction auto-resolution (explicit > inferred)
/// 2. Confidence propagation for stale facts
/// 3. Inference re-evaluation
///
/// Leaves dormant cleanup and compaction as TODO stubs.
pub async fn run_nightly_optimization(kg: &KnowledgeGraph) -> Result<(), crate::KnowledgeError> {
    // 1. Contradiction auto-resolution.
    ContradictionRule::evaluate_batch(kg).await?;

    // 2. Confidence propagation.
    let stale_facts: Vec<i32> =
        sqlx::query_scalar("SELECT id FROM facts WHERE stale_confidence = TRUE")
            .fetch_all(kg.pool())
            .await?;

    for fact_id in stale_facts {
        crate::confidence::cascade_confidence_change(kg.pool(), fact_id, None).await?;
    }

    // 3. Inference re-evaluation.
    let mut engine = crate::inference::RuleEngine::new();
    engine.register(Box::new(
        crate::inference::rules::transitivity::TransitivityRule,
    ));
    engine.register(Box::new(
        crate::inference::rules::contradiction::ContradictionRule,
    ));
    // ThresholdRule is intentionally omitted from the batch engine because its
    // side-effect (preference upsert) is handled separately.
    let inferred = engine.evaluate_batch(kg).await?;

    for mut new_fact in inferred {
        new_fact.inferred = true;
        new_fact.source_type = crate::models::source::SourceType::Inference;
        new_fact.extraction_method = Some(crate::models::source::ExtractionMethod::InferenceRule);

        let predicate_id = kg.ensure_predicate(&new_fact.predicate).await?;

        if fact_already_exists(kg, &new_fact, predicate_id).await? {
            tracing::debug!("nightly batch: skipping duplicate fact");
            continue;
        }

        // Each top-level insertion gets its own CascadeContext so that cycle
        // detection is scoped to a single cascade, not shared across the batch.
        let mut ctx = crate::inference::CascadeContext::new();
        kg.insert_fact_internal(new_fact, &mut ctx).await?;
    }

    // 4. Threshold nightly re-count.
    ThresholdRule::evaluate_batch(kg).await?;

    // TODO: dormant cleanup pass.
    // TODO: compaction pass.

    Ok(())
}
