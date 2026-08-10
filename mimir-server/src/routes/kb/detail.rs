//! KB show + edit handlers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    response::Response,
};

use mimir_api_types::{
    AuditRow, DependencyRow, FactDetailResponse, FactEditRequest, FactEditResponse, FactRow,
    SourceRow,
};
use mimir_knowledge::models::audit_log::ChangedBy;

use crate::error;
use crate::routes::kb::helpers::{
    change_type_name, changed_by_name, fact_row_from, parse_datetime, parse_status,
    source_type_name, status_name,
};
use crate::state::AppState;

pub async fn kb_show_handler(
    State(state): State<Arc<AppState>>,
    Path(fact_id): Path<i32>,
) -> Result<Json<FactDetailResponse>, Response> {
    let fact = state
        .knowledge_graph
        .get_fact(fact_id)
        .await
        .map_err(error::knowledge_error)?
        .ok_or_else(|| error::not_found(format!("fact {} not found", fact_id)))?;

    let subject = state
        .knowledge_graph
        .get_entity(fact.subject_id)
        .await
        .map_err(error::knowledge_error)?
        .map(|e| e.name)
        .unwrap_or_else(|| "(deleted)".to_string());

    let predicate = state
        .knowledge_graph
        .relationship_type_name(fact.relationship_type_id)
        .await
        .unwrap_or_else(|| "unknown".to_string());

    let object = if let Some(oid) = fact.object_id {
        state
            .knowledge_graph
            .get_entity(oid)
            .await
            .map_err(error::knowledge_error)?
            .map(|e| e.name)
    } else {
        fact.object_literal.clone()
    };

    let sources = state
        .knowledge_graph
        .get_sources_for_fact(fact_id)
        .await
        .map_err(error::knowledge_error)?
        .into_iter()
        .map(|s| SourceRow {
            source_type: source_type_name(s.source_type_id),
            connector_instance_id: s.connector_instance_id,
            raw_reference: s.raw_reference,
            extracted_at: s.extracted_at.to_rfc3339(),
        })
        .collect();

    let deps = state
        .knowledge_graph
        .get_fact_dependencies(fact_id)
        .await
        .map_err(error::knowledge_error)?;

    let mut dependencies = Vec::with_capacity(deps.len());
    for (parent, child, rt_id) in deps {
        let rt_name = state
            .knowledge_graph
            .relationship_type_name(rt_id)
            .await
            .unwrap_or_else(|| "unknown".to_string());
        dependencies.push(DependencyRow {
            relation_type: rt_name,
            parent_fact_id: parent,
            child_fact_id: child,
        });
    }

    let audit = state
        .knowledge_graph
        .get_audit_log(fact_id)
        .await
        .map_err(error::knowledge_error)?
        .into_iter()
        .map(|a| AuditRow {
            audit_id: a.id,
            fact_id: a.fact_id,
            change_type: change_type_name(a.change_type_id),
            entity_name: Some(subject.clone()),
            predicate_name: Some(predicate.clone()),
            old_value: a.old_value,
            new_value: a.new_value,
            changed_at: a.changed_at.to_rfc3339(),
            changed_by: changed_by_name(a.changed_by_id),
            reason: a.reason,
        })
        .collect();

    let fact_row = FactRow {
        id: fact.id,
        subject,
        predicate,
        object,
        confidence: fact.confidence,
        status: status_name(fact.fact_status_id),
        valid_from: fact.valid_from.map(|dt| dt.to_rfc3339()),
        valid_until: fact.valid_until.map(|dt| dt.to_rfc3339()),
        inferred: fact.inferred,
    };

    Ok(Json(FactDetailResponse {
        fact: fact_row,
        sources,
        dependencies,
        audit_log: audit,
    }))
}

pub async fn kb_edit_handler(
    State(state): State<Arc<AppState>>,
    Path(fact_id): Path<i32>,
    Json(body): Json<FactEditRequest>,
) -> Result<Json<FactEditResponse>, Response> {
    let status = if let Some(ref s) = body.status {
        Some(parse_status(s).ok_or_else(error::json_rejection)?)
    } else {
        None
    };

    let valid_from = body.valid_from.as_deref().and_then(parse_datetime);
    if body.valid_from.is_some() && valid_from.is_none() {
        return Err(error::json_rejection());
    }

    let valid_until = body.valid_until.as_deref().and_then(parse_datetime);
    if body.valid_until.is_some() && valid_until.is_none() {
        return Err(error::json_rejection());
    }

    let updated = state
        .knowledge_graph
        .update_fact(
            fact_id,
            body.confidence,
            valid_from,
            valid_until,
            body.object_literal.clone(),
            status,
            ChangedBy::User,
        )
        .await
        .map_err(error::knowledge_error)?;

    let fact_row = fact_row_from(&state, &updated).await?;

    Ok(Json(FactEditResponse { fact: fact_row }))
}
