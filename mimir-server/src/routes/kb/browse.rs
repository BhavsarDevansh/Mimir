//! KB browse, profile, and audit handlers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    response::Response,
};

use crate::error;
use crate::routes::kb::helpers::{parse_datetime, resolve_entity_id, status_name};
use crate::routes::kb::params::{AuditQueryParams, BrowseQueryParams, ProfileQueryParams};
use crate::state::AppState;
use mimir_api_types::{
    AuditQueryResponse, AuditRow, BrowseEdge, BrowseResponse, FactRow, ProfileGroup,
    ProfileResponse,
};

pub async fn kb_browse_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BrowseQueryParams>,
) -> Result<Json<BrowseResponse>, Response> {
    let root_id = resolve_entity_id(&state.knowledge_graph, &params.entity).await?;
    let depth = params.depth.min(5);
    let limit = params.limit.unwrap_or(50).min(500);
    let offset = params.offset.unwrap_or(0) as usize;

    let result = mimir_knowledge::queries::traverse::traverse_graph(
        state.knowledge_graph.pool(),
        root_id as u32,
        depth,
        limit,
        None,
    )
    .await
    .map_err(error::knowledge_error)?;

    let total_edges = result.edges.len();
    let edges: Vec<BrowseEdge> = result
        .edges
        .into_iter()
        .skip(offset)
        .take(limit as usize)
        .map(|e| BrowseEdge {
            depth: e.depth,
            subject: e.subject,
            predicate: e.predicate,
            object: e.object,
            confidence: e.confidence,
        })
        .collect();

    Ok(Json(BrowseResponse {
        total_edges,
        offset: params.offset.unwrap_or(0),
        limit: params.limit.unwrap_or(50),
        edges,
    }))
}

pub async fn kb_profile_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ProfileQueryParams>,
) -> Result<Json<ProfileResponse>, Response> {
    let entity_name = if let Some(ref name) = params.entity {
        name.clone()
    } else if let Some(uid) = state.user_entity_id {
        let e = state
            .knowledge_graph
            .get_entity(uid)
            .await
            .map_err(error::knowledge_error)?
            .ok_or_else(|| error::not_found("user entity not found"))?;
        e.name
    } else {
        return Err(error::not_found("no user entity configured; use --entity"));
    };

    let entity_id = resolve_entity_id(&state.knowledge_graph, &entity_name).await?;

    let facts = state
        .knowledge_graph
        .query_facts(entity_id, None, 0.0, 0, 20)
        .await
        .map_err(error::knowledge_error)?;

    // Group by category (fetch category for each fact).
    let mut groups: std::collections::HashMap<String, Vec<FactRow>> =
        std::collections::HashMap::new();
    for f in facts {
        let predicate = state
            .knowledge_graph
            .relationship_type_name(f.relationship_type_id)
            .await
            .unwrap_or_else(|| "unknown".to_string());
        let object = f.object_name.clone().or(f.object_literal.clone());
        let row = FactRow {
            id: f.id,
            subject: entity_name.clone(),
            predicate: predicate.clone(),
            object,
            confidence: f.confidence,
            status: status_name(f.fact_status_id),
            valid_from: f.valid_from.map(|dt| dt.to_rfc3339()),
            valid_until: f.valid_until.map(|dt| dt.to_rfc3339()),
            inferred: f.inferred,
        };

        let categories = state
            .knowledge_graph
            .get_categories_for_fact(f.id)
            .await
            .map_err(error::knowledge_error)?;
        if categories.is_empty() {
            groups
                .entry("Uncategorized".to_string())
                .or_default()
                .push(row);
        } else {
            for cat in categories {
                groups.entry(cat.name).or_default().push(row.clone());
            }
        }
    }

    let groups: Vec<ProfileGroup> = groups
        .into_iter()
        .map(|(category, facts)| ProfileGroup { category, facts })
        .collect();

    Ok(Json(ProfileResponse {
        entity_name,
        groups,
    }))
}

pub async fn kb_audit_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditQueryParams>,
) -> Result<Json<AuditQueryResponse>, Response> {
    let ct = params.change_type.as_deref().and_then(|s| s.parse().ok());

    let from_dt = params.from.as_deref().and_then(parse_datetime);
    let to_dt = params.to.as_deref().and_then(parse_datetime);

    let filter = mimir_knowledge::queries::audit::AuditLogFilter {
        entity_name: params.entity,
        relationship_type_name: params.predicate,
        from: from_dt,
        to: to_dt,
        change_type: ct,
        limit: params.limit.map(|l| l as i64),
        offset: params.offset.map(|o| o as i64),
    };

    let rows = state
        .knowledge_graph
        .query_audit_log(filter)
        .await
        .map_err(error::knowledge_error)?;

    let total = rows.len() as i64; // approximate without a separate count query
    let entries: Vec<AuditRow> = rows
        .into_iter()
        .map(|r| AuditRow {
            audit_id: r.audit_id,
            fact_id: r.fact_id,
            change_type: r.change_type_name,
            entity_name: r.entity_name,
            predicate_name: r.relationship_type_name,
            old_value: r.old_value,
            new_value: r.new_value,
            changed_at: r.changed_at.to_rfc3339(),
            changed_by: r.changed_by_name,
            reason: r.reason,
        })
        .collect();

    Ok(Json(AuditQueryResponse {
        total,
        offset: params.offset.unwrap_or(0),
        limit: params.limit.unwrap_or(50),
        entries,
    }))
}
