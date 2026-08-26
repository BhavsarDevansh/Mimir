//! Internal `MockConnector` behaviour (config-driven fact generation).

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use tokio::sync::Mutex;

use mimir_knowledge::models::entity::ENTITY_TYPES;
use mimir_knowledge::models::enums::{ConnectorType, RECURRENCE_TYPES};

use super::MockConnector;
use super::config::{
    DEFAULT_INTERVAL_MS, DEFAULT_JITTER_MS, default_recurrence, default_slug, default_subject_type,
};
use super::config::{MockConnectorConfig, MockMode};
use super::recorder::MockSyncRecorder;
use crate::connector::{ConnectorError, ConnectorMode};

impl MockConnector {
    /// Build a mock connector from its merged `config_json` value.
    ///
    /// `__slug` / `__ctype` / `__instance_id` (injected by the supervisor) are
    /// read directly from the value; the remaining behaviour surface is
    /// deserialised into `MockConnectorConfig`. A malformed payload returns
    /// [`ConnectorError::Config`].
    pub fn from_config(config: serde_json::Value) -> Result<Self, ConnectorError> {
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(default_slug);

        let ctype = match config.get("__ctype") {
            None => ConnectorType::Email,
            Some(value) => {
                let id = value
                    .as_i64()
                    .ok_or_else(|| ConnectorError::Config("`__ctype` must be an integer".into()))?;
                let id = i16::try_from(id)
                    .map_err(|_| ConnectorError::Config("`__ctype` is out of range".into()))?;
                ConnectorType::try_from(id)
                    .map_err(|_| ConnectorError::Config(format!("unknown `__ctype`: {id}")))?
            }
        };

        let parsed: MockConnectorConfig = serde_json::from_value(config)
            .map_err(|error| ConnectorError::Config(error.to_string()))?;
        if parsed.batch_size == Some(0) {
            return Err(ConnectorError::Config(
                "`batch_size` must be greater than zero".into(),
            ));
        }

        let mode = match parsed.mode {
            MockMode::Polling => ConnectorMode::Polling {
                interval: Duration::from_millis(parsed.interval_ms),
                jitter: Duration::from_millis(parsed.jitter_ms),
            },
            MockMode::Push => ConnectorMode::Push,
        };

        let display_name = parsed.display_name.unwrap_or_else(|| slug.clone());

        Ok(Self {
            slug,
            display_name,
            ctype,
            mode,
            mode_override: None,
            mode_resolution_override: None,
            facts: parsed.facts,
            batch_size: parsed.batch_size,
            health: parsed.health,
            auth_state: parsed.auth_state,
            fail_first: parsed.fail_first,
            panic_first: parsed.panic_first,
            always_fail: parsed.always_fail,
            auth_fail: parsed.auth_fail,
            cursor: parsed.cursor,
            sync_delay: Duration::from_millis(parsed.sync_delay_ms),
            interval: Duration::from_millis(parsed.interval_ms),
            recorder: None,
            sync_calls: AtomicU32::new(0),
            sync_successes: AtomicU32::new(0),
            buffer: Mutex::new(Vec::new()),
            deletions: parsed.deletions,
            tombstones: Mutex::new(Vec::new()),
            act_kind: parsed.act_kind,
        })
    }

    /// Attach a shared [`MockSyncRecorder`] so `sync()` records its
    /// [`SyncOptions`](crate::connector::SyncOptions) and in-flight concurrency. Consumes and returns `self`
    /// for chaining; not exposed through the factory/config path.
    pub fn with_recorder(mut self, recorder: Arc<MockSyncRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Attach a shared runtime mode override (issue #397 review): while the
    /// override is `Some`, [`Connector::mode`](crate::connector::Connector::mode)
    /// reports it instead of the configured mode. Consumes and returns `self`
    /// for chaining; not exposed through the factory/config path. The test
    /// and the supervisor's cloned instance share the same `Arc`, so flipping
    /// the value is visible to `trigger_sync` without re-instantiating.
    pub fn with_mode_override(mut self, mode: Arc<StdMutex<Option<ConnectorMode>>>) -> Self {
        self.mode_override = Some(mode);
        self
    }

    /// Attach a shared runtime mode-resolution override (issue #475): while
    /// present, [`Connector::mode_if_resolved`](crate::connector::Connector::mode_if_resolved)
    /// reports its value instead of the default `Some(self.mode())`. `None`
    /// simulates an unprobed `auto` connector whose capability probe has not
    /// run yet; `Some(mode)` pins the resolved mode. Consumes and returns
    /// `self` for chaining; not exposed through the factory/config path.
    pub fn with_mode_resolution_override(
        mut self,
        mode: Arc<StdMutex<Option<ConnectorMode>>>,
    ) -> Self {
        self.mode_resolution_override = Some(mode);
        self
    }

    /// JSON Schema describing the mock's config surface (for the future
    /// `mimir connector add` flow and discoverability).
    pub(super) fn config_schema_value() -> serde_json::Value {
        // The `subject_type`/`object_type`/`recurrence` enum lists *and* their
        // defaults are derived from the serde representation of the canonical
        // enum variant arrays (see [`ENTITY_TYPES`] / [`RECURRENCE_TYPES`]) so
        // they cannot silently desync from the enums on a future rename.
        // `object_type` additionally permits JSON `null`.
        let entity_enum = Self::entity_type_schema_enum();
        // `object_type` reuses the entity-type enum and additionally permits
        // JSON `null` (the field is `Option<EntityType>`).
        let mut object_enum = entity_enum.clone();
        object_enum.push(serde_json::Value::Null);
        let recurrence_enum = Self::recurrence_schema_enum();
        let subject_default = Self::entity_type_schema_default();
        let recurrence_default = Self::recurrence_schema_default();
        Self::config_schema_template(
            entity_enum,
            object_enum,
            recurrence_enum,
            subject_default,
            recurrence_default,
        )
    }

    /// Serialise [`ENTITY_TYPES`] to the JSON Schema `enum` values for
    /// `subject_type`. Falls back to the variant names because [`EntityType`]
    /// serialises as its variant string.
    fn entity_type_schema_enum() -> Vec<serde_json::Value> {
        ENTITY_TYPES
            .iter()
            .map(|t| serde_json::to_value(*t).expect("EntityType serialises as string"))
            .collect()
    }

    /// Serialise [`RECURRENCE_TYPES`] to the JSON Schema `enum` values for
    /// `recurrence`.
    fn recurrence_schema_enum() -> Vec<serde_json::Value> {
        RECURRENCE_TYPES
            .iter()
            .map(|r| serde_json::to_value(*r).expect("RecurrenceType serialises as string"))
            .collect()
    }

    /// Default `subject_type` schema value, serialised from
    /// [`default_subject_type`] to stay source-linked with [`EntityType`].
    fn entity_type_schema_default() -> serde_json::Value {
        serde_json::to_value(default_subject_type()).expect("EntityType default serialises")
    }

    /// Default `recurrence` schema value, serialised from
    /// [`default_recurrence`] to stay source-linked with [`RecurrenceType`].
    fn recurrence_schema_default() -> serde_json::Value {
        serde_json::to_value(default_recurrence()).expect("RecurrenceType default serialises")
    }

    /// Closed-over schema template, interpolating the serde-derived enum
    /// arrays so the schema stays source-linked to the enums.
    fn config_schema_template(
        entity_enum: Vec<serde_json::Value>,
        object_enum: Vec<serde_json::Value>,
        recurrence_enum: Vec<serde_json::Value>,
        subject_default: serde_json::Value,
        recurrence_default: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["polling", "push"],
                    "default": "polling",
                    "description": "Polling paces via the supervisor interval; push self-paces inside sync()."
                },
                "interval_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "default": DEFAULT_INTERVAL_MS,
                    "description": "Polling interval, or push internal sync cadence, in milliseconds."
                },
                "jitter_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "default": DEFAULT_JITTER_MS,
                    "description": "Polling jitter in milliseconds (ignored in push mode)."
                },
                "facts": {
                    "type": "array",
                    "description": "Canned NormalizedFacts emitted by sync().",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["subject", "relationship_type", "object"],
                        "properties": {
                            "subject": {
                                "type": "string",
                                "description": "Subject display name."
                            },
                            "subject_type": {
                                "type": "string",
                                "enum": entity_enum,
                                "default": subject_default,
                                "description": "Entity type for the subject."
                            },
                            "relationship_type": {
                                "type": "string",
                                "description": "Predicate (canonicalised later by normalize_and_insert)."
                            },
                            "object": {
                                "type": "string",
                                "description": "Object display name or literal value."
                            },
                            "object_is_entity": {
                                "type": "boolean",
                                "default": false,
                                "description": "Whether the object is an entity reference (vs a literal)."
                            },
                            "object_type": {
                                "type": ["string", "null"],
                                "enum": object_enum,
                                "default": null,
                                "description": "Entity type for the object when object_is_entity is true."
                            },
                            "valid_from": {
                                "type": ["string", "null"],
                                "format": "date-time",
                                "default": null,
                                "description": "Temporal lower bound (RFC 3339)."
                            },
                            "valid_until": {
                                "type": ["string", "null"],
                                "format": "date-time",
                                "default": null,
                                "description": "Temporal upper bound (RFC 3339)."
                            },
                            "is_sensitive": {
                                "type": "boolean",
                                "default": false,
                                "description": "Producer sensitivity flag."
                            },
                            "recurrence": {
                                "type": "string",
                                "enum": recurrence_enum,
                                "default": recurrence_default,
                                "description": "Recurrence kind."
                            },
                            "requires_user_action": {
                                "type": "boolean",
                                "default": false,
                                "description": "Whether the fact requires user action (a task)."
                            },
                            "raw_reference": {
                                "type": ["string", "null"],
                                "default": null,
                                "description": "Native id of the source item; auto-generated when absent."
                            }
                        }
                    }
                },
                "batch_size": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "description": "Emit at most N facts per sync (incremental). Omit to emit all."
                },
                "health": {
                    "oneOf": [
                        {
                            "type": "string",
                            "enum": ["online", "offline", "degraded", "not_configured"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "auth_expired": {
                                    "type": "string",
                                    "description": "Rejection message surfaced in `last_error` (issue #507)."
                                }
                            },
                            "required": ["auth_expired"],
                            "additionalProperties": false
                        }
                    ],
                    "default": "online",
                    "description": "Health probe outcome: a string (`online` / `offline` / `degraded` / `not_configured`) or an object `{ \"auth_expired\": \"<message>\" }` carrying the rejection message surfaced in `last_error` (issue #507)."
                },
                "auth_state": {
                    "type": "string",
                    "enum": ["Unauthenticated", "Authenticated", "Expired"],
                    "default": "Authenticated"
                },
                "fail_first": { "type": "integer", "minimum": 0, "default": 0 },
                "panic_first": { "type": "integer", "minimum": 0, "default": 0 },
                "always_fail": { "type": "boolean", "default": false },
                "auth_fail": {
                    "type": "boolean",
                    "default": false,
                    "description": "When set, authenticate() fails with NotAuthenticated so the runner exits at the auth handshake."
                },
                "cursor": {
                    "type": ["string", "null"],
                    "description": "Static cursor returned by every successful sync."
                },
                "sync_delay_ms": { "type": "integer", "minimum": 0, "default": 0 },
                "display_name": { "type": ["string", "null"] },
                "act_kind": {
                    "type": ["string", "null"],
                    "default": null,
                    "description": "When set, act() accepts this action kind and echoes the payload's native_id / message; any other kind yields UnsupportedAction."
                },
                "deletions": {
                    "type": "array",
                    "items": { "type": "string" },
                    "default": [],
                    "description": "Raw references to report as server-side deletions via extract_deletions(). Staged by every sync and acknowledged by the supervisor via acknowledge_deletions(); the KB trash path is idempotent, so re-reports are no-ops."
                }
            }
        })
    }
}
