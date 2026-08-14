//! Shared connector [`NormalizedFact`] construction (issue #255).
//!
//! Every connector backend builds facts with the same fixed defaults
//! (`SourceType::Connector`, non-sensitive, non-correction, no category ids,
//! no user action). [`connector_fact`] owns that boilerplate once so a new
//! connector cannot silently drift on a default (e.g. forgetting
//! `is_correction: false`), and the per-shape fields are the arguments.
//! Always compiled — any backend (Photos, Calendar, Email) may use it
//! regardless of feature flags.

use chrono::{DateTime, Utc};
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::{NormalizedFact, NormalizedLocation};

/// Build a connector [`NormalizedFact`] with the shared connector defaults
/// filled in: connector source type, non-sensitive, non-correction, no
/// category ids, no user action required.
///
/// All connector fact shapes (Calendar `has_event` / `located_in` /
/// `attending`, Email JSON-LD clusters, Photos `took_photo_at` / `visited` /
/// `took_photo`) share these defaults; the per-shape fields (subject,
/// relationship, object, temporal bounds, recurrence, raw reference,
/// extraction method, event-type hint, location overlay) are the arguments.
///
/// `extraction_method: None` means "inherit the batch `Provenance`'s
/// extraction method" — the default every connector relies on; the Email
/// connector's mixed-method batch passes an explicit `Some(...)` so each fact
/// records how it was produced (#234).
#[allow(clippy::too_many_arguments)] // constructor helper: every arg maps to a `NormalizedFact` field
pub(crate) fn connector_fact(
    subject: String,
    subject_type: EntityType,
    relationship_type: &str,
    object: String,
    object_is_entity: bool,
    object_type: Option<EntityType>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
    recurrence: RecurrenceType,
    raw_ref: &str,
    extraction_method: Option<ExtractionMethod>,
    event_type: Option<EventType>,
    location: Option<NormalizedLocation>,
) -> NormalizedFact {
    NormalizedFact {
        source_type: SourceType::Connector,
        subject,
        subject_type,
        relationship_type: relationship_type.to_string(),
        object,
        object_is_entity,
        object_type,
        valid_from,
        valid_until,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence,
        requires_user_action: false,
        raw_reference: Some(raw_ref.to_string()),
        extraction_method,
        event_type,
        location,
    }
}
