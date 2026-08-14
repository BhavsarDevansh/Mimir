//! Mock connector configuration types and serde defaults.
//!
//! All behaviour knobs of the mock connector (facts, cadence, mode, failure
//! injection) are expressed as config so the same binary exercises the
//! framework under every feature combination.

use serde::{Deserialize, Serialize};

use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{ConnectorAuthState, RecurrenceType};
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::normalize::{NormalizedFact, NormalizedLocation};

use chrono::Utc;

use crate::connector::HealthStatus;

pub(super) const DEFAULT_SLUG: &str = "mock";
pub(super) const DEFAULT_DISPLAY_NAME: &str = "Mock Connector";
pub(super) const DEFAULT_INTERVAL_MS: u64 = 60_000;
pub(super) const DEFAULT_JITTER_MS: u64 = 5_000;

pub(super) fn default_slug() -> String {
    DEFAULT_SLUG.to_string()
}
pub(super) fn default_display_name() -> String {
    DEFAULT_DISPLAY_NAME.to_string()
}
pub(super) fn default_interval_ms() -> u64 {
    DEFAULT_INTERVAL_MS
}
pub(super) fn default_jitter_ms() -> u64 {
    DEFAULT_JITTER_MS
}
pub(super) fn default_health() -> HealthStatus {
    HealthStatus::Online
}
pub(super) fn default_auth_state() -> ConnectorAuthState {
    ConnectorAuthState::Authenticated
}

// ---------------------------------------------------------------------------
// Config DTOs (serde boundary for `config_json`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MockMode {
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

pub(super) fn default_subject_type() -> EntityType {
    EntityType::Concept
}
pub(super) fn default_recurrence() -> RecurrenceType {
    RecurrenceType::None
}

impl MockFactConfig {
    /// Convert into a [`NormalizedFact`] for the connector pipeline, filling
    /// the connector-only fields. `index` is the fact's position in the canned
    /// list, used to auto-generate `raw_reference` when absent.
    pub(super) fn to_normalized(&self, slug: &str, index: usize) -> NormalizedFact {
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
            extraction_method: None,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct MockConnectorConfig {
    #[serde(default)]
    pub(super) mode: MockMode,
    #[serde(default = "default_interval_ms")]
    pub(super) interval_ms: u64,
    #[serde(default = "default_jitter_ms")]
    pub(super) jitter_ms: u64,
    #[serde(default)]
    pub(super) facts: Vec<MockFactConfig>,
    /// When set, emit at most `batch_size` facts per sync (incremental sync to
    /// completion). `None` emits the full list each sync.
    #[serde(default)]
    pub(super) batch_size: Option<u32>,
    #[serde(default = "default_health")]
    pub(super) health: HealthStatus,
    #[serde(default = "default_auth_state")]
    pub(super) auth_state: ConnectorAuthState,
    #[serde(default)]
    pub(super) fail_first: u32,
    #[serde(default)]
    pub(super) panic_first: u32,
    #[serde(default)]
    pub(super) always_fail: bool,
    /// When set, `authenticate()` fails with `NotAuthenticated` so the
    /// supervisor's runner exits at the auth handshake (used to exercise the
    /// "already-finished handle" path in `stop`).
    #[serde(default)]
    pub(super) auth_fail: bool,
    /// Static cursor returned by every successful `sync()` (`None` ⇒ unchanged).
    #[serde(default)]
    pub(super) cursor: Option<String>,
    /// Artificial delay inside a successful `sync()` (serialization tests).
    #[serde(default)]
    pub(super) sync_delay_ms: u64,
    /// Optional display name; defaults to the slug.
    #[serde(default)]
    pub(super) display_name: Option<String>,
    /// Raw references (KB `sources.raw_reference` values) to report as
    /// server-side deletions via `extract_deletions`. Staged by every `sync`;
    /// the supervisor acknowledges processed removals via
    /// `acknowledge_deletions` (PR #313 review), mirroring a server that
    /// keeps re-reporting a tombstone until the cursor advances; the KB
    /// trash path is idempotent, so re-reports are no-ops.
    #[serde(default)]
    pub(super) deletions: Vec<String>,
    /// When set, the mock accepts this `act()` kind and returns a canned
    /// `ActionResult` echoing the payload (Phase 3 A2 / #203).
    #[serde(default)]
    pub(super) act_kind: Option<String>,
}

// ---------------------------------------------------------------------------
// Sync-options recorder (test instrumentation)
// ---------------------------------------------------------------------------
