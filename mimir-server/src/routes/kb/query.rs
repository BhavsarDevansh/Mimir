//! KB query handler.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    response::Response,
};

use mimir_api_types::{FactQueryResponse, FactRow};

use crate::error;
use crate::routes::kb::helpers::{resolve_entity_id, status_name};
use crate::routes::kb::params::QueryParams;
use crate::state::AppState;

pub async fn kb_query_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QueryParams>,
) -> Result<Json<FactQueryResponse>, Response> {
    let subject_id = resolve_entity_id(&state.knowledge_graph, &params.entity).await?;

    let relationship_type_id = if let Some(ref pred) = params.predicate {
        Some(
            state
                .knowledge_graph
                .get_relationship_type_id(pred)
                .await
                .map_err(error::knowledge_error)?
                .ok_or_else(|| error::not_found(format!("predicate '{}' not found", pred)))?,
        )
    } else {
        None
    };

    let min_confidence = params.min_confidence.unwrap_or(0.0);
    let offset = params.offset.unwrap_or(0) as i64;
    let limit = params.limit.unwrap_or(50).min(500) as i64;

    let total = state
        .knowledge_graph
        .count_facts(subject_id, relationship_type_id, min_confidence)
        .await
        .map_err(error::knowledge_error)?;

    let facts = state
        .knowledge_graph
        .query_facts(
            subject_id,
            relationship_type_id,
            min_confidence,
            offset,
            limit,
        )
        .await
        .map_err(error::knowledge_error)?;

    let mut rows = Vec::with_capacity(facts.len());
    for f in facts {
        let predicate = state
            .knowledge_graph
            .relationship_type_name(f.relationship_type_id)
            .await
            .unwrap_or_else(|| "unknown".to_string());
        let object = f.object_name.clone().or(f.object_literal.clone());
        rows.push(FactRow {
            id: f.id,
            subject: params.entity.clone(),
            predicate,
            object,
            confidence: f.confidence,
            status: status_name(f.fact_status_id),
            valid_from: f.valid_from.map(|dt| dt.to_rfc3339()),
            valid_until: f.valid_until.map(|dt| dt.to_rfc3339()),
            inferred: f.inferred,
        });
    }

    Ok(Json(FactQueryResponse {
        total,
        offset: params.offset.unwrap_or(0),
        limit: params.limit.unwrap_or(50),
        facts: rows,
    }))
}
