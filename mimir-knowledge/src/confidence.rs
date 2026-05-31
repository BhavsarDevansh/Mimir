//! Structural confidence model for the knowledge graph.
//!
//! Confidence is derived entirely from graph structure — zero LLM involvement,
//! zero time-based decay.  See issue #51.

use sqlx::SqlitePool;
use std::pin::Pin;

use crate::KnowledgeError;
use crate::models::enums::ConnectorType;
use crate::models::source::SourceType;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const CHAIN_PENALTY_BASE: f32 = 0.8;

/// Per-connector default reliability scores, seeded from migration 020.
pub fn default_connector_score(ct: ConnectorType) -> f32 {
    match ct {
        ConnectorType::Gmail => 0.85,
        ConnectorType::Calendar => 0.90,
        ConnectorType::Photos => 0.80,
        ConnectorType::LinkedIn => 0.75,
    }
}

// ---------------------------------------------------------------------------
// Breadth factor
// ---------------------------------------------------------------------------

fn breadth_factor(n: usize) -> f32 {
    match n {
        0 => 0.0,
        1 => 0.6,
        2 => 0.75,
        3 => 0.9,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------------------
// Initial confidence
// ---------------------------------------------------------------------------

/// Compute initial confidence for a fact based solely on its learning mode.
///
/// `connector_type` is required when `source_type == SourceType::Connector`.
/// For other source types it is ignored.
pub fn initial(source_type: SourceType, connector_type: Option<ConnectorType>) -> f32 {
    match source_type {
        SourceType::UserEdit => 1.0,
        SourceType::System => 1.0,
        SourceType::CasualMention => 0.30,
        SourceType::Import => 0.80,
        SourceType::Connector => connector_type.map(default_connector_score).unwrap_or(0.80),
        SourceType::Email | SourceType::Calendar | SourceType::Photo | SourceType::Message => {
            // Legacy source types that map to a connector subtype.
            match source_type {
                SourceType::Calendar => default_connector_score(ConnectorType::Calendar),
                SourceType::Email => default_connector_score(ConnectorType::Gmail),
                SourceType::Photo => default_connector_score(ConnectorType::Photos),
                SourceType::Message => default_connector_score(ConnectorType::Gmail),
                _ => 0.80,
            }
        }
        SourceType::Inference => {
            // Inferred facts never use `initial` — their confidence is
            // computed during insertion by `inference_confidence`.
            0.0
        }
    }
}

// ---------------------------------------------------------------------------
// Inference confidence formula
// ---------------------------------------------------------------------------

/// Confidence of an inferred fact.
///
/// `parents`: (parent.confidence, parent.is_positive_on_this_edge)
/// `static_depth`: inference depth at creation time, never changes
/// `num_parents`: count of remaining parents after any removal
pub fn inference_confidence(parents: &[(f32, bool)], static_depth: i32, num_parents: usize) -> f32 {
    let parent_sum: f32 = parents
        .iter()
        .map(|(conf, is_positive)| if *is_positive { *conf } else { -*conf })
        .sum();

    let chain_penalty = CHAIN_PENALTY_BASE.powi(static_depth);
    let breadth = breadth_factor(num_parents);

    (parent_sum * chain_penalty * breadth).clamp(0.0, 0.95) // non-explicit cap; explicit facts use 1.0
}

// ---------------------------------------------------------------------------
// Recalculate confidence for an inferred fact from its parents
// ---------------------------------------------------------------------------

/// Recalculate confidence for an inferred fact after a parent is removed
/// or its confidence changes.
///
/// Returns the new confidence value.
pub async fn recalculate(pool: &SqlitePool, fact_id: i32) -> Result<f32, KnowledgeError> {
    let parents: Vec<(f32, bool)> = sqlx::query_as(
        "SELECT f.confidence, fd.is_positive \
         FROM facts f \
         JOIN fact_dependencies fd ON fd.parent_fact_id = f.id \
         WHERE fd.child_fact_id = ? AND fd.relation_type_id = ?",
    )
    .bind(fact_id)
    .bind(crate::models::enums::RelationType::InferredFrom as i16)
    .fetch_all(pool)
    .await?;

    let num_parents = parents.len();

    // Fetch the child's static inference_depth from the facts table.
    let depth: Option<i32> = sqlx::query_scalar("SELECT inference_depth FROM facts WHERE id = ?")
        .bind(fact_id)
        .fetch_optional(pool)
        .await?;

    let depth = depth.unwrap_or(0);

    Ok(inference_confidence(&parents, depth, num_parents))
}

// ---------------------------------------------------------------------------
// Eager bounded cascade
// ---------------------------------------------------------------------------

/// Recalculate confidence for all inferred children of `changed_fact_id`
/// and cascade the change down the graph.
///
/// `depth_budget` limits recursion depth to prevent runaway cascades.
/// TODO(#51-followup): replace with async background worker when system
/// work queue is implemented.
pub async fn cascade_confidence_change(
    pool: &SqlitePool,
    changed_fact_id: i32,
    depth_budget: u8,
) -> Result<(), KnowledgeError> {
    cascade_inner(pool, changed_fact_id, depth_budget).await
}

fn cascade_inner<'a>(
    pool: &'a SqlitePool,
    changed_fact_id: i32,
    depth_budget: u8,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), KnowledgeError>> + 'a>> {
    Box::pin(async move {
        if depth_budget == 0 {
            return Ok(());
        }

        let children: Vec<(i32, i32)> = sqlx::query_as(
            "SELECT fd.child_fact_id, f.inference_depth \
             FROM fact_dependencies fd \
             JOIN facts f ON f.id = fd.child_fact_id \
             WHERE fd.parent_fact_id = ? AND fd.relation_type_id = ?",
        )
        .bind(changed_fact_id)
        .bind(crate::models::enums::RelationType::InferredFrom as i16)
        .fetch_all(pool)
        .await?;

        for (child_id, _child_depth) in children {
            let new_confidence = recalculate(pool, child_id).await?;

            let old_confidence: Option<f32> =
                sqlx::query_scalar("SELECT confidence FROM facts WHERE id = ?")
                    .bind(child_id)
                    .fetch_optional(pool)
                    .await?;

            let old_confidence = old_confidence.unwrap_or(0.0);

            if (new_confidence - old_confidence).abs() > 0.001 {
                sqlx::query("UPDATE facts SET confidence = ? WHERE id = ?")
                    .bind(new_confidence)
                    .bind(child_id)
                    .execute(pool)
                    .await?;

                cascade_inner(pool, child_id, depth_budget - 1).await?;
            }
        }

        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Connector reliability
// ---------------------------------------------------------------------------

/// Adjust a connector's reliability score by `delta`.
///
/// The score is clamped to [0.0, 1.0].  Only future extractions are
/// affected; existing fact confidence is never retroactively changed.
pub async fn adjust_connector_reliability(
    pool: &SqlitePool,
    connector_type: ConnectorType,
    delta: f32,
) -> Result<(), KnowledgeError> {
    let old: Option<f32> =
        sqlx::query_scalar("SELECT score FROM connector_reliability WHERE connector_type_id = ?")
            .bind(connector_type as i16)
            .fetch_optional(pool)
            .await?;

    let new_score = old.unwrap_or_else(|| default_connector_score(connector_type)) + delta;
    let new_score = new_score.clamp(0.0, 1.0);

    sqlx::query(
        "INSERT INTO connector_reliability (connector_type_id, score) \
         VALUES (?, ?) \
         ON CONFLICT(connector_type_id) DO UPDATE SET score = excluded.score",
    )
    .bind(connector_type as i16)
    .bind(new_score)
    .execute(pool)
    .await?;

    Ok(())
}

/// Load the current reliability score for a connector.
pub async fn connector_reliability(
    pool: &SqlitePool,
    connector_type: ConnectorType,
) -> Result<f32, KnowledgeError> {
    let score: Option<f32> =
        sqlx::query_scalar("SELECT score FROM connector_reliability WHERE connector_type_id = ?")
            .bind(connector_type as i16)
            .fetch_optional(pool)
            .await?;

    Ok(score.unwrap_or_else(|| default_connector_score(connector_type)))
}
