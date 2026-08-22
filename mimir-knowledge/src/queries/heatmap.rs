//! Knowledge-graph heatmap aggregates (issue #69).
//!
//! One snapshot query set powering `mimir kb heatmap`: totals, entity
//! density, predicate distribution, temporal distribution (by month), and
//! confidence bands. Trashed (forgotten) facts are excluded everywhere so
//! the heatmap reflects the live knowledge graph, not the trash.

use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::fact::FactStatus;

/// One ranked (entity | predicate) count.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct HeatmapCount {
    pub name: String,
    pub count: i64,
}

/// One month bucket in the temporal distribution.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct HeatmapTemporal {
    pub period: String,
    pub count: i64,
}

/// One confidence band in the distribution.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct HeatmapBand {
    pub label: String,
    pub count: i64,
}

/// Full heatmap snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct HeatmapData {
    /// Live (non-forgotten) fact count.
    pub facts: i64,
    /// Entity count.
    pub entities: i64,
    /// Mean confidence over live facts (0.0 when empty).
    pub avg_confidence: f32,
    /// Top entities by fact count, then name (ascending).
    pub top_entities: Vec<HeatmapCount>,
    /// Top predicates by fact count, then name (ascending).
    pub predicates: Vec<HeatmapCount>,
    /// Facts per `YYYY-MM` period, ascending.
    pub temporal: Vec<HeatmapTemporal>,
    /// Facts per confidence band in fixed order (explicit, connector,
    /// inference, casual).
    pub confidence_bands: Vec<HeatmapBand>,
}

/// Status id of forgotten (trashed) facts — excluded from every aggregate.
const FORGOTTEN_STATUS_ID: i16 = FactStatus::Forgotten as i16;
/// How many ranked rows `kb heatmap` shows for entities and predicates.
const TOP_N: i64 = 10;

/// Compute the full heatmap snapshot.
pub async fn heatmap(pool: &SqlitePool) -> Result<HeatmapData, KnowledgeError> {
    // Read every aggregate inside one transaction so a concurrent write
    // cannot interleave between statements and mix graph states.
    let mut tx = pool.begin().await?;

    let facts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts WHERE fact_status_id <> ?")
        .bind(FORGOTTEN_STATUS_ID)
        .fetch_one(&mut *tx)
        .await?;

    let entities: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entities")
        .fetch_one(&mut *tx)
        .await?;

    let avg_confidence: f64 = sqlx::query_scalar(
        "SELECT COALESCE(AVG(confidence), 0.0) FROM facts WHERE fact_status_id <> ?",
    )
    .bind(FORGOTTEN_STATUS_ID)
    .fetch_one(&mut *tx)
    .await?;

    let top_entities = sqlx::query_as::<_, HeatmapCount>(
        "SELECT e.name AS name, COUNT(*) AS count \
         FROM facts f JOIN entities e ON e.id = f.subject_id \
         WHERE f.fact_status_id <> ? \
         GROUP BY e.id ORDER BY count DESC, e.name ASC LIMIT ?",
    )
    .bind(FORGOTTEN_STATUS_ID)
    .bind(TOP_N)
    .fetch_all(&mut *tx)
    .await?;

    let predicates = sqlx::query_as::<_, HeatmapCount>(
        "SELECT rt.name AS name, COUNT(*) AS count \
         FROM facts f JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         WHERE f.fact_status_id <> ? \
         GROUP BY rt.id ORDER BY count DESC, rt.name ASC LIMIT ?",
    )
    .bind(FORGOTTEN_STATUS_ID)
    .bind(TOP_N)
    .fetch_all(&mut *tx)
    .await?;

    let temporal = sqlx::query_as::<_, HeatmapTemporal>(
        "SELECT strftime('%Y-%m', COALESCE(f.valid_from, f.created_at)) AS period, \
         COUNT(*) AS count FROM facts f WHERE f.fact_status_id <> ? \
         GROUP BY period ORDER BY period ASC",
    )
    .bind(FORGOTTEN_STATUS_ID)
    .fetch_all(&mut *tx)
    .await?;

    let confidence_bands = sqlx::query_as::<_, HeatmapBand>(
        "SELECT CASE \
           WHEN confidence = 1.0 THEN 'explicit (1.0)' \
           WHEN confidence >= 0.7 THEN 'connector (0.7-1.0)' \
           WHEN confidence >= 0.4 THEN 'inference (0.4-0.7)' \
           ELSE 'casual (<0.4)' \
         END AS label, COUNT(*) AS count \
         FROM facts WHERE fact_status_id <> ? \
         GROUP BY label",
    )
    .bind(FORGOTTEN_STATUS_ID)
    .fetch_all(&mut *tx)
    .await?;

    let confidence_bands = order_confidence_bands(confidence_bands);
    tx.commit().await?;

    Ok(HeatmapData {
        facts,
        entities,
        avg_confidence: avg_confidence as f32,
        top_entities,
        predicates,
        temporal,
        confidence_bands,
    })
}

/// Reorder the SQL `GROUP BY` band rows into the fixed display order
/// (explicit, connector, inference, casual), zero-filling missing bands.
fn order_confidence_bands(rows: Vec<HeatmapBand>) -> Vec<HeatmapBand> {
    let expected = [
        "explicit (1.0)",
        "connector (0.7-1.0)",
        "inference (0.4-0.7)",
        "casual (<0.4)",
    ];
    expected
        .iter()
        .map(|label| {
            rows.iter()
                .find(|b| b.label == *label)
                .cloned()
                .unwrap_or_else(|| HeatmapBand {
                    label: (*label).to_string(),
                    count: 0,
                })
        })
        .collect()
}
