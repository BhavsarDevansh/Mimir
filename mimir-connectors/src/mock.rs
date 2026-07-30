//! Always-compiled, configurable mock connector — the framework's test harness
//! (Phase 3 F13 / issue #190).
//!
//! `MockConnector` is an in-memory connector whose behaviour is driven
//! entirely by its `config_json`: it emits canned [`NormalizedFact`]s on a
//! configurable cadence, in either [`ConnectorMode::Polling`] or
//! [`ConnectorMode::Push`], and can inject failures, panics, and health/auth
//! states to exercise the [`ConnectorSupervisor`](crate::supervisor). It is
//! always compiled (no feature flag) so the framework and registry stay
//! exercisable under every feature combination, including
//! `--no-default-features`, and it is the vehicle for the T1
//! sync→extract→insert→query end-to-end test without real services.
//!
//! # Two-step ingestion model
//!
//! [`Connector::sync`] stages the configured facts into an internal buffer and
//! returns a [`SyncOutcome`] (item count + cursor); [`Connector::extract`]
//! drains that buffer into `Vec<NormalizedFact>`. The supervisor then inserts
//! them through [`mimir_knowledge::normalize::normalize_and_insert`]. The mock
//! never touches the database.
//!
//! # Instance identity
//!
//! The supervisor injects `__slug`, `__ctype`, and `__instance_id` into a
//! connector's `config_json` before handing it to the factory (see
//! [`crate::supervisor`]). [`MockConnector::from_config`] reads these to
//! recover its identity. When they are absent (e.g. a bare
//! [`MockConnector::default`]) it falls back to the legacy no-op identity
//! (`id "mock"`, type `Gmail`).
//!
//! # Push mode
//!
//! Push connectors block inside [`Connector::sync`] waiting for service events.
//! The mock simulates this by sleeping the configured `interval_ms` at the
//! start of every `sync()` (the "schedule"), then staging the canned facts.
//! The supervisor aborts the runner task on shutdown, cancelling the in-flight
//! sleep; F9 manual triggers are rejected for push connectors, so no trigger
//! path is needed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::connector::{
    Connector, ConnectorError, ConnectorFactory, ConnectorMode, HealthStatus, SyncOptions,
    SyncOutcome,
};
use mimir_knowledge::models::entity::{ENTITY_TYPES, EntityType};
use mimir_knowledge::models::enums::{
    ConnectorAuthState, ConnectorType, RECURRENCE_TYPES, RecurrenceType,
};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::normalize::{NormalizedFact, NormalizedLocation};

// ---------------------------------------------------------------------------
// Default constants (preserve the legacy no-op identity)
// ---------------------------------------------------------------------------

const DEFAULT_SLUG: &str = "mock";
const DEFAULT_DISPLAY_NAME: &str = "Mock Connector";
const DEFAULT_INTERVAL_MS: u64 = 60_000;
const DEFAULT_JITTER_MS: u64 = 5_000;

fn default_slug() -> String {
    DEFAULT_SLUG.to_string()
}
fn default_display_name() -> String {
    DEFAULT_DISPLAY_NAME.to_string()
}
fn default_interval_ms() -> u64 {
    DEFAULT_INTERVAL_MS
}
fn default_jitter_ms() -> u64 {
    DEFAULT_JITTER_MS
}
fn default_health() -> HealthStatus {
    HealthStatus::Online
}
fn default_auth_state() -> ConnectorAuthState {
    ConnectorAuthState::Authenticated
}

// ---------------------------------------------------------------------------
// Config DTOs (serde boundary for `config_json`)
// ---------------------------------------------------------------------------

/// Connector run mode for the mock, serialised as `"polling"` / `"push"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MockMode {
    #[default]
    Polling,
    Push,
}

/// A single canned fact declared in a mock connector's `config_json`.
///
/// Mirrors the insertable subset of [`NormalizedFact`]. Entity types and
/// temporal bounds are already typed; the mock fills `source_type` and
/// `is_correction`/`correction_scope`/`category_ids` at conversion time. If
/// `raw_reference` is `None` it is auto-generated from the connector slug and
/// the fact's position in the list, so connector provenance (which requires
/// it) is always satisfied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MockFactConfig {
    /// Subject display name.
    pub subject: String,
    /// Entity type for the subject. Defaults to [`EntityType::Concept`].
    #[serde(default = "default_subject_type")]
    pub subject_type: EntityType,
    /// Predicate (canonicalised later by `normalize_and_insert`).
    pub relationship_type: String,
    /// Object display name or literal value.
    pub object: String,
    /// Whether the object is an entity reference (vs a literal).
    #[serde(default)]
    pub object_is_entity: bool,
    /// Entity type for the object when `object_is_entity` is true.
    #[serde(default)]
    pub object_type: Option<EntityType>,
    /// Temporal lower bound (RFC 3339).
    #[serde(default)]
    pub valid_from: Option<chrono::DateTime<Utc>>,
    /// Temporal upper bound (RFC 3339).
    #[serde(default)]
    pub valid_until: Option<chrono::DateTime<Utc>>,
    /// Producer sensitivity flag; the pipeline narrows it via the AND-gate.
    #[serde(default)]
    pub is_sensitive: bool,
    /// Recurrence kind. Defaults to [`RecurrenceType::None`].
    #[serde(default = "default_recurrence")]
    pub recurrence: RecurrenceType,
    /// Whether the fact requires user action (a task).
    #[serde(default)]
    pub requires_user_action: bool,
    /// Native id of the source item. Required for connector provenance; if
    /// `None` the mock generates one.
    #[serde(default)]
    pub raw_reference: Option<String>,
    /// Optional structured location overlay (Phase 3 S3 / #193). When set,
    /// the fact carries a [`NormalizedLocation`] that the pipeline turns into
    /// an `entity_locations` row for the subject entity.
    #[serde(default)]
    pub location: Option<NormalizedLocation>,
}

fn default_subject_type() -> EntityType {
    EntityType::Concept
}
fn default_recurrence() -> RecurrenceType {
    RecurrenceType::None
}

impl MockFactConfig {
    /// Convert into a [`NormalizedFact`] for the connector pipeline, filling
    /// the connector-only fields. `index` is the fact's position in the canned
    /// list, used to auto-generate `raw_reference` when absent.
    fn to_normalized(&self, slug: &str, index: usize) -> NormalizedFact {
        let raw_reference = self
            .raw_reference
            .clone()
            .unwrap_or_else(|| format!("mock-{slug}-{index}"));
        NormalizedFact {
            source_type: SourceType::Connector,
            subject: self.subject.clone(),
            subject_type: self.subject_type,
            relationship_type: self.relationship_type.clone(),
            object: self.object.clone(),
            object_is_entity: self.object_is_entity,
            object_type: self.object_type,
            valid_from: self.valid_from,
            valid_until: self.valid_until,
            is_sensitive: self.is_sensitive,
            is_correction: false,
            correction_scope: None,
            category_ids: Vec::new(),
            recurrence: self.recurrence,
            requires_user_action: self.requires_user_action,
            raw_reference: Some(raw_reference),
            event_type: None,
            location: self.location.clone(),
        }
    }
}

/// Deserialisable configuration for [`MockConnector`].
///
/// Stored as the `config_json` of a `connectors` row (with `__slug` /
/// `__ctype` / `__instance_id` injected by the supervisor). Unknown fields
/// (including the injected identity keys) are ignored by serde, so this DTO
/// only declares the behaviour surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MockConnectorConfig {
    #[serde(default)]
    mode: MockMode,
    #[serde(default = "default_interval_ms")]
    interval_ms: u64,
    #[serde(default = "default_jitter_ms")]
    jitter_ms: u64,
    #[serde(default)]
    facts: Vec<MockFactConfig>,
    /// When set, emit at most `batch_size` facts per sync (incremental sync to
    /// completion). `None` emits the full list each sync.
    #[serde(default)]
    batch_size: Option<u32>,
    #[serde(default = "default_health")]
    health: HealthStatus,
    #[serde(default = "default_auth_state")]
    auth_state: ConnectorAuthState,
    #[serde(default)]
    fail_first: u32,
    #[serde(default)]
    panic_first: u32,
    #[serde(default)]
    always_fail: bool,
    /// Static cursor returned by every successful `sync()` (`None` ⇒ unchanged).
    #[serde(default)]
    cursor: Option<String>,
    /// Artificial delay inside a successful `sync()` (serialization tests).
    #[serde(default)]
    sync_delay_ms: u64,
    /// Optional display name; defaults to the slug.
    #[serde(default)]
    display_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Sync-options recorder (test instrumentation)
// ---------------------------------------------------------------------------

/// Shared observer recording the [`SyncOptions`] each `sync()` receives and
/// the peak number of in-flight `sync()` calls.
///
/// Used by F9-style concurrency tests to assert that the supervisor serialises
/// triggers. Held behind `Arc` so a factory closure can inject one into every
/// constructed [`MockConnector`]; it is *not* part of the config schema or the
/// factory path — it is attached with [`MockConnector::with_recorder`].
#[derive(Debug, Default)]
pub struct MockSyncRecorder {
    recorded: std::sync::Mutex<Vec<SyncOptions>>,
    in_flight: AtomicU32,
    max_concurrent: AtomicU32,
}

impl MockSyncRecorder {
    /// Number of `sync()` calls recorded.
    pub fn len(&self) -> usize {
        self.recorded.lock().expect("recorder lock poisoned").len()
    }

    /// Whether no `sync()` calls have been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The [`SyncOptions`] received by the most recent `sync()`, if any.
    pub fn last(&self) -> Option<SyncOptions> {
        self.recorded
            .lock()
            .expect("recorder lock poisoned")
            .last()
            .copied()
    }

    /// Peak number of concurrently in-flight `sync()` calls observed.
    pub fn max_concurrent(&self) -> u32 {
        self.max_concurrent.load(Ordering::SeqCst)
    }

    /// Enter a `sync()` call, returning an RAII guard that records the
    /// [`SyncOptions`] and decrements the in-flight counter on [`Drop`].
    ///
    /// The guard is cancellation-, panic-, and failure-safe: it must be
    /// created *before* the first `.await` of `sync()` and is dropped when
    /// the call ends — whether by returning, unwinding on a panic, or having
    /// its task aborted by the supervisor. This guarantees `in_flight` is
    /// always balanced and that *every* call (including injected failures and
    /// panics) is recorded, rather than only successful post-delay calls.
    fn enter(&self, options: SyncOptions) -> MockSyncGuard<'_> {
        let prev = self.in_flight.fetch_add(1, Ordering::SeqCst);
        self.max_concurrent.fetch_max(prev + 1, Ordering::SeqCst);
        MockSyncGuard {
            recorder: self,
            options,
        }
    }
}

/// RAII guard returned by [`MockSyncRecorder::enter`].
///
/// [`Drop`] records the captured [`SyncOptions`] and decrements the
/// recorder's in-flight counter, so `sync()` tracking stays balanced across
/// normal returns, panics, and task cancellation.
pub struct MockSyncGuard<'a> {
    recorder: &'a MockSyncRecorder,
    options: SyncOptions,
}

impl Drop for MockSyncGuard<'_> {
    fn drop(&mut self) {
        self.recorder.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.recorder
            .recorded
            .lock()
            .expect("recorder lock poisoned")
            .push(self.options);
    }
}

// ---------------------------------------------------------------------------
// MockConnector
// ---------------------------------------------------------------------------

/// Configurable in-memory connector for testing (Phase 3 F13 / #190).
///
/// Always compiled. Behaviour is driven entirely by its `config_json` (see
/// [`MockConnectorConfig`]/[`MockFactConfig`]); [`MockConnector::default`]
/// yields the legacy no-op identity so existing trait tests keep passing.
pub struct MockConnector {
    slug: String,
    display_name: String,
    ctype: ConnectorType,
    mode: ConnectorMode,
    facts: Vec<MockFactConfig>,
    batch_size: Option<u32>,
    health: HealthStatus,
    auth_state: ConnectorAuthState,
    fail_first: u32,
    panic_first: u32,
    always_fail: bool,
    cursor: Option<String>,
    sync_delay: Duration,
    interval: Duration,
    recorder: Option<Arc<MockSyncRecorder>>,
    sync_calls: AtomicU32,
    /// Counts only *successful* syncs; drives the `batch_size` slice so
    /// failed/panicked cycles do not consume a batch window.
    sync_successes: AtomicU32,
    buffer: Mutex<Vec<NormalizedFact>>,
}

impl Default for MockConnector {
    fn default() -> Self {
        Self {
            slug: default_slug(),
            display_name: default_display_name(),
            ctype: ConnectorType::Gmail,
            mode: ConnectorMode::Polling {
                interval: Duration::from_millis(DEFAULT_INTERVAL_MS),
                jitter: Duration::from_millis(DEFAULT_JITTER_MS),
            },
            facts: Vec::new(),
            batch_size: None,
            health: HealthStatus::Online,
            auth_state: ConnectorAuthState::Authenticated,
            fail_first: 0,
            panic_first: 0,
            always_fail: false,
            cursor: None,
            sync_delay: Duration::ZERO,
            interval: Duration::from_millis(DEFAULT_INTERVAL_MS),
            recorder: None,
            sync_calls: AtomicU32::new(0),
            sync_successes: AtomicU32::new(0),
            buffer: Mutex::new(Vec::new()),
        }
    }
}

impl std::fmt::Debug for MockConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockConnector")
            .field("slug", &self.slug)
            .field("ctype", &self.ctype)
            .field("mode", &self.mode)
            .field("facts", &self.facts.len())
            .field("batch_size", &self.batch_size)
            .field("health", &self.health)
            .field("fail_first", &self.fail_first)
            .field("panic_first", &self.panic_first)
            .field("always_fail", &self.always_fail)
            .field("cursor", &self.cursor)
            .finish()
    }
}

impl MockConnector {
    /// Build a mock connector from its merged `config_json` value.
    ///
    /// `__slug` / `__ctype` / `__instance_id` (injected by the supervisor) are
    /// read directly from the value; the remaining behaviour surface is
    /// deserialised into [`MockConnectorConfig`]. A malformed payload returns
    /// [`ConnectorError::Config`].
    pub fn from_config(config: serde_json::Value) -> Result<Self, ConnectorError> {
        let slug = config
            .get("__slug")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(default_slug);

        let ctype = match config.get("__ctype") {
            None => ConnectorType::Gmail,
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
            facts: parsed.facts,
            batch_size: parsed.batch_size,
            health: parsed.health,
            auth_state: parsed.auth_state,
            fail_first: parsed.fail_first,
            panic_first: parsed.panic_first,
            always_fail: parsed.always_fail,
            cursor: parsed.cursor,
            sync_delay: Duration::from_millis(parsed.sync_delay_ms),
            interval: Duration::from_millis(parsed.interval_ms),
            recorder: None,
            sync_calls: AtomicU32::new(0),
            sync_successes: AtomicU32::new(0),
            buffer: Mutex::new(Vec::new()),
        })
    }

    /// Attach a shared [`MockSyncRecorder`] so `sync()` records its
    /// [`SyncOptions`] and in-flight concurrency. Consumes and returns `self`
    /// for chaining; not exposed through the factory/config path.
    pub fn with_recorder(mut self, recorder: Arc<MockSyncRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// JSON Schema describing the mock's config surface (for the future
    /// `mimir connector add` flow and discoverability).
    fn config_schema_value() -> serde_json::Value {
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
                    "type": "string",
                    "enum": ["online", "offline", "degraded", "auth_expired", "not_configured"],
                    "default": "online"
                },
                "auth_state": {
                    "type": "string",
                    "enum": ["Unauthenticated", "Authenticated", "Expired"],
                    "default": "Authenticated"
                },
                "fail_first": { "type": "integer", "minimum": 0, "default": 0 },
                "panic_first": { "type": "integer", "minimum": 0, "default": 0 },
                "always_fail": { "type": "boolean", "default": false },
                "cursor": {
                    "type": ["string", "null"],
                    "description": "Static cursor returned by every successful sync."
                },
                "sync_delay_ms": { "type": "integer", "minimum": 0, "default": 0 },
                "display_name": { "type": ["string", "null"] }
            }
        })
    }
}

#[async_trait::async_trait]
impl Connector for MockConnector {
    fn id(&self) -> &str {
        &self.slug
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn connector_type(&self) -> ConnectorType {
        self.ctype
    }

    fn mode(&self) -> ConnectorMode {
        self.mode
    }

    fn config_schema(&self) -> serde_json::Value {
        Self::config_schema_value()
    }

    async fn authenticate(&self) -> Result<ConnectorAuthState, ConnectorError> {
        Ok(self.auth_state)
    }

    async fn health(&self) -> Result<HealthStatus, ConnectorError> {
        Ok(self.health)
    }

    async fn sync(&self, options: SyncOptions) -> Result<SyncOutcome, ConnectorError> {
        let n = self.sync_calls.fetch_add(1, Ordering::SeqCst);

        // Track the complete sync() call. The guard is created before the
        // first await and dropped on return, panic unwind, or task
        // cancellation, so `in_flight` is always balanced and every call —
        // including injected failures and panics — is recorded.
        let _guard = self
            .recorder
            .as_ref()
            .map(|recorder| recorder.enter(options));

        // Push connectors block inside sync waiting for events; the mock
        // simulates this by sleeping the configured cadence. The supervisor
        // aborts the runner task on shutdown, cancelling the sleep.
        if matches!(self.mode, ConnectorMode::Push) {
            tokio::time::sleep(self.interval).await;
        }

        // Panic injection (counted as a failure by the supervisor).
        if n < self.panic_first {
            panic!("mock connector panic #{n}");
        }

        // Failure injection.
        if self.always_fail || n < self.fail_first {
            return Err(ConnectorError::Network(format!(
                "simulated mock failure #{n}"
            )));
        }

        // Optional artificial delay (serialization/concurrency tests). The
        // recorder guard above already brackets this, so overlapping triggers
        // are observable even if the delay is cancelled.
        if !self.sync_delay.is_zero() {
            tokio::time::sleep(self.sync_delay).await;
        }

        // Stage the canned facts for this cycle. With `batch_size`, slice
        // incrementally to completion; otherwise emit the full list. The batch
        // window is keyed on the *successful*-sync counter (`sync_successes`),
        // not the raw call counter (`n`), so failed/panicked cycles do not
        // consume a window and silently drop facts.
        let success_index = self.sync_successes.fetch_add(1, Ordering::SeqCst);
        let staged: Vec<NormalizedFact> = match self.batch_size {
            None => self
                .facts
                .iter()
                .enumerate()
                .map(|(i, f)| f.to_normalized(&self.slug, i))
                .collect(),
            Some(size) => {
                let size = size as usize;
                let start = (success_index as usize)
                    .saturating_mul(size)
                    .min(self.facts.len());
                let end = (start + size).min(self.facts.len());
                self.facts[start..end]
                    .iter()
                    .enumerate()
                    .map(|(offset, f)| f.to_normalized(&self.slug, start + offset))
                    .collect()
            }
        };

        let fetched = u32::try_from(staged.len()).unwrap_or(u32::MAX);
        self.buffer.lock().await.extend(staged);

        Ok(SyncOutcome {
            fetched,
            new_cursor: self.cursor.clone(),
            fetched_at: Utc::now(),
        })
    }

    async fn extract(&self) -> Result<Vec<NormalizedFact>, ConnectorError> {
        let mut buffer = self.buffer.lock().await;
        let drained = std::mem::take(&mut *buffer);
        Ok(drained)
    }

    async fn forget(&self) -> Result<(), ConnectorError> {
        // The mock holds no credentials or persisted local data; forget is a
        // no-op. The supervisor cascades KB facts via the trash machinery.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// [`ConnectorFactory`] that builds a configured [`MockConnector`] from its
/// `config_json`. Always compiled so the registry is exercisable under every
/// feature combination (including `--no-default-features`).
#[derive(Debug, Default)]
pub struct MockConnectorFactory;

impl ConnectorFactory for MockConnectorFactory {
    fn create(
        &self,
        config: serde_json::Value,
        _ctx: &crate::connector::ConnectorContext,
    ) -> Result<std::sync::Arc<dyn Connector>, ConnectorError> {
        let connector = MockConnector::from_config(config)?;
        Ok(std::sync::Arc::new(connector) as std::sync::Arc<dyn Connector>)
    }
}
