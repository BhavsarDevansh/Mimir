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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use mimir_knowledge::models::enums::LocationType;

    #[test]
    fn connector_fact_fills_shared_defaults_and_carries_override_params() {
        // The shared connector-fact helper (issue #255) must fill every fixed
        // connector default so per-connector call sites (Calendar VEVENTs, Email
        // JSON-LD, Photos) never repeat the struct literal with subtly wrong
        // defaults (e.g. forgetting `is_correction: false`).
        let fact = connector_fact(
            "Alice".to_string(),
            EntityType::Person,
            "located_in",
            "Rome".to_string(),
            true,
            Some(EntityType::Place),
            None,
            None,
            RecurrenceType::None,
            "raw-1",
            Some(ExtractionMethod::StructuredParse),
            Some(EventType::Appointment),
            None,
        );
        // Fixed connector defaults (the DRY contract).
        assert_eq!(fact.source_type, SourceType::Connector);
        assert!(!fact.is_sensitive);
        assert!(!fact.is_correction);
        assert_eq!(fact.correction_scope, None);
        assert!(fact.category_ids.is_empty());
        assert!(!fact.requires_user_action);
        assert_eq!(fact.raw_reference.as_deref(), Some("raw-1"));
        // Per-fact override params.
        assert_eq!(fact.subject, "Alice");
        assert_eq!(fact.subject_type, EntityType::Person);
        assert_eq!(fact.relationship_type, "located_in");
        assert_eq!(fact.object, "Rome");
        assert!(fact.object_is_entity);
        assert_eq!(fact.object_type, Some(EntityType::Place));
        assert_eq!(fact.valid_from, None);
        assert_eq!(fact.valid_until, None);
        assert_eq!(fact.recurrence, RecurrenceType::None);
        assert_eq!(
            fact.extraction_method,
            Some(ExtractionMethod::StructuredParse)
        );
        assert_eq!(fact.event_type, Some(EventType::Appointment));
        assert_eq!(fact.location, None);

        // Photos-style literal-object variant: no per-fact extraction override
        // (inherits the batch provenance) and a location overlay.
        let taken_at = Utc.with_ymd_and_hms(2025, 5, 3, 9, 0, 0).unwrap();
        let visited = connector_fact(
            "Devansh".to_string(),
            EntityType::Person,
            "visited",
            "46.500, 7.500".to_string(),
            false,
            None,
            Some(taken_at),
            None,
            RecurrenceType::None,
            "photos/a.jpg",
            None,
            None,
            Some(NormalizedLocation {
                location_type: LocationType::Visited,
                address: None,
                latitude: Some(46.5),
                longitude: Some(7.5),
                timezone: None,
            }),
        );
        assert!(!visited.object_is_entity);
        assert_eq!(visited.object_type, None);
        assert_eq!(visited.extraction_method, None);
        assert_eq!(visited.valid_from, Some(taken_at));
        assert!(visited.location.is_some());
    }
}
