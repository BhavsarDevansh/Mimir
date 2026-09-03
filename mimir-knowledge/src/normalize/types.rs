//! Shared pipeline data types: batch provenance, normalized facts, and
//! outcome summaries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::KnowledgeError;
use crate::models::entity::EntityType;
use crate::models::enums::{ConnectorType, EventType, LocationType, RecurrenceType};
use crate::models::fact::Fact;
use crate::models::source::{ExtractionMethod, SourceType};
// ---------------------------------------------------------------------------
// Outcome types (shared by the extraction + connector pipelines)
// ---------------------------------------------------------------------------

/// A fact awaiting user confirmation because it was flagged as sensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingFact {
    pub fact_id: i32,
    pub subject_name: String,
    pub relationship_type: String,
    pub object_display: String,
}

/// Result of running the normalize → insert pipeline over a batch of facts.
#[derive(Debug, Default)]
pub struct ExtractionOutcome {
    pub inserted: Vec<Fact>,
    pub pending_confirmation: Vec<PendingFact>,
    pub errors: Vec<KnowledgeError>,
}

// ---------------------------------------------------------------------------
// Provenance + NormalizedFact
// ---------------------------------------------------------------------------

/// Batch-level provenance shared by every fact in one [`normalize_and_insert`](super::normalize_and_insert)
/// call.
///
/// Carries the connector identity (when the facts come from a connector sync)
/// and the extraction method that produced them. Per-fact provenance
/// (`source_type`, `raw_reference`) lives on [`NormalizedFact`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    /// Registered connector instance backing these facts, or `None` for
    /// conversational learning. When set, `connector_type` must match the
    /// instance's registered type.
    pub connector_instance_id: Option<i32>,
    /// Denormalised connector kind. Required when `connector_instance_id` is
    /// set so the confidence model can read the reliability score without a
    /// join; `None` for conversational facts.
    pub connector_type: Option<ConnectorType>,
    /// How the facts were extracted (`LlmExtraction` for chat and LLM-driven
    /// connector extraction, `StructuredParse` for structurally-parsed
    /// connector items such as calendar events or email headers).
    pub extraction_method: ExtractionMethod,
}

impl Provenance {
    /// Provenance for conversational learning (no connector instance).
    pub const fn chat(extraction_method: ExtractionMethod) -> Self {
        Self {
            connector_instance_id: None,
            connector_type: None,
            extraction_method,
        }
    }

    /// Provenance for a connector sync. The instance id and type identify the
    /// registered connector; the extraction method describes how the raw items
    /// were turned into facts.
    pub const fn connector(
        connector_instance_id: i32,
        connector_type: ConnectorType,
        extraction_method: ExtractionMethod,
    ) -> Self {
        Self {
            connector_instance_id: Some(connector_instance_id),
            connector_type: Some(connector_type),
            extraction_method,
        }
    }
}

/// Structured location overlay carried by a [`NormalizedFact`] (Phase 3 S3 /
/// #193).
///
/// When a fact describes where an entity is/was (a "lives at" / "located at"
/// assertion, or a connector-extracted address/GPS fix), the producer fills
/// this overlay with the typed geo data. [`normalize_and_insert`](super::normalize_and_insert) then derives
/// an `entity_locations` row for the resolved subject entity, geocoding the
/// missing half (address -> coords or coords -> address) via the injected
/// [`Geocoder`](mimir_core::geocoder::Geocoder) when only one side is known.
/// The temporal bounds (`valid_from` / `valid_until`) come from the fact, so a
/// move ("home 2020-2023, home 2023-present") is modelled by the fact's bounds
/// plus the upsert's supersession of the prior open-ended location.
///
/// `f64` coordinates keep this `PartialEq`-only (not `Eq`); consequently
/// [`NormalizedFact`] is `PartialEq`-only too.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NormalizedLocation {
    /// Classification of the location (Home / Work / Visited / Origin /
    /// Current). Mirrors the `location_types` lookup.
    pub location_type: LocationType,
    /// Free-text address or place name. Forward-geocoded to coords when
    /// `latitude` / `longitude` are both `None`.
    pub address: Option<String>,
    /// WGS-84 latitude in decimal degrees. Reverse-geocoded to a place name
    /// (stored as `address`) when `address` is `None`.
    pub latitude: Option<f64>,
    /// WGS-84 longitude in decimal degrees.
    pub longitude: Option<f64>,
    /// IANA timezone name (e.g. `Europe/London`), when known.
    pub timezone: Option<String>,
}

impl NormalizedLocation {
    /// `true` when at least one half of the geo data is present.
    pub fn has_geo_data(&self) -> bool {
        self.address.is_some() || (self.latitude.is_some() && self.longitude.is_some())
    }
}

/// A single fact ready for the shared insert pipeline, provenance-annotated.
///
/// Both the LLM `remember` path and connector ingestion produce this type;
/// they differ only in `source_type` and (via [`Provenance`]) `extraction_method`.
/// Entity types and temporal bounds are already typed — no string parsing
/// happens inside [`normalize_and_insert`](super::normalize_and_insert). An optional [`NormalizedLocation`]
/// overlay turns a "where" fact into an `entity_locations` row.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NormalizedFact {
    /// Origin family for this fact. Chat sets `UserEdit`/`Interaction` per
    /// fact (a batch may mix them); connectors set `Connector`.
    pub source_type: SourceType,
    pub subject: String,
    pub subject_type: EntityType,
    pub relationship_type: String,
    pub object: String,
    pub object_is_entity: bool,
    pub object_type: Option<EntityType>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    /// Producer's initial sensitivity flag; Rust narrows it via the
    /// sensitivity `AND`-gate.
    pub is_sensitive: bool,
    /// Whether this fact is a conversational correction. The chat adapter sets
    /// this from the LLM `Correction` classification; connectors always leave it
    /// `false` (corrections are conversational-only). When `true` the fact is
    /// routed through `handle_correction` even if [`correction_scope`](Self::correction_scope)
    /// is `None`, which defaults to a temporal correction at `now`.
    pub is_correction: bool,
    /// Conversational correction scope (`Some("always")` or a datetime, or
    /// `None` for a temporal correction at `now` when [`is_correction`](Self::is_correction)
    /// is set). Connectors leave this `None`.
    pub correction_scope: Option<String>,
    /// Already-parsed catalogue category IDs; validated against the DB.
    pub category_ids: Vec<i32>,
    pub recurrence: RecurrenceType,
    /// Raw `RRULE` string (interval, day/month constraints, `COUNT`/`UNTIL`)
    /// when the producer supplied one; `None` for kind-only producers.
    pub recurrence_rule: Option<String>,
    /// How often the series repeats (every N periods; 1 = every period).
    pub recurrence_interval: i32,
    /// Effective series end (from `UNTIL`, or computed from `COUNT` at
    /// extraction); `None` = unbounded.
    pub recurrence_until: Option<DateTime<Utc>>,
    pub requires_user_action: bool,
    /// Native id of the source item (e.g. an email UID, a calendar event id).
    /// Required when [`Provenance::connector_instance_id`] is set.
    pub raw_reference: Option<String>,
    /// Per-fact extraction-method override (#234).
    ///
    /// `None` (the default) means "inherit the batch [`Provenance`]'s
    /// `extraction_method`" — the behaviour every existing producer relies
    /// on. A connector whose single `extract()` batch mixes extraction
    /// methods (e.g. the Email connector, which runs deterministic iMIP /
    /// JSON-LD layers alongside the LLM layer #201) sets this per fact so
    /// `sources.extraction_method_id` records how *this* fact was produced,
    /// not the supervisor's batch-wide default. The fact value wins when set;
    /// `None` always falls back to the provenance. This keeps mixed-method
    /// batches distinguishable in the provenance chain and the confidence
    /// model without requiring one connector instance per method.
    pub extraction_method: Option<ExtractionMethod>,
    /// Per-fact confidence override (#62).
    ///
    /// `None` (the default for every existing producer) keeps the structural
    /// confidence model: the source-type initial (chat/connector). The
    /// Obsidian import sets this from a rendered `confidence: N` attribute so
    /// an export → re-import round trip preserves non-explicit scores;
    /// values are clamped to `[0.0, 1.0]` by the pipeline.
    pub confidence: Option<f32>,
    /// Optional event-type hint for the events-subsystem overlay (#74).
    ///
    /// `None` (the default for conversational facts) lets
    /// `event_from_extraction` derive the type from `requires_user_action`
    /// (`Task` vs `Reminder`). A producer that knows the kind — e.g. the
    /// Calendar connector setting [`EventType::Appointment`] — supplies it
    /// so the overlay is typed correctly without overloading
    /// `requires_user_action`. The hint only narrows the derived type when
    /// the fact also qualifies for an overlay (future-dated, recurring, or
    /// requires action); it never forces an overlay on its own.
    pub event_type: Option<EventType>,
    /// Optional structured location overlay. When present, the resolved
    /// subject entity gets an `entity_locations` row derived from this and the
    /// fact's temporal bounds (Phase 3 S3 / #193).
    pub location: Option<NormalizedLocation>,
}
