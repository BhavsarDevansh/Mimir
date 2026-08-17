//! Shared KB route helpers: parsing and name resolution.

use axum::response::Response;
use chrono::Utc;

use mimir_api_types::FactRow;
use mimir_knowledge::models::fact::FactStatus;
use mimir_knowledge::models::source::SourceType;

use crate::error;
use crate::state::AppState;

pub(super) fn parse_datetime(s: &str) -> Option<chrono::DateTime<Utc>> {
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

pub(super) fn parse_status(s: &str) -> Option<FactStatus> {
    s.parse().ok()
}

pub(super) fn status_name(status_id: i16) -> String {
    FactStatus::try_from(status_id)
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|_| "Unknown".to_string())
}

pub(super) fn source_type_name(source_type_id: i16) -> String {
    SourceType::try_from(source_type_id)
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|_| "Unknown".to_string())
}

pub(super) fn change_type_name(change_type_id: i16) -> String {
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

pub(super) fn changed_by_name(changed_by_id: Option<i16>) -> Option<String> {
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

pub(super) async fn resolve_entity_id(
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

/// Build a [`FactRow`] from a [`mimir_knowledge::models::fact::Fact`], resolving
pub(super) async fn fact_row_from(
    state: &AppState,
    fact: &mimir_knowledge::models::fact::Fact,
) -> Result<FactRow, Response> {
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

    Ok(FactRow {
        id: fact.id,
        subject,
        predicate,
        object,
        confidence: fact.confidence,
        status: status_name(fact.fact_status_id),
        valid_from: fact.valid_from.map(|dt| dt.to_rfc3339()),
        valid_until: fact.valid_until.map(|dt| dt.to_rfc3339()),
        inferred: fact.inferred,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_name_matches_wire_contract() {
        assert_eq!(status_name(FactStatus::Active as i16), "Active");
        assert_eq!(status_name(FactStatus::Inferred as i16), "Inferred");
        assert_eq!(status_name(FactStatus::Disputed as i16), "Disputed");
        assert_eq!(status_name(FactStatus::Corrected as i16), "Corrected");
        assert_eq!(status_name(FactStatus::Superseded as i16), "Superseded");
        assert_eq!(status_name(FactStatus::Forgotten as i16), "Forgotten");
        assert_eq!(status_name(99), "Unknown");
    }

    #[test]
    fn source_type_name_matches_wire_contract() {
        assert_eq!(source_type_name(SourceType::UserEdit as i16), "UserEdit");
        assert_eq!(source_type_name(SourceType::Connector as i16), "Connector");
        assert_eq!(source_type_name(SourceType::Inference as i16), "Inference");
        assert_eq!(
            source_type_name(SourceType::Interaction as i16),
            "Interaction"
        );
        assert_eq!(source_type_name(SourceType::Import as i16), "Import");
        assert_eq!(source_type_name(SourceType::System as i16), "System");
        assert_eq!(source_type_name(99), "Unknown");
    }

    #[test]
    fn parse_status_accepts_wire_strings() {
        assert_eq!(parse_status("active"), Some(FactStatus::Active));
        assert_eq!(parse_status("Active"), Some(FactStatus::Active));
        assert_eq!(parse_status("forgotten"), Some(FactStatus::Forgotten));
        assert_eq!(parse_status("bogus"), None);
    }
}
