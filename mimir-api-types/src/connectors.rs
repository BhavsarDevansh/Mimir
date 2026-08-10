use serde::{Deserialize, Serialize};
// ---------------------------------------------------------------------------
// Connector management (Phase 3 A1 / issue #202)
// ---------------------------------------------------------------------------

/// Request body for `POST /connectors` — register a new connector instance.
///
/// `connector_type` and `backend` select the registered factory; `slug` is the
/// immutable human label; `config_json` is the backend-specific configuration
/// object serialised as a string (mirrors the `connectors.config_json`
/// column). The daemon rejects an existing `slug` with `409 Conflict`
/// (respawn-on-reconfig is A2 / #203) and an unregistered `(type, backend)`
/// pair with `400 Bad Request`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AddConnectorRequest {
    pub connector_type: String,
    pub backend: String,
    pub slug: String,
    pub display_name: String,
    pub config_json: serde_json::Value,
}

/// Status of a connector instance, mirrored as a lowercase string from the
/// `ConnectorStatus` enum (`setup` / `active` / `paused` / `error`).
pub type ConnectorStatus = String;
/// Auth state of a connector instance, mirrored as a lowercase string from the
/// `ConnectorAuthState` enum (`unauthenticated` / `authenticated` / `expired`).
pub type ConnectorAuthState = String;

/// A single connector instance with derived status, surfaced by the
/// connector management routes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorResponse {
    pub id: i32,
    /// Connector kind (`gmail` / `calendar` / `photos` / ...).
    pub connector_type: String,
    pub slug: String,
    pub backend: String,
    pub display_name: String,
    pub status: ConnectorStatus,
    pub auth_state: ConnectorAuthState,
    pub sync_cursor: Option<String>,
    /// RFC-3339 timestamp of the last successful sync, if any.
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    /// RFC-3339 timestamp of row creation.
    pub created_at: String,
    /// RFC-3339 timestamp of the last row mutation.
    pub updated_at: String,
    /// Number of `sources` rows attributed to this instance — the derived
    /// "items ingested" metric computed from the knowledge graph on demand.
    pub item_count: i64,
}

/// `GET /connectors` response — every registered instance, oldest first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorListResponse {
    pub connectors: Vec<ConnectorResponse>,
}

// ---------------------------------------------------------------------------
// Connector action routes (Phase 3 A2 / issue #203)
// ---------------------------------------------------------------------------

/// Request body for `POST /connectors/{id}/sync` — trigger a manual sync.
///
/// `full` forces a non-incremental pass (default `false`); `since` is an
/// optional relative window in seconds (`now - since`) restricting fetched
/// items. The daemon dispatches these to
/// `mimir_connectors::ConnectorSupervisor::trigger_sync` and returns a
/// [`SyncConnectorResponse`] mirroring the cycle outcome.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SyncConnectorRequest {
    #[serde(default)]
    pub full: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
}

/// Response for `POST /connectors/{id}/sync` — mirrors the supervisor's
/// `TriggerOutcome`. A successful cycle reports the item count and updated
/// cursor; `auth_expired` means the service rejected the connector's
/// credentials (the supervisor has already paused it); `failed` carries a
/// recoverable cycle error message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SyncConnectorResponse {
    /// The cycle succeeded.
    Ok {
        /// Number of raw items fetched and staged for extraction.
        fetched: u32,
        /// Updated sync cursor the supervisor persisted, or `None` if unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_cursor: Option<String>,
    },
    /// The service reported expired auth; the connector has been paused.
    AuthExpired,
    /// The cycle failed with a recoverable error.
    Failed { message: String },
}

/// Request body for `POST /connectors/{id}/tokens` — ingest credentials.
///
/// Mirrors `mimir_connectors::SecretBundle` (the `kind`-tagged enum) without
/// pulling `mimir-connectors` (or `chrono`) into this decoupled wire crate.
/// The daemon converts this to a `SecretBundle`, stores it via the
/// `SecretStore` keyed by the connector's slug, and flips the connector's
/// `auth_state` to `authenticated` — meaning *credentials are present*, not
/// that they have been validated against the service (validation happens at
/// the next sync's auth handshake).
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IngestTokenRequest {
    /// OAuth 2.0 access token with optional refresh token and expiry.
    #[serde(rename = "oauth")]
    OAuth {
        /// Short-lived bearer token presented to the service.
        access_token: String,
        /// Refresh token, absent for grants that do not issue one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        /// RFC-3339 expiry timestamp, absent when the provider omits one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
    },
    /// Static API token (e.g. a PAT).
    #[serde(rename = "api_token")]
    ApiToken { token: String },
    /// App password (legacy IMAP / Fastmail).
    #[serde(rename = "app_password")]
    AppPassword { password: String },
}

/// Redacting `Debug`: the secret fields (`access_token`, `refresh_token`,
/// `token`, `password`) are never printed verbatim, so a stray
/// `tracing::debug!(?body, ...)` or panic message cannot leak plaintext
/// credentials into logs. Variant tags and optional-field presence stay
/// visible for diagnostics.
impl std::fmt::Debug for IngestTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OAuth {
                refresh_token,
                expires_at,
                ..
            } => f
                .debug_struct("IngestTokenRequest::OAuth")
                .field("access_token", &"<redacted>")
                .field(
                    "refresh_token",
                    &refresh_token.as_ref().map(|_| "<redacted>"),
                )
                .field("expires_at", expires_at)
                .finish(),
            Self::ApiToken { .. } => f
                .debug_struct("IngestTokenRequest::ApiToken")
                .field("token", &"<redacted>")
                .finish(),
            Self::AppPassword { .. } => f
                .debug_struct("IngestTokenRequest::AppPassword")
                .field("password", &"<redacted>")
                .finish(),
        }
    }
}

/// Request body for `POST /connectors/{id}/actions` — write-back dispatch.
///
/// `kind` and `payload` are forwarded to the connector's `act()` (e.g. the
/// Calendar connector's `create_event` / `update_event` / `delete_event`).
/// Backends that do not support the action return `400 Bad Request` (mapped
/// from `ConnectorError::UnsupportedAction`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ConnectorActionRequest {
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Response for `POST /connectors/{id}/actions` — mirrors the connector's
/// `ActionResult`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionResultResponse {
    pub success: bool,
    /// Native id of the created/modified item (e.g. the event href).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    /// Optional human-readable detail (e.g. the new `ETag`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Response for `POST /connectors/{id}/forget` — cascade-forget a connector.
///
/// Reports how many sourced facts were soft-deleted to trash. The connector
/// row and its stored secret are removed regardless of the fact count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForgetConnectorResponse {
    pub forgotten_count: u64,
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

    // -- Connector action route wire types (Phase 3 A2 / #203) --
    roundtrip_tests!(
        sync_connector_request,
        full: SyncConnectorRequest {
            full: true,
            since: Some(3600),
        },
        sparse: SyncConnectorRequest {
            full: false,
            since: None,
        },
        sparse_skips: ["since"]
    );

    #[test]
    fn sync_connector_request_defaults_to_incremental() {
        let parsed: SyncConnectorRequest = serde_json::from_str("{}").unwrap();
        assert!(!parsed.full);
        assert_eq!(parsed.since, None);
    }

    #[test]
    fn sync_connector_response_ok_roundtrip() {
        let resp = SyncConnectorResponse::Ok {
            fetched: 12,
            new_cursor: Some("v2".to_string()),
        };
        assert_eq!(roundtrip(&resp), resp);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[test]
    fn sync_connector_response_ok_omits_null_cursor() {
        let resp = SyncConnectorResponse::Ok {
            fetched: 0,
            new_cursor: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(!json.as_object().unwrap().contains_key("new_cursor"));
    }

    #[test]
    fn sync_connector_response_auth_expired_roundtrip() {
        let resp = SyncConnectorResponse::AuthExpired;
        assert_eq!(roundtrip(&resp), resp);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "auth_expired");
    }

    #[test]
    fn sync_connector_response_failed_roundtrip() {
        let resp = SyncConnectorResponse::Failed {
            message: "boom".to_string(),
        };
        assert_eq!(roundtrip(&resp), resp);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["message"], "boom");
    }

    #[test]
    fn ingest_token_oauth_roundtrip() {
        let req = IngestTokenRequest::OAuth {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: Some("2026-01-01T00:00:00Z".to_string()),
        };
        assert_eq!(roundtrip(&req), req);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["kind"], "oauth");
    }

    #[test]
    fn ingest_token_api_token_roundtrip() {
        let req = IngestTokenRequest::ApiToken {
            token: "tok".to_string(),
        };
        assert_eq!(roundtrip(&req), req);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["kind"], "api_token");
    }

    #[test]
    fn ingest_token_app_password_roundtrip() {
        let req = IngestTokenRequest::AppPassword {
            password: "hunter2".to_string(),
        };
        assert_eq!(roundtrip(&req), req);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["kind"], "app_password");
    }

    #[test]
    fn ingest_token_oauth_omits_optional_fields() {
        let req = IngestTokenRequest::OAuth {
            access_token: "at".to_string(),
            refresh_token: None,
            expires_at: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(!json.as_object().unwrap().contains_key("refresh_token"));
        assert!(!json.as_object().unwrap().contains_key("expires_at"));
    }

    /// The redacting `Debug` impl must never print a secret value verbatim,
    /// while keeping the variant tag and optional-field presence visible.
    #[test]
    fn ingest_token_debug_redacts_secrets() {
        let req = IngestTokenRequest::OAuth {
            access_token: "super-secret-at".to_string(),
            refresh_token: Some("super-secret-rt".to_string()),
            expires_at: Some("2026-01-01T00:00:00Z".to_string()),
        };
        let debug = format!("{req:?}");
        assert!(!debug.contains("super-secret-at"));
        assert!(!debug.contains("super-secret-rt"));
        assert!(debug.contains("IngestTokenRequest::OAuth"));
        assert!(debug.contains("expires_at"));

        let api = IngestTokenRequest::ApiToken {
            token: "super-secret-tok".to_string(),
        };
        let debug = format!("{api:?}");
        assert!(!debug.contains("super-secret-tok"));

        let pw = IngestTokenRequest::AppPassword {
            password: "super-secret-pw".to_string(),
        };
        let debug = format!("{pw:?}");
        assert!(!debug.contains("super-secret-pw"));
    }

    roundtrip_tests!(
        connector_action_request,
        full: ConnectorActionRequest {
            kind: "create_event".to_string(),
            payload: serde_json::json!({"summary": "Lunch"}),
        },
        sparse: ConnectorActionRequest {
            kind: "delete_event".to_string(),
            payload: serde_json::Value::Null,
        },
        sparse_skips: []
    );

    #[test]
    fn action_result_response_roundtrip() {
        let resp = ActionResultResponse {
            success: true,
            native_id: Some("/cal/abc.ics".to_string()),
            message: Some("etag-1".to_string()),
        };
        assert_eq!(roundtrip(&resp), resp);
    }

    #[test]
    fn action_result_response_omits_optional_fields() {
        let resp = ActionResultResponse {
            success: false,
            native_id: None,
            message: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(!json.as_object().unwrap().contains_key("native_id"));
        assert!(!json.as_object().unwrap().contains_key("message"));
    }

    #[test]
    fn forget_connector_response_roundtrip() {
        let resp = ForgetConnectorResponse { forgotten_count: 7 };
        assert_eq!(roundtrip(&resp), resp);
    }
}
