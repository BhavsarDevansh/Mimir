use super::*;

use crate::email::imap;
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{ConnectorStatus, EventType};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::normalize::{Provenance, normalize_and_insert};

use super::extract_tests::{connector_with_identity, invite_email, jsonld_flight_email};

async fn init_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    (kg, dir)
}

async fn entity_named(kg: &KnowledgeGraph, name: &str) -> Option<i32> {
    kg.search_entities(name, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.entity.name == name)
        .map(|r| r.entity.id)
}

#[tokio::test]
async fn extract_funnels_into_kb_with_resolution_and_provenance() {
    let (kg, _dir) = init_kg().await;
    // Register a Gmail connector instance so connector provenance has a
    // valid `connector_instance_id` FK.
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gmail-personal".to_string(),
            backend: "imap".to_string(),
            display_name: "Gmail".to_string(),
            config_json: "{}".to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let instance_id = row.id;

    let connector = connector_with_identity(Some("Devansh"));
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 123,
        internal_date: None,
        raw: invite_email("REQUEST"),
    });
    let facts = connector.extract().await.expect("extract");
    assert_eq!(facts.len(), 4);

    let outcome = normalize_and_insert(
        &kg,
        facts,
        Provenance::connector(
            instance_id,
            ConnectorType::Gmail,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("normalize_and_insert");
    assert!(
        outcome.pending_confirmation.is_empty(),
        "no sensitive facts"
    );
    assert_eq!(outcome.inserted.len(), 4, "all four facts inserted");

    // F5: the user, the event, the place, and both attendees resolved to
    // entities (the user identity + attendees → Person; the venue → Place;
    // the SUMMARY → Event).
    let devansh = entity_named(&kg, "Devansh").await.expect("Devansh entity");
    let event = entity_named(&kg, "Dentist appointment")
        .await
        .expect("event entity");
    let place = entity_named(&kg, "123 Main St")
        .await
        .expect("place entity");
    let dr_smith = entity_named(&kg, "Dr Smith")
        .await
        .expect("Dr Smith entity");
    // The `located_in` fact resolved the venue to the Place entity.
    let mut located_in = None;
    for f in &outcome.inserted {
        if kg
            .relationship_type_name(f.relationship_type_id)
            .await
            .as_deref()
            == Some("located_in")
        {
            located_in = Some(f);
        }
    }
    let located_in = located_in.expect("located_in fact");
    assert_eq!(located_in.object_id, Some(place));

    // Locate the primary `has_event` fact and assert its temporal bounds +
    // Appointment overlay.
    let mut has_event = None;
    for f in &outcome.inserted {
        if kg
            .relationship_type_name(f.relationship_type_id)
            .await
            .as_deref()
            == Some("has_event")
        {
            has_event = Some(f);
        }
    }
    let has_event = has_event.expect("has_event fact");
    assert_eq!(has_event.subject_id, devansh);
    assert_eq!(has_event.object_id, Some(event));
    assert!(
        has_event.valid_from.is_some(),
        "DTSTART carried as valid_from"
    );
    assert!(
        has_event.valid_until.is_some(),
        "DTEND carried as valid_until"
    );
    let overlay = kg
        .get_event_by_fact(has_event.id)
        .await
        .unwrap()
        .expect("events-subsystem overlay");
    assert_eq!(overlay.event_type(), Some(EventType::Appointment));

    // Secondary facts carry no temporal bounds (no overlay) — guard against
    // the PR #248 regression where secondary facts inherited DTSTART/DTEND.
    for f in &outcome.inserted {
        let name = kg.relationship_type_name(f.relationship_type_id).await;
        if matches!(name.as_deref(), Some("located_in") | Some("attending")) {
            assert!(
                f.valid_from.is_none(),
                "secondary fact {} has valid_from",
                f.id
            );
            assert!(
                f.valid_until.is_none(),
                "secondary fact {} has valid_until",
                f.id
            );
            assert!(
                kg.get_event_by_fact(f.id).await.unwrap().is_none(),
                "secondary fact {} must not spawn an overlay",
                f.id
            );
        }
    }
    // The attendee facts resolved both the user and Dr Smith to Person
    // entities (the user is also an attendee of their own appointment).
    // The `attending` fact whose subject is Dr Smith must have resolved to
    // the Dr Smith Person entity and point at the appointment Event.
    let mut dr_smith_attending = None;
    for f in &outcome.inserted {
        if kg
            .relationship_type_name(f.relationship_type_id)
            .await
            .as_deref()
            == Some("attending")
            && f.subject_id == dr_smith
        {
            dr_smith_attending = Some(f);
            break;
        }
    }
    let dr_smith_attending = dr_smith_attending.expect("Dr Smith attending fact");
    assert_eq!(dr_smith_attending.object_id, Some(event));

    // Provenance: every fact has one Connector source tied to the instance,
    // with the VEVENT UID as `raw_reference` (the stable iMIP identity a
    // CANCEL maps onto, issue #283) and StructuredParse method.
    for f in &outcome.inserted {
        let sources = kg.get_sources_for_fact(f.id).await.unwrap();
        assert!(
            sources.iter().any(|s| {
                s.source_type_id == mimir_knowledge::models::source::SourceType::Connector as i16
                    && s.connector_instance_id == Some(instance_id)
                    && s.connector_type_id == Some(ConnectorType::Gmail as i16)
                    && s.raw_reference.as_deref() == Some("dentist-1@example.com")
                    && s.extraction_method_id == Some(ExtractionMethod::StructuredParse as i16)
            }),
            "missing connector provenance on fact {}: {:?}",
            f.id,
            sources
        );
    }
}

#[tokio::test]
async fn cancel_invite_trashes_previously_extracted_facts() {
    let (kg, _dir) = init_kg().await;
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gmail-personal".to_string(),
            backend: "imap".to_string(),
            display_name: "Gmail".to_string(),
            config_json: "{}".to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let instance_id = row.id;

    let connector = connector_with_identity(Some("Devansh"));
    // Stage the REQUEST, extract, and insert the appointment cluster.
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 42,
        uid_validity: 123,
        internal_date: None,
        raw: invite_email("REQUEST"),
    });
    let facts = connector.extract().await.expect("extract");
    assert_eq!(facts.len(), 4);
    let outcome = normalize_and_insert(
        &kg,
        facts,
        Provenance::connector(
            instance_id,
            ConnectorType::Gmail,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("normalize_and_insert");
    assert_eq!(outcome.inserted.len(), 4);
    let mut has_event = None;
    for f in &outcome.inserted {
        if kg
            .relationship_type_name(f.relationship_type_id)
            .await
            .as_deref()
            == Some("has_event")
        {
            has_event = Some(f);
            break;
        }
    }
    let has_event = has_event.expect("has_event fact");
    let has_event_id = has_event.id;
    assert!(
        kg.get_event_by_fact(has_event_id).await.unwrap().is_some(),
        "the appointment has an events-subsystem overlay"
    );

    // Stage the CANCEL (a different email, same VEVENT UID), extract (no
    // facts), and report the buffered tombstone.
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 43,
        uid_validity: 123,
        internal_date: None,
        raw: invite_email("CANCEL"),
    });
    let facts = connector.extract().await.expect("extract");
    assert!(facts.is_empty(), "CANCEL emits no facts");
    let deletions = connector.extract_deletions().await.expect("deletions");
    assert_eq!(deletions, vec!["dentist-1@example.com".to_string()]);

    // The supervisor's trash path (issue #247 machinery) removes exactly the
    // facts this instance authored for the cancelled event.
    let result = kg
        .forget_connector_facts_by_raw_reference(instance_id, &deletions, ChangedBy::System)
        .await
        .expect("trash");
    assert_eq!(result.forgotten_count, 4, "all four cluster facts trashed");
    assert!(
        kg.get_fact(has_event_id).await.unwrap().is_none(),
        "the cancelled event's fact is trashed"
    );
    assert!(
        kg.get_event_by_fact(has_event_id).await.unwrap().is_none(),
        "the events-subsystem overlay is cascade-deleted with the fact"
    );

    // Acknowledge: the processed removal is dropped from the buffer, and a
    // re-report (e.g. a duplicate CANCEL) is an idempotent no-op.
    connector
        .acknowledge_deletions(&deletions)
        .await
        .expect("acknowledge");
    assert!(
        connector
            .extract_deletions()
            .await
            .expect("deletions")
            .is_empty(),
        "acknowledged tombstones are dropped"
    );
    let again = kg
        .forget_connector_facts_by_raw_reference(instance_id, &deletions, ChangedBy::System)
        .await
        .expect("idempotent trash");
    assert_eq!(
        again.forgotten_count, 0,
        "a CANCEL with no prior facts is a no-op"
    );
}

#[tokio::test]
async fn extract_jsonld_funnels_into_kb_with_provenance() {
    let (kg, _dir) = init_kg().await;
    let row = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Gmail,
            slug: "gmail-personal".to_string(),
            backend: "imap".to_string(),
            display_name: "Gmail".to_string(),
            config_json: "{}".to_string(),
            status: Some(ConnectorStatus::Active),
            auth_state: Some(ConnectorAuthState::Authenticated),
        })
        .await
        .unwrap();
    let instance_id = row.id;

    let connector = connector_with_identity(Some("Devansh"));
    connector.buffer.lock().await.push(imap::RawEmail {
        uid: 99,
        uid_validity: 17,
        internal_date: None,
        raw: jsonld_flight_email(),
    });
    let facts = connector.extract().await.expect("extract");
    assert_eq!(facts.len(), 4);

    let outcome = normalize_and_insert(
        &kg,
        facts,
        Provenance::connector(
            instance_id,
            ConnectorType::Gmail,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("normalize_and_insert");
    assert!(outcome.pending_confirmation.is_empty());
    assert_eq!(outcome.inserted.len(), 4);

    // The user and the flight resolve to entities.
    let devansh = entity_named(&kg, "Devansh").await.expect("Devansh entity");
    let flight = entity_named(&kg, "British Airways 123")
        .await
        .expect("flight entity");

    // Primary has_flight fact links user → flight event.
    let mut has_flight = None;
    for f in &outcome.inserted {
        if kg
            .relationship_type_name(f.relationship_type_id)
            .await
            .as_deref()
            == Some("has_flight")
        {
            has_flight = Some(f);
            break;
        }
    }
    let has_flight = has_flight.expect("has_flight fact");
    assert_eq!(has_flight.subject_id, devansh);
    assert_eq!(has_flight.object_id, Some(flight));
    assert!(has_flight.valid_from.is_some());
    assert!(has_flight.valid_until.is_some());

    // Events-subsystem overlay typed as Appointment (a flight is a
    // time-bound event the user attends).
    let overlay = kg
        .get_event_by_fact(has_flight.id)
        .await
        .unwrap()
        .expect("events-subsystem overlay for flight");
    assert_eq!(overlay.event_type(), Some(EventType::Appointment));

    // Provenance: every fact has a Connector source with the
    // UIDVALIDITY-qualified UID and StructuredParse method.
    for f in &outcome.inserted {
        let sources = kg.get_sources_for_fact(f.id).await.unwrap();
        assert!(
            sources.iter().any(|s| {
                s.source_type_id == mimir_knowledge::models::source::SourceType::Connector as i16
                    && s.connector_instance_id == Some(instance_id)
                    && s.connector_type_id == Some(ConnectorType::Gmail as i16)
                    && s.raw_reference.as_deref() == Some("17:99")
                    && s.extraction_method_id == Some(ExtractionMethod::StructuredParse as i16)
            }),
            "missing connector provenance on JSON-LD fact {}: {:?}",
            f.id,
            sources
        );
    }
}
