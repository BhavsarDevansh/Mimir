use crate::kb::FactRow;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ForgetRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub yes: bool,
    #[serde(default)]
    pub confirm_sensitive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_phrase: Option<String>,
    #[serde(default)]
    pub archive: bool,
}

/// Response after forgetting facts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForgetResponse {
    pub forgotten_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

/// Request to restore facts from trash.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RestoreRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash_id: Option<i32>,
    #[serde(default)]
    pub all: bool,
}

/// Response after restoring facts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestoreResponse {
    pub restored_count: usize,
}

/// A single row in the trash list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrashRow {
    pub trash_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    pub deleted_at: String,
    pub expires_at: String,
}

/// Response for trash list queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrashListResponse {
    pub total: i64,
    pub offset: u32,
    pub limit: u32,
    pub items: Vec<TrashRow>,
}

// ---------------------------------------------------------------------------
// Knowledge Graph — pending sensitive-fact confirmation (issue #141)
// ---------------------------------------------------------------------------

/// A single pending sensitive fact awaiting user confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingFactRow {
    pub fact_id: i32,
    pub subject: String,
    pub predicate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    pub created_at: String,
}

/// Response body for `GET /kb/pending`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingListResponse {
    pub total: usize,
    pub facts: Vec<PendingFactRow>,
}

/// Response body for `POST /kb/facts/{id}/confirm`.
///
/// Wraps the updated fact as a [`FactRow`], consistent with the edit endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfirmFactResponse {
    pub fact: FactRow,
}

/// Request body for `POST /kb/facts/{id}/reject`.
///
/// All fields optional: an empty POST body is valid and yields a `204 No
/// Content`. A `reason`, if supplied, is written to the audit log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RejectFactRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

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

    #[test]
    fn forget_request_defaults() {
        let parsed: ForgetRequest = serde_json::from_str("{}").unwrap();
        assert!(!parsed.all);
        assert!(!parsed.yes);
        assert!(!parsed.confirm_sensitive);
        assert!(!parsed.archive);
        assert_eq!(parsed.fact_id, None);
        assert_eq!(parsed.confirmation_phrase, None);
    }

    roundtrip_tests!(
        forget_request,
        full: ForgetRequest {
            fact_id: Some(42),
            predicate: Some("lives_in".to_string()),
            subject: Some("Alice".to_string()),
            entity: Some("Alice".to_string()),
            source: Some("chat".to_string()),
            from: Some("2020-01-01T00:00:00Z".to_string()),
            to: Some("2021-01-01T00:00:00Z".to_string()),
            all: false,
            yes: true,
            confirm_sensitive: true,
            confirmation_phrase: Some("I am sure".to_string()),
            archive: true,
        },
        sparse: ForgetRequest {
            fact_id: None,
            predicate: None,
            subject: None,
            entity: None,
            source: None,
            from: None,
            to: None,
            all: false,
            yes: false,
            confirm_sensitive: false,
            confirmation_phrase: None,
            archive: false,
        },
        sparse_skips: [
            "fact_id",
            "predicate",
            "subject",
            "entity",
            "source",
            "from",
            "to",
            "confirmation_phrase"
        ]
    );

    roundtrip_tests!(
        forget_response,
        full: ForgetResponse {
            forgotten_count: 5,
            backup_path: Some("/tmp/backup.json".to_string()),
        },
        sparse: ForgetResponse {
            forgotten_count: 0,
            backup_path: None,
        },
        sparse_skips: ["backup_path"]
    );

    roundtrip_tests!(
        restore_request,
        full: RestoreRequest {
            trash_id: Some(7),
            all: false,
        },
        sparse: RestoreRequest {
            trash_id: None,
            all: true,
        },
        sparse_skips: ["trash_id"]
    );

    #[test]
    fn restore_response_roundtrip() {
        let resp = RestoreResponse { restored_count: 3 };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        trash_row,
        full: TrashRow {
            trash_id: 1,
            subject: Some("Alice".to_string()),
            predicate: Some("lives_in".to_string()),
            object: Some("London".to_string()),
            deleted_at: "2020-01-01T00:00:00Z".to_string(),
            expires_at: "2021-01-01T00:00:00Z".to_string(),
        },
        sparse: TrashRow {
            trash_id: 1,
            subject: None,
            predicate: None,
            object: None,
            deleted_at: "2020-01-01T00:00:00Z".to_string(),
            expires_at: "2021-01-01T00:00:00Z".to_string(),
        },
        sparse_skips: ["subject", "predicate", "object"]
    );

    #[test]
    fn trash_list_response_roundtrip() {
        let resp = TrashListResponse {
            total: 1,
            offset: 0,
            limit: 10,
            items: vec![TrashRow {
                trash_id: 1,
                subject: Some("Alice".to_string()),
                predicate: None,
                object: None,
                deleted_at: "2020-01-01T00:00:00Z".to_string(),
                expires_at: "2021-01-01T00:00:00Z".to_string(),
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    roundtrip_tests!(
        pending_fact_row,
        full: PendingFactRow {
            fact_id: 1,
            subject: "Alice".to_string(),
            predicate: "ssn".to_string(),
            object: Some("123-45-6789".to_string()),
            created_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse: PendingFactRow {
            fact_id: 1,
            subject: "Alice".to_string(),
            predicate: "ssn".to_string(),
            object: None,
            created_at: "2020-01-01T00:00:00Z".to_string(),
        },
        sparse_skips: ["object"]
    );

    #[test]
    fn pending_list_response_roundtrip() {
        let resp = PendingListResponse {
            total: 1,
            facts: vec![PendingFactRow {
                fact_id: 1,
                subject: "Alice".to_string(),
                predicate: "ssn".to_string(),
                object: None,
                created_at: "2020-01-01T00:00:00Z".to_string(),
            }],
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn confirm_fact_response_roundtrip() {
        let resp = ConfirmFactResponse {
            fact: sample_fact_row(),
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn reject_fact_request_defaults_and_roundtrip() {
        let parsed: RejectFactRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.reason, None);
        let req = RejectFactRequest {
            reason: Some("entered in error".to_string()),
        };
        assert_eq!(roundtrip(&req), req);
        let sparse = serde_json::to_string(&RejectFactRequest::default()).unwrap();
        assert!(!sparse.contains("reason"));
    }
}
