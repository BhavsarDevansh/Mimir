//! Nightly optimization orchestrator.

use crate::KnowledgeGraph;
use crate::inference::rules::contradiction::ContradictionRule;
use crate::inference::rules::threshold::ThresholdRule;

/// Run the nightly optimization passes in order.
///
/// 1. Contradiction auto-resolution (explicit > inferred)
/// 2. Confidence propagation for stale facts
/// 3. Inference re-evaluation
///
/// Leaves dedup, dormant cleanup, and compaction as TODO stubs.
pub async fn run_nightly_optimization(kg: &KnowledgeGraph) -> Result<(), crate::KnowledgeError> {
    // 1. Contradiction auto-resolution.
    ContradictionRule::evaluate_batch(kg).await?;

    // 2. Confidence propagation.
    let stale_facts: Vec<i32> =
        sqlx::query_scalar("SELECT id FROM facts WHERE stale_confidence = TRUE")
            .fetch_all(kg.pool())
            .await?;

    for fact_id in stale_facts {
        crate::confidence::cascade_confidence_change(kg.pool(), fact_id, 5).await?;
    }

    // 3. Inference re-evaluation.
    let mut engine = crate::inference::RuleEngine::new();
    engine.register(Box::new(
        crate::inference::rules::transitivity::TransitivityRule,
    ));
    engine.register(Box::new(
        crate::inference::rules::contradiction::ContradictionRule,
    ));
    engine.register(Box::new(crate::inference::rules::threshold::ThresholdRule));
    let inferred = engine.evaluate_batch(kg).await;
    for mut new_fact in inferred {
        new_fact.inferred = true;
        new_fact.source_type = crate::models::source::SourceType::Inference;
        new_fact.extraction_method = Some(crate::models::source::ExtractionMethod::InferenceRule);
        let _ = kg.insert_fact(new_fact).await;
    }

    // 4. Threshold nightly re-count.
    ThresholdRule::evaluate_batch(kg).await?;

    // TODO: deduplication pass.
    // TODO: dormant cleanup pass.
    // TODO: compaction pass.

    Ok(())
}
