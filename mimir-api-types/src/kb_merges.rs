//! Knowledge Graph — entity merge-queue review wire types (issue #282).

use serde::{Deserialize, Serialize};

/// A pending entity-merge suggestion awaiting human review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityMergeQueueRow {
    pub id: i64,
    pub primary_entity_id: i32,
    pub primary_name: String,
    pub primary_type: String,
    pub duplicate_entity_id: i32,
    pub duplicate_name: String,
    pub duplicate_type: String,
    /// LLM recommendation (`merge` or `keep_separate`); `None` for rows
    /// flagged deterministically (alias overlap) before LLM evaluation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_action: Option<String>,
    /// LLM confidence in the suggestion; `None` before LLM evaluation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_confidence: Option<f32>,
    pub queued_at: String,
}

/// Response body for `GET /kb/merges`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MergeQueueListResponse {
    pub total: usize,
    pub items: Vec<EntityMergeQueueRow>,
}

/// Response body for `POST /kb/merges/{id}/apply`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MergeApplyResponse {
    pub survivor_id: i32,
    pub merged_id: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> EntityMergeQueueRow {
        EntityMergeQueueRow {
            id: 7,
            primary_entity_id: 1,
            primary_name: "Jane Smith".to_string(),
            primary_type: "Person".to_string(),
            duplicate_entity_id: 2,
            duplicate_name: "Jane Smith-Jones".to_string(),
            duplicate_type: "Person".to_string(),
            suggested_action: Some("merge".to_string()),
            llm_confidence: Some(0.85),
            queued_at: "2026-08-25T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn merge_queue_row_roundtrip() {
        let row = sample_row();
        let json = serde_json::to_string(&row).unwrap();
        let parsed: EntityMergeQueueRow = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, row);
    }

    #[test]
    fn merge_queue_row_sparse_skips_llm_fields() {
        let row = EntityMergeQueueRow {
            suggested_action: None,
            llm_confidence: None,
            ..sample_row()
        };
        let json = serde_json::to_string(&row).unwrap();
        let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert!(!obj.contains_key("suggested_action"));
        assert!(!obj.contains_key("llm_confidence"));
    }

    #[test]
    fn list_and_apply_responses_roundtrip() {
        let list = MergeQueueListResponse {
            total: 1,
            items: vec![sample_row()],
        };
        let json = serde_json::to_string(&list).unwrap();
        assert_eq!(
            serde_json::from_str::<MergeQueueListResponse>(&json).unwrap(),
            list
        );

        let apply = MergeApplyResponse {
            survivor_id: 1,
            merged_id: 2,
        };
        let json = serde_json::to_string(&apply).unwrap();
        assert_eq!(
            serde_json::from_str::<MergeApplyResponse>(&json).unwrap(),
            apply
        );
    }
}
