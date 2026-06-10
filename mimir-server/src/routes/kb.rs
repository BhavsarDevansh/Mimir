use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use chrono::Utc;
use serde::Deserialize;

use mimir_api_types::{
    AuditQueryResponse, AuditRow, BrowseEdge, BrowseResponse, DependencyRow, FactDetailResponse,
    FactEditRequest, FactEditResponse, FactQueryResponse, FactRow, ForgetRequest, ForgetResponse,
    ProfileGroup, ProfileResponse, RestoreRequest, RestoreResponse, SourceRow, TrashListResponse,
    TrashRow,
};

use mimir_knowledge::models::audit_log::{ChangeType, ChangedBy};
use mimir_knowledge::models::fact::FactStatus;

use crate::error;
use crate::state::AppState;

// ------------------------------------------------------------------
// Existing optimization handlers (preserved)
// ------------------------------------------------------------------

use mimir_api_types::{
    OptimizationRunNowResponse, OptimizationRunSummary, OptimizationStatusResponse,
};

/// GET /kb/optimization/status
pub async fn kb_optimization_status_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OptimizationStatusResponse>, StatusCode> {
    let status = state
        .job_queue
        .status("knowledge.optimization")
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch optimization status: {}", e);
            if e.is_not_registered() {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    Ok(Json(OptimizationStatusResponse {
        job_id: status.job_id,
        priority: format!("{:?}", status.priority).to_lowercase(),
        schedule: status.schedule.map(|s| s.as_hhmm()),
        next_run_at: status.next_run_at.map(|dt| dt.to_rfc3339()),
        last_run: status.last_run.map(|run| OptimizationRunSummary {
            run_id: run.run_id,
            status: format!("{:?}", run.status).to_lowercase(),
            started_at: run.started_at.to_rfc3339(),
            finished_at: run.finished_at.map(|dt| dt.to_rfc3339()),
            error: run.error,
        }),
    }))
}

/// POST /kb/optimization/run-now
pub async fn kb_optimization_run_now_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OptimizationRunNowResponse>, StatusCode> {
    let summary = state
        .job_queue
        .run_now("knowledge.optimization")
        .await
        .map_err(|e| {
            tracing::error!("Failed to run optimization: {}", e);
            if e.is_not_registered() {
                StatusCode::NOT_FOUND
            } else if e.is_already_running() {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })?;

    Ok(Json(OptimizationRunNowResponse {
        run_id: summary.run_id,
        status: format!("{:?}", summary.status).to_lowercase(),
        started_at: summary.started_at.to_rfc3339(),
        finished_at: summary.finished_at.map(|dt| dt.to_rfc3339()),
        error: summary.error,
    }))
}

// ------------------------------------------------------------------
// Query parameters
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct QueryParams {
    entity: String,
    predicate: Option<String>,
    min_confidence: Option<f32>,
    offset: Option<u32>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQueryParams {
    entity: String,
    #[serde(default = "default_depth")]
    depth: u32,
    offset: Option<u32>,
    limit: Option<u32>,
}

fn default_depth() -> u32 {
    2
}

#[derive(Debug, Deserialize)]
pub struct ProfileQueryParams {
    entity: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuditQueryParams {
    entity: Option<String>,
    predicate: Option<String>,
    from: Option<String>,
    to: Option<String>,
    change_type: Option<String>,
    offset: Option<u32>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct TrashQueryParams {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_trash_limit")]
    limit: u32,
}

fn default_trash_limit() -> u32 {
    50
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn parse_datetime(s: &str) -> Option<chrono::DateTime<Utc>> {
    if let Ok(dt) = s.parse::<chrono::DateTime<Utc>>() {
        return Some(dt);
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
    ] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc());
        }
    }
    None
}

fn parse_status(s: &str) -> Option<FactStatus> {
    match s.to_lowercase().as_str() {
        "active" => Some(FactStatus::Active),
        "inferred" => Some(FactStatus::Inferred),
        "disputed" => Some(FactStatus::Disputed),
        "corrected" => Some(FactStatus::Corrected),
        "superseded" => Some(FactStatus::Superseded),
        "forgotten" => Some(FactStatus::Forgotten),
        _ => None,
    }
}

fn status_name(status_id: i16) -> String {
    match status_id {
        1 => "Active",
        2 => "Inferred",
        3 => "Disputed",
        4 => "Corrected",
        5 => "Superseded",
        6 => "Forgotten",
        _ => "Unknown",
    }
    .to_string()
}

fn source_type_name(source_type_id: i16) -> String {
    match source_type_id {
        1 => "UserEdit",
        2 => "Connector",
        3 => "Inference",
        4 => "Interaction",
        5 => "Import",
        6 => "System",
        _ => "Unknown",
    }
    .to_string()
}

fn change_type_name(change_type_id: i16) -> String {
    match change_type_id {
        1 => "created",
        2 => "status_change",
        3 => "confidence_change",
        4 => "temporal_update",
        5 => "source_added",
        6 => "forgotten",
        7 => "restored",
        8 => "rejected",
        _ => "Unknown",
    }
    .to_string()
}

fn changed_by_name(changed_by_id: Option<i16>) -> Option<String> {
    changed_by_id.map(|id| {
        match id {
            1 => "User",
            2 => "System",
            3 => "InferenceEngine",
            4 => "NightlyOptimization",
            _ => "Unknown",
        }
        .to_string()
    })
}

async fn resolve_entity_id(
    kg: &mimir_knowledge::KnowledgeGraph,
    name: &str,
) -> Result<i32, Response> {
    let mut results = kg
        .search_entities(name, 1)
        .await
        .map_err(error::knowledge_error)?;
    if results.is_empty() {
        return Err(error::not_found(format!("entity '{}' not found", name)));
    }
    Ok(results.remove(0).entity.id)
}

// ------------------------------------------------------------------
// kb query
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// kb show
// ------------------------------------------------------------------

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
            connector_id: s.connector_id,
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

// ------------------------------------------------------------------
// kb edit
// ------------------------------------------------------------------

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

    let subject = state
        .knowledge_graph
        .get_entity(updated.subject_id)
        .await
        .map_err(error::knowledge_error)?
        .map(|e| e.name)
        .unwrap_or_else(|| "(deleted)".to_string());

    let predicate = state
        .knowledge_graph
        .relationship_type_name(updated.relationship_type_id)
        .await
        .unwrap_or_else(|| "unknown".to_string());

    let object = if let Some(oid) = updated.object_id {
        state
            .knowledge_graph
            .get_entity(oid)
            .await
            .map_err(error::knowledge_error)?
            .map(|e| e.name)
    } else {
        updated.object_literal.clone()
    };

    Ok(Json(FactEditResponse {
        fact: FactRow {
            id: updated.id,
            subject,
            predicate,
            object,
            confidence: updated.confidence,
            status: status_name(updated.fact_status_id),
            valid_from: updated.valid_from.map(|dt| dt.to_rfc3339()),
            valid_until: updated.valid_until.map(|dt| dt.to_rfc3339()),
            inferred: updated.inferred,
        },
    }))
}

// ------------------------------------------------------------------
// kb browse
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// kb profile
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// kb audit
// ------------------------------------------------------------------

pub async fn kb_audit_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditQueryParams>,
) -> Result<Json<AuditQueryResponse>, Response> {
    let ct = params
        .change_type
        .as_deref()
        .and_then(|s| match s.to_lowercase().as_str() {
            "created" => Some(ChangeType::Created),
            "status_change" => Some(ChangeType::StatusChange),
            "confidence_change" => Some(ChangeType::ConfidenceChange),
            "temporal_update" => Some(ChangeType::TemporalUpdate),
            "source_added" => Some(ChangeType::SourceAdded),
            "forgotten" => Some(ChangeType::Forgotten),
            "restored" => Some(ChangeType::Restored),
            "rejected" => Some(ChangeType::Rejected),
            "content_update" => Some(ChangeType::ContentUpdate),
            _ => None,
        });

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

// ------------------------------------------------------------------
// kb forget
// ------------------------------------------------------------------

pub async fn kb_forget_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForgetRequest>,
) -> Result<Json<ForgetResponse>, Response> {
    let filters = mimir_knowledge::forget::ForgetFilters {
        fact_id: body.fact_id,
        predicate: body.predicate,
        subject: body.subject,
        entity: body.entity,
        source: body.source,
        from: body.from.as_deref().and_then(parse_datetime),
        to: body.to.as_deref().and_then(parse_datetime),
        all: body.all,
    };

    let opts = mimir_knowledge::forget::ForgetOptions {
        yes: body.yes,
        confirm_sensitive: body.confirm_sensitive,
        confirmation_phrase: body.confirmation_phrase,
        archive: body.archive,
    };

    let result = state
        .knowledge_graph
        .forget_facts(filters, opts, ChangedBy::User)
        .await
        .map_err(error::knowledge_error)?;

    Ok(Json(ForgetResponse {
        forgotten_count: result.forgotten_count,
        backup_path: result.backup_path.map(|p| p.to_string_lossy().to_string()),
    }))
}

// ------------------------------------------------------------------
// kb trash list
// ------------------------------------------------------------------

pub async fn kb_trash_list_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TrashQueryParams>,
) -> Result<Json<TrashListResponse>, Response> {
    let items = state
        .knowledge_graph
        .list_trash(params.limit as i64, params.offset as i64)
        .await
        .map_err(error::knowledge_error)?;

    let total = items.len() as i64; // approximate
    let rows: Vec<TrashRow> = items
        .into_iter()
        .map(|i| TrashRow {
            trash_id: i.trash_id,
            subject: i.subject_name,
            predicate: i.relationship_type_name,
            object: i.object_name.or(i.object_literal),
            deleted_at: i.deleted_at.to_rfc3339(),
            expires_at: i.expires_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(TrashListResponse {
        total,
        offset: params.offset,
        limit: params.limit,
        items: rows,
    }))
}

// ------------------------------------------------------------------
// kb trash restore
// ------------------------------------------------------------------

pub async fn kb_trash_restore_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RestoreRequest>,
) -> Result<Json<RestoreResponse>, Response> {
    if body.all {
        let facts = state
            .knowledge_graph
            .restore_all(ChangedBy::User)
            .await
            .map_err(error::knowledge_error)?;
        Ok(Json(RestoreResponse {
            restored_count: facts.len(),
        }))
    } else {
        let id = body.trash_id.ok_or_else(error::json_rejection)?;
        let _fact = state
            .knowledge_graph
            .restore_fact(id, ChangedBy::User)
            .await
            .map_err(error::knowledge_error)?;
        Ok(Json(RestoreResponse { restored_count: 1 }))
    }
}

// ------------------------------------------------------------------
// kb trash empty
// ------------------------------------------------------------------

pub async fn kb_trash_empty_handler(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, Response> {
    state
        .knowledge_graph
        .empty_trash()
        .await
        .map_err(error::knowledge_error)?;
    Ok(StatusCode::NO_CONTENT)
}
