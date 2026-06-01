use mimir_knowledge::models::enums::{ConnectorType, Predicate};
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;
use mimir_knowledge::{KnowledgeGraph, confidence};

async fn create_person(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(
        name,
        mimir_knowledge::models::entity::EntityType::Person,
        &[],
    )
    .await
    .unwrap()
    .id
}

async fn create_place(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(
        name,
        mimir_knowledge::models::entity::EntityType::Place,
        &[],
    )
    .await
    .unwrap()
    .id
}

// ---------------------------------------------------------------------------
// Unit-level formula tests
// ---------------------------------------------------------------------------

#[test]
fn test_initial_confidence_by_source_type() {
    assert_eq!(confidence::initial(SourceType::UserEdit, None), 1.0);
    assert_eq!(confidence::initial(SourceType::System, None), 1.0);
    assert_eq!(confidence::initial(SourceType::Interaction, None), 0.30);
    assert_eq!(confidence::initial(SourceType::Import, None), 0.80);
    assert_eq!(confidence::initial(SourceType::Inference, None), 0.0);
}

#[test]
fn test_connector_initial_uses_reliability_score() {
    assert_eq!(
        confidence::initial(SourceType::Connector, Some(ConnectorType::Calendar)),
        0.90
    );
    assert_eq!(
        confidence::initial(SourceType::Connector, Some(ConnectorType::Gmail)),
        0.85
    );
    assert_eq!(
        confidence::initial(SourceType::Connector, Some(ConnectorType::Photos)),
        0.80
    );
    assert_eq!(
        confidence::initial(SourceType::Connector, Some(ConnectorType::LinkedIn)),
        0.75
    );
    assert_eq!(confidence::initial(SourceType::Connector, None), 0.80);
}

#[test]
fn test_inference_confidence_formula_single_positive() {
    let parents = vec![(0.90, true)];
    let conf = confidence::inference_confidence(&parents, 1, 1);
    assert!(
        (conf - 0.432).abs() < 0.001,
        "expected ~0.432, got {}",
        conf
    );
}

#[test]
fn test_inference_confidence_two_positives() {
    let parents = vec![(0.90, true), (0.90, true)];
    let conf = confidence::inference_confidence(&parents, 1, 2);
    assert!((conf - 0.95).abs() < 0.001, "expected 0.95, got {}", conf);
}

#[test]
fn test_inference_confidence_negative_parent() {
    let parents = vec![(0.90, true), (0.70, false)];
    let conf = confidence::inference_confidence(&parents, 1, 2);
    assert!((conf - 0.12).abs() < 0.001, "expected ~0.12, got {}", conf);
}

#[test]
fn test_inference_confidence_orphaned() {
    let parents: Vec<(f32, bool)> = vec![];
    let conf = confidence::inference_confidence(&parents, 1, 0);
    assert_eq!(conf, 0.0);
}

#[test]
fn test_inference_confidence_strong_corroboration() {
    let parents = vec![(1.0, true), (1.0, true), (1.0, true)];
    let conf = confidence::inference_confidence(&parents, 1, 3);
    assert!((conf - 0.95).abs() < 0.001, "expected 0.95, got {}", conf);
}

#[test]
fn test_inference_confidence_perfect_conflict() {
    let parents = vec![(0.90, true), (0.90, false)];
    let conf = confidence::inference_confidence(&parents, 1, 2);
    assert_eq!(conf, 0.0);
}

#[test]
fn test_inference_confidence_negative_removal_boosts() {
    let parents_before = vec![(0.90, true), (0.70, false)];
    let conf_before = confidence::inference_confidence(&parents_before, 1, 2);

    let parents_after = vec![(0.90, true)];
    let conf_after = confidence::inference_confidence(&parents_after, 1, 1);

    assert!(
        conf_after > conf_before,
        "expected confidence to rise after removing negative parent: {} -> {}",
        conf_before,
        conf_after
    );
}

#[test]
fn test_inference_confidence_positive_removal_drops() {
    let parents_before = vec![(0.90, true), (0.80, true)];
    let conf_before = confidence::inference_confidence(&parents_before, 1, 2);

    let parents_after = vec![(0.90, true)];
    let conf_after = confidence::inference_confidence(&parents_after, 1, 1);

    assert!(
        conf_after < conf_before,
        "expected confidence to drop after removing positive parent: {} -> {}",
        conf_before,
        conf_after
    );
}

// ---------------------------------------------------------------------------
// Connector reliability integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_connector_reliability_feedback() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let initial = kg
        .connector_reliability(ConnectorType::Gmail)
        .await
        .unwrap();
    assert!((initial - 0.85).abs() < 1e-4);

    kg.adjust_connector_reliability(ConnectorType::Gmail, -0.02)
        .await
        .unwrap();
    let after_drop = kg
        .connector_reliability(ConnectorType::Gmail)
        .await
        .unwrap();
    assert!((after_drop - 0.83).abs() < 1e-4);

    kg.adjust_connector_reliability(ConnectorType::Gmail, 0.01)
        .await
        .unwrap();
    let after_rise = kg
        .connector_reliability(ConnectorType::Gmail)
        .await
        .unwrap();
    assert!((after_rise - 0.84).abs() < 1e-4);
}

#[tokio::test]
async fn test_connector_reliability_clamped() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    kg.adjust_connector_reliability(ConnectorType::Gmail, -1.0)
        .await
        .unwrap();
    let score = kg
        .connector_reliability(ConnectorType::Gmail)
        .await
        .unwrap();
    assert_eq!(score, 0.0);

    kg.adjust_connector_reliability(ConnectorType::Gmail, 2.0)
        .await
        .unwrap();
    let score = kg
        .connector_reliability(ConnectorType::Gmail)
        .await
        .unwrap();
    assert_eq!(score, 1.0);
}

// ---------------------------------------------------------------------------
// Fact confidence integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_connector_reliability_defaults_match_migration() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    for &(ct, expected) in &[
        (
            ConnectorType::Gmail,
            confidence::default_connector_score(ConnectorType::Gmail),
        ),
        (
            ConnectorType::Calendar,
            confidence::default_connector_score(ConnectorType::Calendar),
        ),
        (
            ConnectorType::Photos,
            confidence::default_connector_score(ConnectorType::Photos),
        ),
        (
            ConnectorType::LinkedIn,
            confidence::default_connector_score(ConnectorType::LinkedIn),
        ),
    ] {
        let actual = kg.connector_reliability(ct).await.unwrap();
        assert!(
            (actual - expected).abs() < 1e-4,
            "Mismatch for {:?}: expected {}, got {}",
            ct,
            expected,
            actual
        );
    }
}

#[tokio::test]
async fn test_user_edit_confidence_is_one() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            raw_reference: None,
            extraction_method: None,
            connector_type: None,
        })
        .await
        .unwrap();

    assert!((fact.confidence - 1.0).abs() < 1e-4);
}

#[tokio::test]
async fn test_casual_mention_confidence_is_low() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Interaction,
            connector_id: None,
            raw_reference: None,
            extraction_method: None,
            connector_type: None,
        })
        .await
        .unwrap();

    assert!((fact.confidence - 0.30).abs() < 1e-4);
}

#[tokio::test]
async fn test_system_confidence_is_one() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::System,
            connector_id: None,
            raw_reference: None,
            extraction_method: None,
            connector_type: None,
        })
        .await
        .unwrap();

    assert!((fact.confidence - 1.0).abs() < 1e-4);
}

#[tokio::test]
async fn test_import_confidence_is_eighty() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Import,
            connector_id: None,
            raw_reference: None,
            extraction_method: None,
            connector_type: None,
        })
        .await
        .unwrap();

    assert!((fact.confidence - 0.80).abs() < 1e-4);
}

#[tokio::test]
async fn test_connector_confidence_uses_db_reliability() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Adjust Gmail reliability away from the default.
    kg.adjust_connector_reliability(ConnectorType::Gmail, -0.02)
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: Predicate::IsIn,
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Connector,
            connector_id: Some("gmail-1".to_string()),
            connector_type: Some(ConnectorType::Gmail),
            raw_reference: Some("msg-123".to_string()),
            extraction_method: Some(
                mimir_knowledge::models::source::ExtractionMethod::StructuredParse,
            ),
        })
        .await
        .unwrap();

    assert!((fact.confidence - 0.83).abs() < 1e-4);
}
