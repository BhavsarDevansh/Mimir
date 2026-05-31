//! Confidence computation (placeholder for #51 structural confidence model).

use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::source::SourceType;

/// Compute initial confidence from source type.
///
/// TODO(#51): Replace with full structural confidence model.
pub fn initial(source_type: SourceType) -> f32 {
    match source_type {
        SourceType::UserEdit => 1.0,
        SourceType::Connector => 0.80,
        SourceType::Email | SourceType::Calendar | SourceType::Photo | SourceType::Message => {
            0.80
        }
        SourceType::Inference => 0.50,
    }
}

/// Recalculate confidence for an inferred fact after a parent is removed.
///
/// Averages remaining parent confidences × 0.8^depth.
/// If recalculated confidence < 0.20, the caller should mark the fact `Disputed`.
///
/// TODO(#51): Replace with actual dependency depth tracking.
pub async fn recalculate(pool: &SqlitePool, fact_id: i32) -> Result<f32, KnowledgeError> {
    let avg: Option<f64> = sqlx::query_scalar(
        "SELECT AVG(f.confidence) \
         FROM facts f \
         JOIN fact_dependencies fd ON fd.parent_fact_id = f.id \
         WHERE fd.child_fact_id = ?",
    )
    .bind(fact_id)
    .fetch_one(pool)
    .await?;

    let avg = avg.unwrap_or(0.0) as f32;

    // Placeholder depth = 1 → multiply by 0.8 once.
    let depth_penalty = 0.8f32.powi(1);
    let new_confidence = avg * depth_penalty;

    Ok(new_confidence)
}
