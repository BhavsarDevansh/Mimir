use serde::{Deserialize, Serialize};
// ---------------------------------------------------------------------------
// Knowledge Graph — CLI types
// ---------------------------------------------------------------------------

/// Request to query facts for an entity.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FactQueryParams {
    pub entity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A single fact in query results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactRow {
    pub id: i32,
    pub subject: String,
    pub predicate: String,
    pub object: Option<String>,
    pub confidence: f32,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    pub inferred: bool,
}

/// Response for fact queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactQueryResponse {
    pub total: i64,
    pub offset: u32,
    pub limit: u32,
    pub facts: Vec<FactRow>,
}

/// Source attached to a fact (detail view).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceRow {
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector_instance_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_reference: Option<String>,
    pub extracted_at: String,
}

/// Dependency edge for a fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyRow {
    pub relation_type: String,
    pub parent_fact_id: i32,
    pub child_fact_id: i32,
}

/// Audit log entry for a fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditRow {
    pub audit_id: i32,
    pub fact_id: i32,
    pub change_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<String>,
    pub changed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Detailed view of a single fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactDetailResponse {
    pub fact: FactRow,
    pub sources: Vec<SourceRow>,
    pub dependencies: Vec<DependencyRow>,
    pub audit_log: Vec<AuditRow>,
}

/// Request to edit a fact.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FactEditRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_literal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Response after editing a fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FactEditResponse {
    pub fact: FactRow,
}

/// Request to browse the knowledge graph.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BrowseRequest {
    pub entity: String,
    #[serde(default = "default_browse_depth")]
    pub depth: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

fn default_browse_depth() -> u32 {
    2
}

/// A single edge in a browse traversal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowseEdge {
    pub depth: u32,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
}

/// Response for browse queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowseResponse {
    pub total_edges: usize,
    pub offset: u32,
    pub limit: u32,
    pub edges: Vec<BrowseEdge>,
}

/// A category in the knowledge graph taxonomy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CategoryResponse {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_weight: Option<f32>,
    /// Memory bucket id (`memory_buckets` lookup); `None` classifies as General.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bucket_id: Option<i16>,
}

/// A category with its child categories and fact count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CategoryDetailResponse {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_weight: Option<f32>,
    /// Memory bucket id (`memory_buckets` lookup); `None` classifies as General.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bucket_id: Option<i16>,
    pub fact_count: i64,
    pub children: Vec<CategoryResponse>,
}
/// Request to generate a profile.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ProfileRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
}

/// A group of facts in a profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileGroup {
    pub category: String,
    pub facts: Vec<FactRow>,
}

/// Response for profile queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileResponse {
    pub entity_name: String,
    pub groups: Vec<ProfileGroup>,
}

/// Request to query the audit log.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AuditQueryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Response for audit log queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditQueryResponse {
    pub total: i64,
    pub offset: u32,
    pub limit: u32,
    pub entries: Vec<AuditRow>,
}

/// Request to forget facts.
#[cfg(test)]
mod tests {
    use super::*;

    // Round-trip helper: serialise then deserialise must yield an equal value.
    fn roundtrip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialise");
        serde_json::from_str(&json).expect("deserialise")
    }

    // Macro: declare a round-trip test for one struct, covering both
    // populated and `Option::None` (skip-serialising) forms.
    macro_rules! roundtrip_tests {
        ($name:ident, full: $full:expr, sparse: $sparse:expr, sparse_skips: [$($skip:literal),* $(,)?]) => {
            #[test]
            fn $name() {
                assert_eq!(roundtrip(&$full), $full);
                assert_eq!(roundtrip(&$sparse), $sparse);
                let json = serde_json::to_string(&$sparse).expect("serialise sparse");
                let obj = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
                    .expect("parse sparse json object");
                $(
                    assert!(
                        !obj.contains_key($skip),
                        "sparse form should not serialise `{}` (got: {json})",
                        $skip,
                    );
                )*
                // Keep `json` and `obj` consumed even when `sparse_skips` is empty
                // so the macro never emits unused-variable warnings.
                let _ = (&json, &obj);
            }
        };
    }

    fn sample_fact_row() -> FactRow {
        FactRow {
            id: 7,
            subject: "Alice".to_string(),
            predicate: "lives_in".to_string(),
            object: Some("London".to_string()),
            confidence: 0.9,
            status: "active".to_string(),
            valid_from: Some("2020-01-01T00:00:00Z".to_string()),
            valid_until: None,
            inferred: false,
        }
    }

    roundtrip_tests!(
        fact_query_params,
        full: FactQueryParams {
            entity: "Alice".to_string(),
            predicate: Some("lives_in".to_string()),
            min_confidence: Some(0.5),
            offset: Some(10),
            limit: Some(20),
        },
        sparse: FactQueryParams {
            entity: "Bob".to_string(),
            predicate: None,
            min_confidence: None,
            offset: None,
            limit: None,
        },
        sparse_skips: ["predicate", "min_confidence", "offset", "limit"]
    );

    #[test]
    fn fact_row_roundtrip() {
        let row = sample_fact_row();
        assert_eq!(roundtrip(&row), row);
    }

    #[test]
    fn fact_query_response_roundtrip() {
        let resp = FactQueryResponse {
            total: 2,
            offset: 0,
            limit: 10,
            facts: vec![sample_fact_row()],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        source_row,
        full: SourceRow {
            source_type: "chat".to_string(),
            connector_instance_id: Some(1),
            raw_reference: Some("ref-1".to_string()),
            extracted_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse: SourceRow {
            source_type: "chat".to_string(),
            connector_instance_id: None,
            raw_reference: None,
            extracted_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse_skips: ["connector_instance_id", "raw_reference"]
    );

    #[test]
    fn dependency_row_roundtrip() {
        let row = DependencyRow {
            relation_type: "transitive".to_string(),
            parent_fact_id: 1,
            child_fact_id: 2,
        };
        assert_eq!(roundtrip(&row), row);
    }

    roundtrip_tests!(
        audit_row,
        full: AuditRow {
            audit_id: 9,
            fact_id: 1,
            change_type: "status_change".to_string(),
            entity_name: Some("Alice".to_string()),
            predicate_name: Some("lives_in".to_string()),
            old_value: Some("Paris".to_string()),
            new_value: Some("London".to_string()),
            changed_at: "2020-01-01T00:00:00Z".to_string(),
            changed_by: Some("User".to_string()),
            reason: Some("correction".to_string()),
        },
        sparse: AuditRow {
            audit_id: 9,
            fact_id: 1,
            change_type: "status_change".to_string(),
            entity_name: None,
            predicate_name: None,
            old_value: None,
            new_value: None,
            changed_at: "2020-01-01T00:00:00Z".to_string(),
            changed_by: None,
            reason: None,
        },
        sparse_skips: [
            "old_value",
            "new_value",
            "reason",
            "entity_name",
            "predicate_name",
            "changed_by"
        ]
    );

    #[test]
    fn fact_detail_response_roundtrip() {
        let detail = FactDetailResponse {
            fact: sample_fact_row(),
            sources: vec![SourceRow {
                source_type: "chat".to_string(),
                connector_instance_id: None,
                raw_reference: None,
                extracted_at: "2020-01-01T00:00:00Z".to_string(),
            }],
            dependencies: vec![],
            audit_log: vec![],
        };
        assert_eq!(roundtrip(&detail), detail);
    }

    roundtrip_tests!(
        fact_edit_request,
        full: FactEditRequest {
            confidence: Some(0.8),
            valid_from: Some("2020-01-01T00:00:00Z".to_string()),
            valid_until: None,
            object_literal: Some("London".to_string()),
            status: Some("active".to_string()),
        },
        sparse: FactEditRequest {
            confidence: None,
            valid_from: None,
            valid_until: None,
            object_literal: None,
            status: None,
        },
        sparse_skips: [
            "confidence",
            "valid_from",
            "valid_until",
            "object_literal",
            "status"
        ]
    );

    #[test]
    fn fact_edit_response_roundtrip() {
        let resp = FactEditResponse {
            fact: sample_fact_row(),
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn browse_request_default_depth_is_applied() {
        let json = r#"{"entity":"Alice"}"#;
        let parsed: BrowseRequest = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.entity, "Alice");
        assert_eq!(parsed.depth, 2);
        assert_eq!(parsed.offset, None);
        assert_eq!(parsed.limit, None);
    }

    roundtrip_tests!(
        browse_request,
        full: BrowseRequest {
            entity: "Alice".to_string(),
            depth: 3,
            offset: Some(0),
            limit: Some(10),
        },
        sparse: BrowseRequest {
            entity: "Alice".to_string(),
            depth: 2,
            offset: None,
            limit: None,
        },
        sparse_skips: ["offset", "limit"]
    );

    #[test]
    fn browse_edge_and_response_roundtrip() {
        let edge = BrowseEdge {
            depth: 1,
            subject: "Alice".to_string(),
            predicate: "lives_in".to_string(),
            object: "London".to_string(),
            confidence: 0.9,
        };
        assert_eq!(roundtrip(&edge), edge);

        let resp = BrowseResponse {
            total_edges: 1,
            offset: 0,
            limit: 10,
            edges: vec![edge],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        category_response,
        full: CategoryResponse {
            id: 1,
            name: "people".to_string(),
            description: Some("Humans".to_string()),
            parent_id: Some(0),
            memory_weight: Some(0.8),
            memory_bucket_id: Some(3),
        },
        sparse: CategoryResponse {
            id: 1,
            name: "people".to_string(),
            description: None,
            parent_id: None,
            memory_weight: None,
            memory_bucket_id: None,
        },
        sparse_skips: ["description", "parent_id", "memory_weight", "memory_bucket_id"]
    );

    #[test]
    fn category_detail_response_roundtrip() {
        let detail = CategoryDetailResponse {
            id: 1,
            name: "people".to_string(),
            description: Some("Humans".to_string()),
            parent_id: None,
            memory_weight: Some(0.8),
            memory_bucket_id: Some(4),
            fact_count: 5,
            children: vec![CategoryResponse {
                id: 2,
                name: "friends".to_string(),
                description: None,
                parent_id: Some(1),
                memory_weight: None,
                memory_bucket_id: None,
            }],
        };
        assert_eq!(roundtrip(&detail), detail);
    }

    roundtrip_tests!(
        profile_request,
        full: ProfileRequest {
            entity: Some("Alice".to_string()),
        },
        sparse: ProfileRequest { entity: None },
        sparse_skips: ["entity"]
    );

    #[test]
    fn profile_response_roundtrip() {
        let resp = ProfileResponse {
            entity_name: "Alice".to_string(),
            groups: vec![ProfileGroup {
                category: "personal".to_string(),
                facts: vec![sample_fact_row()],
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        audit_query_request,
        full: AuditQueryRequest {
            entity: Some("Alice".to_string()),
            predicate: Some("lives_in".to_string()),
            from: Some("2020-01-01T00:00:00Z".to_string()),
            to: Some("2021-01-01T00:00:00Z".to_string()),
            change_type: Some("status_change".to_string()),
            offset: Some(0),
            limit: Some(10),
        },
        sparse: AuditQueryRequest {
            entity: None,
            predicate: None,
            from: None,
            to: None,
            change_type: None,
            offset: None,
            limit: None,
        },
        sparse_skips: [
            "entity",
            "predicate",
            "from",
            "to",
            "change_type",
            "offset",
            "limit"
        ]
    );

    #[test]
    fn audit_query_response_roundtrip() {
        let resp = AuditQueryResponse {
            total: 1,
            offset: 0,
            limit: 10,
            entries: vec![AuditRow {
                audit_id: 1,
                fact_id: 1,
                change_type: "status_change".to_string(),
                entity_name: Some("Alice".to_string()),
                predicate_name: Some("lives_in".to_string()),
                old_value: None,
                new_value: Some("London".to_string()),
                changed_at: "2020-01-01T00:00:00Z".to_string(),
                changed_by: None,
                reason: None,
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
    }
}
