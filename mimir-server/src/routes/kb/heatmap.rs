//! `GET /kb/heatmap` handler (issue #69).

use std::sync::Arc;

use axum::{Json, extract::State, response::Response};

use mimir_api_types::{HeatmapBandRow, HeatmapCountRow, HeatmapResponse, HeatmapTemporalRow};

use crate::error;
use crate::state::AppState;

/// Serve the knowledge-graph density snapshot backing `mimir kb heatmap`.
pub async fn kb_heatmap_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<HeatmapResponse>, Response> {
    let data = state
        .knowledge_graph
        .heatmap()
        .await
        .map_err(error::knowledge_error)?;

    Ok(Json(HeatmapResponse {
        facts: data.facts,
        entities: data.entities,
        avg_confidence: data.avg_confidence,
        top_entities: data
            .top_entities
            .into_iter()
            .map(|e| HeatmapCountRow {
                name: e.name,
                count: e.count,
            })
            .collect(),
        predicates: data
            .predicates
            .into_iter()
            .map(|p| HeatmapCountRow {
                name: p.name,
                count: p.count,
            })
            .collect(),
        temporal: data
            .temporal
            .into_iter()
            .map(|t| HeatmapTemporalRow {
                period: t.period,
                count: t.count,
            })
            .collect(),
        confidence_bands: data
            .confidence_bands
            .into_iter()
            .map(|b| HeatmapBandRow {
                label: b.label,
                count: b.count,
            })
            .collect(),
    }))
}
