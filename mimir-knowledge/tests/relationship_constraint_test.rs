//! Relationship-type constraint enforcement (issue #402).
//!
//! Migration 013 seeded `predicate_constraints` (renamed to
//! `relationship_constraints` by migration 031) as a permissive allow-list of
//! subject/object entity-type pairs per predicate, with strict enforcement
//! promised in app code. The enforcement never materialised: `validate_predicate`
//! had no production call sites and every insert path accepted nonsense
//! combinations such as `born_on` with a non-DateTime object. These tests pin
//! the enforcement contract on every insert path:
//!
//! - entity-object facts must use a seeded (subject, object) type pair;
//! - predicates without seeded constraints accept any entity types;
//! - literal-object facts carry no object type and always pass.

mod common;

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::RecurrenceType;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::{NormalizedFact, Provenance, normalize_and_insert};

async fn create_entity(kg: &KnowledgeGraph, name: &str, entity_type: EntityType) -> i32 {
    kg.create_entity(name, entity_type, &[]).await.unwrap().id
}

fn new_fact(subject_id: i32, predicate: &str, object_id: Option<i32>) -> NewFact {
    let mut fact = NewFact::new(subject_id, predicate);
    fact.object_id = object_id;
    fact.source_type = SourceType::UserEdit;
    fact
}

fn normalized_fact(
    subject: &str,
    subject_type: EntityType,
    predicate: &str,
    object: &str,
    object_type: EntityType,
    is_sensitive: bool,
) -> NormalizedFact {
    NormalizedFact {
        confidence: None,
        source_type: SourceType::Interaction,
        subject: subject.to_string(),
        subject_type,
        relationship_type: predicate.to_string(),
        object: object.to_string(),
        object_is_entity: true,
        object_type: Some(object_type),
        valid_from: None,
        valid_until: None,
        is_sensitive,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        recurrence_rule: None,
        recurrence_interval: 1,
        recurrence_until: None,
        requires_user_action: false,
        raw_reference: None,
        extraction_method: None,
        event_type: None,
        location: None,
    }
}

// ---------------------------------------------------------------------------
// Single-insert path (`KnowledgeGraph::insert_fact`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn insert_rejects_invalid_entity_combo() {
    let tg = common::TestGraph::new().await;
    let place = create_entity(&tg.kg, "Stonehenge", EntityType::Place).await;
    let person = create_entity(&tg.kg, "Alice", EntityType::Person).await;

    // `born_on` only allows Person/Organization -> DateTime.
    let result = tg
        .kg
        .insert_fact(new_fact(place, "born_on", Some(person)))
        .await;
    assert!(
        result.is_err(),
        "Place born_on Person must be rejected by the seeded constraint"
    );
}

#[tokio::test]
async fn insert_accepts_valid_entity_combo() {
    let tg = common::TestGraph::new().await;
    let person = create_entity(&tg.kg, "Alice", EntityType::Person).await;
    let date_time = create_entity(&tg.kg, "1990-06-15", EntityType::DateTime).await;

    let fact = tg
        .kg
        .insert_fact(new_fact(person, "born_on", Some(date_time)))
        .await
        .unwrap();
    assert_eq!(fact.object_id, Some(date_time));
}

#[tokio::test]
async fn insert_allows_unconstrained_predicates() {
    let tg = common::TestGraph::new().await;
    let person = create_entity(&tg.kg, "Alice", EntityType::Person).await;
    let place = create_entity(&tg.kg, "Paris", EntityType::Place).await;

    // `likes` has no seeded constraint rows: any type pair is accepted.
    let likes_id = common::ensure_relationship_type(&tg.kg, "likes")
        .await
        .unwrap();
    let fact = tg
        .kg
        .insert_fact(new_fact(person, "likes", Some(place)))
        .await
        .unwrap();
    assert_eq!(fact.relationship_type_id, likes_id);
}

#[tokio::test]
async fn insert_allows_literal_objects_for_constrained_predicates() {
    let tg = common::TestGraph::new().await;
    let person = create_entity(&tg.kg, "Alice", EntityType::Person).await;

    // A literal object has no entity type, so the constraint check is skipped.
    let mut fact = NewFact::new(person, "born_on");
    fact.object_literal = Some("1990-06-15".to_string());
    fact.source_type = SourceType::UserEdit;
    let fact = tg.kg.insert_fact(fact).await.unwrap();
    assert_eq!(fact.object_literal.as_deref(), Some("1990-06-15"));
}

// ---------------------------------------------------------------------------
// Batch path (`KnowledgeGraph::insert_facts_batch`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_insert_rejects_invalid_combo() {
    let tg = common::TestGraph::new().await;
    let place = create_entity(&tg.kg, "Stonehenge", EntityType::Place).await;
    let person = create_entity(&tg.kg, "Alice", EntityType::Person).await;

    let result = tg
        .kg
        .insert_facts_batch(vec![new_fact(place, "born_on", Some(person))])
        .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Shared normalize pipeline (`normalize_and_insert`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn normalize_pipeline_reports_constraint_violation_per_fact() {
    let tg = common::TestGraph::new().await;

    let outcome = normalize_and_insert(
        &tg.kg,
        vec![
            normalized_fact(
                "Stonehenge",
                EntityType::Place,
                "born_on",
                "Alice",
                EntityType::Person,
                false,
            ),
            normalized_fact(
                "Alice",
                EntityType::Person,
                "has_partner",
                "Bob",
                EntityType::Person,
                false,
            ),
        ],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    assert_eq!(outcome.inserted.len(), 1, "the valid fact must insert");
    assert_eq!(outcome.errors.len(), 1, "the invalid fact must be reported");
    assert!(matches!(
        outcome.errors[0],
        mimir_knowledge::KnowledgeError::InvalidRelationshipConstraint(_)
    ));
}

#[tokio::test]
async fn sensitive_facts_are_constraint_checked() {
    let tg = common::TestGraph::new().await;

    // Category 420 (Romantic) plus the LLM flag makes the fact sensitive per
    // the Rust gate, so it routes to the pending-confirmation insert path —
    // which must enforce constraints too.
    let mut fact = normalized_fact(
        "Stonehenge",
        EntityType::Place,
        "has_partner",
        "Alice",
        EntityType::Person,
        true,
    );
    fact.category_ids = vec![420];

    let outcome = normalize_and_insert(
        &tg.kg,
        vec![fact],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();

    assert!(
        outcome.pending_confirmation.is_empty(),
        "an invalid sensitive fact must not land in pending confirmation"
    );
    assert_eq!(outcome.errors.len(), 1);
    assert!(matches!(
        outcome.errors[0],
        mimir_knowledge::KnowledgeError::InvalidRelationshipConstraint(_)
    ));
}

// ---------------------------------------------------------------------------
// Public typed validator (`queries::entity::validate_predicate`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_predicate_rejects_invalid_and_allows_unconstrained() {
    let tg = common::TestGraph::new().await;
    let born_on = common::ensure_relationship_type(&tg.kg, "born_on")
        .await
        .unwrap();
    let likes = common::ensure_relationship_type(&tg.kg, "likes")
        .await
        .unwrap();

    assert!(
        mimir_knowledge::queries::entity::validate_predicate(
            tg.kg.pool(),
            EntityType::Place,
            born_on,
            EntityType::Person,
        )
        .await
        .is_err()
    );

    assert!(
        mimir_knowledge::queries::entity::validate_predicate(
            tg.kg.pool(),
            EntityType::Person,
            born_on,
            EntityType::DateTime,
        )
        .await
        .is_ok()
    );

    assert!(
        mimir_knowledge::queries::entity::validate_predicate(
            tg.kg.pool(),
            EntityType::Person,
            likes,
            EntityType::Person,
        )
        .await
        .is_ok()
    );
}
