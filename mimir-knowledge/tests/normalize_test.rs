//! Integration tests for the shared `normalize_and_insert` DRY boundary
//! (Phase 3 F4 / issue #181).
//!
//! Both conversational `remember` extraction and connector ingestion funnel
//! through this single deterministic Rust pipeline. These tests exercise the
//! boundary directly with connector provenance — the new code path — and the
//! cross-connector corroboration acceptance criterion.

use chrono::{DateTime, Utc};

use mimir_knowledge::confidence;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{ConnectorType, LocationType, RecurrenceType};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::{
    NormalizedFact, NormalizedLocation, Provenance, normalize_and_insert,
};
use mimir_knowledge::{KnowledgeError, KnowledgeGraph};

/// Fresh KnowledgeGraph in a temp dir.
async fn fresh_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("normalize_test.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone::<Utc>(&Utc)
}

fn rome_event(raw_ref: &str, valid_until: Option<DateTime<Utc>>) -> NormalizedFact {
    NormalizedFact {
        source_type: SourceType::Connector,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "has_event".to_string(),
        object: "Trip to Rome".to_string(),
        object_is_entity: true,
        object_type: Some(EntityType::Event),
        valid_from: Some(parse_dt("2026-05-03T00:00:00Z")),
        valid_until,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: Some(raw_ref.to_string()),
        location: None,
    }
}

async fn upsert(kg: &KnowledgeGraph, ct: ConnectorType, slug: &str) -> i32 {
    kg.upsert_connector(UpsertConnectorInput {
        connector_type: ct,
        slug: slug.to_string(),
        backend: if ct == ConnectorType::Gmail {
            "imap".to_string()
        } else {
            "caldav".to_string()
        },
        display_name: slug.to_string(),
        config_json: "{}".to_string(),
        status: None,
        auth_state: None,
    })
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn connector_normalized_fact_inserts_with_connector_provenance() {
    let (kg, _dir) = fresh_kg().await;
    let calendar_instance = upsert(&kg, ConnectorType::Calendar, "calendar-1").await;

    let outcome = normalize_and_insert(
        &kg,
        vec![rome_event(
            "cal-evt-123",
            Some(parse_dt("2026-05-07T00:00:00Z")),
        )],
        Provenance::connector(
            calendar_instance,
            ConnectorType::Calendar,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("connector insert should succeed");
    assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
    assert_eq!(outcome.inserted.len(), 1);
    assert!(outcome.pending_confirmation.is_empty());

    let inserted = &outcome.inserted[0];
    // Connector reliability score for Calendar, no extraction-method discount.
    assert!(
        (inserted.confidence
            - confidence::initial(SourceType::Connector, Some(ConnectorType::Calendar)))
        .abs()
            < 1e-5,
        "confidence {} should equal Calendar reliability 0.90",
        inserted.confidence
    );

    let sources = kg.get_sources_for_fact(inserted.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_type_id, SourceType::Connector as i16);
    assert_eq!(sources[0].connector_instance_id, Some(calendar_instance));
    assert_eq!(sources[0].raw_reference.as_deref(), Some("cal-evt-123"));
    assert_eq!(
        sources[0].extraction_method_id,
        Some(ExtractionMethod::StructuredParse as i16)
    );
}

#[tokio::test]
async fn cross_connector_corroboration_adds_source_and_boosts_confidence() {
    let (kg, _dir) = fresh_kg().await;
    let calendar_instance = upsert(&kg, ConnectorType::Calendar, "calendar-1").await;
    let gmail_instance = upsert(&kg, ConnectorType::Gmail, "gmail-1").await;

    // Calendar: a "Trip to Rome" event spanning 2026-05-03 .. 2026-05-07.
    let first = normalize_and_insert(
        &kg,
        vec![rome_event(
            "cal-evt-123",
            Some(parse_dt("2026-05-07T00:00:00Z")),
        )],
        Provenance::connector(
            calendar_instance,
            ConnectorType::Calendar,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("calendar insert should succeed");
    assert_eq!(first.inserted.len(), 1);
    let calendar_fact_id = first.inserted[0].id;
    let calendar_confidence = first.inserted[0].confidence;
    assert!(
        (calendar_confidence - 0.90).abs() < 1e-5,
        "Calendar initial confidence should be 0.90, got {calendar_confidence}"
    );

    // Gmail: a flight booking email describing the same trip, overlapping date.
    let second = normalize_and_insert(
        &kg,
        vec![rome_event("gmail-msg-456", None)],
        Provenance::connector(
            gmail_instance,
            ConnectorType::Gmail,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("gmail corroboration should succeed");

    // Corroboration folds the Gmail fact into the existing Calendar fact:
    // `insert_fact_in_tx` returns the existing fact rather than creating a new
    // row, so the returned "inserted" fact is the *same* Calendar fact id.
    assert!(second.errors.is_empty(), "errors: {:?}", second.errors);
    assert_eq!(second.inserted.len(), 1);
    assert_eq!(
        second.inserted[0].id, calendar_fact_id,
        "corroboration must return the existing fact, not create a new one"
    );

    // Exactly one fact row exists for the Rome claim — no duplicate.
    let devansh = kg
        .create_entity("Devansh", EntityType::Person, &[])
        .await
        .unwrap()
        .id;
    let facts = kg.get_facts_by_subject(devansh, 100).await.unwrap();
    assert_eq!(facts.len(), 1, "no duplicate fact should be created");
    assert_eq!(facts[0].id, calendar_fact_id);

    // Two independent sources now back the single fact (one per connector).
    let sources = kg.get_sources_for_fact(calendar_fact_id).await.unwrap();
    assert_eq!(
        sources.len(),
        2,
        "both connector sources should be recorded"
    );
    let instance_ids: std::collections::HashSet<Option<i32>> =
        sources.iter().map(|s| s.connector_instance_id).collect();
    assert!(instance_ids.contains(&Some(calendar_instance)));
    assert!(instance_ids.contains(&Some(gmail_instance)));

    // Confidence boosted by +0.05 per independent source, capped at 0.95.
    let corroborated = kg.get_fact(calendar_fact_id).await.unwrap().unwrap();
    assert!(
        (corroborated.confidence - 0.95).abs() < 1e-5,
        "corroborated confidence should be capped at 0.95, got {}",
        corroborated.confidence
    );
}

#[tokio::test]
async fn chat_provenance_inserts_with_interaction_confidence() {
    let (kg, _dir) = fresh_kg().await;

    let fact = NormalizedFact {
        source_type: SourceType::Interaction,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "favourite_colour".to_string(),
        object: "blue".to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from: None,
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: None,
        location: None,
    };

    // Chat casual learning: no connector, LLM extraction method.
    let outcome = normalize_and_insert(
        &kg,
        vec![fact],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .expect("chat insert should succeed");
    assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
    assert_eq!(outcome.inserted.len(), 1);

    let inserted = &outcome.inserted[0];
    assert!(
        (inserted.confidence - confidence::initial(SourceType::Interaction, None)).abs() < 1e-5,
        "casual chat confidence should be 0.30, got {}",
        inserted.confidence
    );

    let sources = kg.get_sources_for_fact(inserted.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_type_id, SourceType::Interaction as i16);
    assert!(sources[0].connector_instance_id.is_none());
    assert_eq!(
        sources[0].extraction_method_id,
        Some(ExtractionMethod::LlmExtraction as i16)
    );
}

#[tokio::test]
async fn connector_fact_missing_raw_reference_is_rejected() {
    let (kg, _dir) = fresh_kg().await;
    let calendar_instance = upsert(&kg, ConnectorType::Calendar, "calendar-1").await;

    let mut fact = rome_event("cal-evt-123", None);
    fact.raw_reference = None; // missing — must be rejected by the provenance gate

    let outcome = normalize_and_insert(
        &kg,
        vec![fact],
        Provenance::connector(
            calendar_instance,
            ConnectorType::Calendar,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("batch processing does not abort on per-fact errors");
    assert_eq!(outcome.errors.len(), 1);
    assert!(matches!(
        outcome.errors[0],
        KnowledgeError::Validation(ref msg) if msg.contains("raw_reference")
    ));
    assert!(outcome.inserted.is_empty());
}

#[tokio::test]
async fn sensitive_fact_persists_its_catalogue_categories() {
    let (kg, _dir) = fresh_kg().await;
    let gmail_instance = upsert(&kg, ConnectorType::Gmail, "gmail-1").await;

    // A connector-sourced allergy flagged sensitive with the Allergies (230)
    // category lands as pending_confirmation via insert_sensitive_fact. Its
    // category links must be persisted exactly like the normal insert path so
    // category-based reads and downstream sensitivity logic see them.
    let fact = NormalizedFact {
        source_type: SourceType::Connector,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "allergy".to_string(),
        object: "peanuts".to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from: None,
        valid_until: None,
        is_sensitive: true,
        is_correction: false,
        correction_scope: None,
        category_ids: vec![230],
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: Some("gmail-msg-42".to_string()),
        location: None,
    };

    let outcome = normalize_and_insert(
        &kg,
        vec![fact],
        Provenance::connector(
            gmail_instance,
            ConnectorType::Gmail,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("sensitive connector insert should succeed");

    assert_eq!(outcome.pending_confirmation.len(), 1, "{:?}", outcome);
    let pending = &outcome.pending_confirmation[0];
    let categories = kg.get_categories_for_fact(pending.fact_id).await.unwrap();
    assert!(
        categories.iter().any(|c| c.id == 230),
        "sensitive fact missing its catalogue category; got {:?}",
        categories
    );
}

// ---------------------------------------------------------------------------
// Entity resolution chain (Phase 3 F5 / issue #182)
//
// End-to-end resolution through `normalize_and_insert`: exact name → alias →
// FTS5 fuzzy (>= threshold) → create new, with strict same-type filtering.
// The pure decision policy (threshold boundary) is unit-tested in
// `normalize::resolution_tests`; these integration tests cover the real
// resolve path against the SQLite FTS5 index.
//
// FTS5 bm25 IDF is corpus-sensitive: a query token that appears in most/all
// documents scores ~0 and is filtered out by the `rank <= -0.2` gate. The fuzzy
// tests therefore seed a handful of distractor entities so query tokens have a
// positive IDF — mirroring a real, populated knowledge graph.
// ---------------------------------------------------------------------------

/// Build a simple subject-fact with a literal object so only the subject name
/// is resolved. `favourite_colour` is seeded and stays non-sensitive.
fn subject_fact(subject: &str, subject_type: EntityType, object: &str) -> NormalizedFact {
    NormalizedFact {
        source_type: SourceType::Interaction,
        subject: subject.to_string(),
        subject_type,
        relationship_type: "favourite_colour".to_string(),
        object: object.to_string(),
        object_is_entity: false,
        object_type: None,
        valid_from: None,
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: None,
        location: None,
    }
}

/// Seed unrelated entities so FTS5 query tokens get a positive IDF. Without
/// these, a token present in the only indexed document has IDF ≈ 0 and the
/// `rank <= -0.2` gate suppresses the fuzzy match.
async fn seed_distractors(kg: &KnowledgeGraph) {
    for name in [
        "Berlin", "Tokyo", "Madrid", "Vienna", "Prague", "Boston", "Seattle", "Dublin",
    ] {
        kg.create_entity(name, EntityType::Place, &[])
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn create_on_miss_creates_new_entity() {
    let (kg, _dir) = fresh_kg().await;

    let outcome = normalize_and_insert(
        &kg,
        vec![subject_fact("Ada Lovelace", EntityType::Person, "blue")],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let new_id = outcome.inserted[0].subject_id;
    let entity = kg.get_entity(new_id).await.unwrap().unwrap();
    assert_eq!(entity.name, "Ada Lovelace");
    assert_eq!(entity.entity_type_id, EntityType::Person as i16);
}

#[tokio::test]
async fn exact_name_resolves_to_existing_entity() {
    let (kg, _dir) = fresh_kg().await;
    let canonical = kg
        .create_entity("Ada Lovelace", EntityType::Person, &[])
        .await
        .unwrap();

    let outcome = normalize_and_insert(
        &kg,
        vec![subject_fact("Ada Lovelace", EntityType::Person, "green")],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(outcome.inserted[0].subject_id, canonical.id);
    // No duplicate entity was created.
    let search = kg.search_entities("Ada Lovelace", 10).await.unwrap();
    let matching: Vec<_> = search
        .iter()
        .filter(|r| r.entity.name == "Ada Lovelace")
        .collect();
    assert_eq!(matching.len(), 1, "expected a single canonical entity");
}

#[tokio::test]
async fn alias_match_resolves_to_existing_entity() {
    let (kg, _dir) = fresh_kg().await;
    let canonical = kg
        .create_entity("John Smith", EntityType::Person, &["J. Smith"])
        .await
        .unwrap();

    let outcome = normalize_and_insert(
        &kg,
        vec![subject_fact("J. Smith", EntityType::Person, "red")],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(
        outcome.inserted[0].subject_id, canonical.id,
        "alias 'J. Smith' should resolve to the canonical 'John Smith' entity"
    );
}

#[tokio::test]
async fn fts5_fuzzy_match_resolves_to_existing_entity() {
    let (kg, _dir) = fresh_kg().await;
    seed_distractors(&kg).await;
    // Canonical name is multi-token; a single-token query that is *not* an
    // exact name or alias exercises the FTS5 fuzzy branch. With distractors
    // seeded, "John" has a positive IDF and scores 1.0 (>= 0.9 threshold).
    let canonical = kg
        .create_entity("John Smith", EntityType::Person, &[])
        .await
        .unwrap();

    let outcome = normalize_and_insert(
        &kg,
        vec![subject_fact("John", EntityType::Person, "yellow")],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    assert_eq!(
        outcome.inserted[0].subject_id, canonical.id,
        "fuzzy query 'John' should resolve to 'John Smith'"
    );
}

#[tokio::test]
async fn cross_type_fuzzy_match_creates_new_entity() {
    let (kg, _dir) = fresh_kg().await;
    seed_distractors(&kg).await;
    // "Apple" is a token in the Organization "Apple Inc". Resolving "Apple" as
    // a Concept must NOT merge into the cross-type Organization: strict
    // same-type filtering drops the fuzzy hit and a new Concept entity is
    // created instead. (Entity names are globally unique by LOWER(name), so
    // the cross-type guard matters for token-overlap/fuzzy matches, not for
    // identical names which cannot coexist anyway.)
    let org = kg
        .create_entity("Apple Inc", EntityType::Organization, &[])
        .await
        .unwrap();

    let outcome = normalize_and_insert(
        &kg,
        vec![subject_fact("Apple", EntityType::Concept, "crisp")],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    assert_eq!(outcome.inserted.len(), 1);
    let concept_id = outcome.inserted[0].subject_id;
    assert_ne!(
        concept_id, org.id,
        "Concept query must not fuzzy-resolve into a cross-type Organization"
    );
    let concept = kg.get_entity(concept_id).await.unwrap().unwrap();
    assert_eq!(concept.name, "Apple");
    assert_eq!(concept.entity_type_id, EntityType::Concept as i16);
}

// ---------------------------------------------------------------------------
// Phase 3 C2 / #196: Photos connector GPS → place fact + corroboration
// ---------------------------------------------------------------------------

/// A `took_photo_at <place>` Photos connector fact with a GPS location overlay
/// (Phase 3 C2 / #196). `raw_ref` is the photo's file path (the native source
/// id); `place` is the reverse-geocoded locality that becomes the `Place`
/// object entity.
fn took_photo_at_fact(raw_ref: &str, place: &str, lat: f64, lng: f64) -> NormalizedFact {
    NormalizedFact {
        source_type: SourceType::Connector,
        subject: "Devansh".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "took_photo_at".to_string(),
        object: place.to_string(),
        object_is_entity: true,
        object_type: Some(EntityType::Place),
        valid_from: Some(parse_dt("2024-05-15T14:30:00Z")),
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        requires_user_action: false,
        raw_reference: Some(raw_ref.to_string()),
        location: Some(NormalizedLocation {
            location_type: LocationType::Visited,
            address: Some(place.to_string()),
            latitude: Some(lat),
            longitude: Some(lng),
            timezone: None,
        }),
    }
}

/// Two photos at the same place corroborate into one `took_photo_at` fact with
/// two sources, a boosted confidence, and a `Geographic` coordinate row
/// anchoring the place entity (Phase 3 C2 / #196 acceptance).
#[tokio::test]
async fn photos_at_same_place_corroborate_and_anchor_place_coords() {
    let (kg, _dir) = fresh_kg().await;
    let photos_instance = upsert(&kg, ConnectorType::Photos, "photos-1").await;

    // Two photos at the same GPS/timestamp — only their file paths differ, so
    // they are independent sources for the same "Devansh took a photo at Rome"
    // claim. They are ingested in separate `normalize_and_insert` calls (two
    // connector syncs) with the overlay worker flushed between them, so the
    // background overlay writes never contend with a concurrent fact insert
    // (SQLite "database is locked").
    let first = normalize_and_insert(
        &kg,
        vec![took_photo_at_fact("IMG_001.jpg", "Rome", 46.5, 7.5)],
        Provenance::connector(
            photos_instance,
            ConnectorType::Photos,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("photos insert should succeed");
    assert!(first.errors.is_empty(), "errors: {:?}", first.errors);
    assert_eq!(first.inserted.len(), 1);
    let fact_id = first.inserted[0].id;
    // Drain the first photo's overlay before the corroborating insert.
    kg.flush_location_overlays().await;

    // Second photo: same place/time, different file → corroborates the first.
    let second = normalize_and_insert(
        &kg,
        vec![took_photo_at_fact("IMG_002.jpg", "Rome", 46.5, 7.5)],
        Provenance::connector(
            photos_instance,
            ConnectorType::Photos,
            ExtractionMethod::StructuredParse,
        ),
    )
    .await
    .expect("photos corroboration should succeed");
    assert!(second.errors.is_empty(), "errors: {:?}", second.errors);
    // Corroboration returns the existing fact, not a new row.
    assert_eq!(second.inserted.len(), 1);
    assert_eq!(
        second.inserted[0].id, fact_id,
        "corroboration must return the existing fact, not create a new one"
    );

    // Drain the second photo's overlay (an idempotent place re-anchor).
    kg.flush_location_overlays().await;

    // Two independent photo sources back the single fact.
    let sources = kg.get_sources_for_fact(fact_id).await.unwrap();
    assert_eq!(sources.len(), 2, "both photo sources should be recorded");
    let raw_refs: std::collections::HashSet<String> = sources
        .iter()
        .filter_map(|s| s.raw_reference.clone())
        .collect();
    assert!(raw_refs.contains("IMG_001.jpg"));
    assert!(raw_refs.contains("IMG_002.jpg"));

    // Photos base 0.80 + one independent corroborating source (+0.05) = 0.85.
    let corroborated = kg.get_fact(fact_id).await.unwrap().unwrap();
    assert!(
        (corroborated.confidence - 0.85).abs() < 1e-5,
        "corroborated confidence should be 0.85, got {}",
        corroborated.confidence
    );

    // The place entity was created and is the fact's object.
    let place = kg
        .search_entities("Rome", 10)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.entity.name == "Rome" && r.entity.entity_type_id == EntityType::Place as i16)
        .map(|r| r.entity.id)
        .expect("Rome place entity should be created");
    assert_eq!(corroborated.object_id, Some(place));

    // Flush the overlay worker: the place is anchored with Geographic coords,
    // and the owner has a Visited row carrying the place name + coords.
    kg.flush_location_overlays().await;
    let place_locs = kg.get_locations(place).await.unwrap();
    assert!(
        place_locs.iter().any(|loc| {
            loc.location_type_id == LocationType::Geographic as i16
                && (loc.latitude.unwrap() - 46.5).abs() < 1e-6
                && (loc.longitude.unwrap() - 7.5).abs() < 1e-6
                && loc.source_fact_id == Some(fact_id)
        }),
        "place should be anchored with a Geographic coordinate row; got {place_locs:?}"
    );
}
