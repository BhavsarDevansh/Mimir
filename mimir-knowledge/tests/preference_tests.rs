//! Preference system tests (Issue #53).

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::Predicate;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::preference::{
    NewPreference, PreferenceCategory, PreferenceSourceType, UpsertAction, UpsertPreferenceInput,
};
use mimir_knowledge::models::source::SourceType;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn create_person(kg: &KnowledgeGraph, name: &str) -> i32 {
    kg.create_entity(name, EntityType::Person, &[])
        .await
        .unwrap()
        .id
}

async fn create_has_preference_fact(kg: &KnowledgeGraph, subject_id: i32) -> i32 {
    let fact = kg
        .insert_fact(NewFact {
            subject_id,
            predicate: Predicate::HasPreference,
            object_id: None,
            object_literal: Some("pref".to_string()),
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
        })
        .await
        .unwrap();
    fact.id
}

fn upsert_input(
    entity_id: Option<i32>,
    key: &str,
    value: &str,
    confidence: f32,
    overridden: bool,
    source_fact_id: i32,
    contexts: Vec<(&str, &str)>,
) -> UpsertPreferenceInput {
    UpsertPreferenceInput {
        preference: NewPreference {
            entity_id,
            category: PreferenceCategory::General,
            key: key.to_string(),
            value: value.to_string(),
            confidence,
            overridden_by_user: overridden,
            source_fact_id,
        },
        changed_by: ChangedBy::User,
        contexts: contexts
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        sources: vec![(PreferenceSourceType::UserEdit, "test".to_string())],
    }
}

// ---------------------------------------------------------------------------
// 1. Migration applies cleanly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_023_creates_new_preference_tables() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let tables: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(kg.pool())
            .await
            .unwrap();

    let names: Vec<String> = tables.into_iter().map(|r| r.0).collect();
    assert!(names.contains(&"preferences".to_string()));
    assert!(names.contains(&"preference_contexts".to_string()));
    assert!(names.contains(&"preference_sources".to_string()));
    assert!(names.contains(&"preference_audit_log".to_string()));

    // Verify new schema has source_fact_id
    let cols: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM pragma_table_info('preferences') ORDER BY name")
            .fetch_all(kg.pool())
            .await
            .unwrap();
    let col_names: Vec<String> = cols.into_iter().map(|r| r.0).collect();
    assert!(col_names.contains(&"source_fact_id".to_string()));
}

// ---------------------------------------------------------------------------
// 2. Insert roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn insert_preference_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact_id = create_has_preference_fact(&kg, alice).await;

    let input = upsert_input(
        Some(alice),
        "theme",
        "dark",
        0.9,
        false,
        fact_id,
        vec![("time_of_day", "evening")],
    );

    let pref = kg.insert_preference(input).await.unwrap();
    assert_eq!(pref.entity_id, Some(alice));
    assert_eq!(pref.key, "theme");
    assert_eq!(pref.value, "dark");
    assert!((pref.confidence - 0.9).abs() < f32::EPSILON);
    assert_eq!(pref.source_fact_id, fact_id);

    let contexts = kg.get_preference_contexts(pref.id).await.unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].context_key, "time_of_day");
    assert_eq!(contexts[0].context_value, "evening");

    let sources = kg.get_preference_sources(pref.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources[0].source_type_id,
        PreferenceSourceType::UserEdit as i16
    );

    let audit = kg.get_preference_audit_log(pref.id).await.unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].change_type_id, 1); // Created
}

// ---------------------------------------------------------------------------
// 3. Duplicate rejection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_preference_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact_id = create_has_preference_fact(&kg, alice).await;

    let input = upsert_input(
        Some(alice),
        "theme",
        "dark",
        0.9,
        false,
        fact_id,
        vec![("time_of_day", "evening")],
    );

    kg.insert_preference(input.clone()).await.unwrap();
    let result = kg.insert_preference(input).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("Duplicate"),
        "Expected duplicate error, got: {}",
        err_msg
    );
}

// ---------------------------------------------------------------------------
// 4. Conflict resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn explicit_overrides_inferred() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact1 = create_has_preference_fact(&kg, alice).await;
    let fact2 = create_has_preference_fact(&kg, alice).await;

    let inferred = upsert_input(Some(alice), "theme", "light", 0.6, false, fact1, vec![]);

    let (old_pref, action) = kg.upsert_preference(inferred).await.unwrap();
    assert_eq!(action, UpsertAction::Created);
    let old_id = old_pref.id;

    let explicit = upsert_input(
        Some(alice),
        "theme",
        "dark",
        0.5, // lower confidence but explicit
        true,
        fact2,
        vec![],
    );

    let (pref, action) = kg.upsert_preference(explicit).await.unwrap();
    assert_eq!(action, UpsertAction::Overwritten);
    assert_eq!(pref.value, "dark");

    let audit = kg.get_preference_audit_log(old_id).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.reason.as_deref() == Some("overridden by user"))
    );
}

#[tokio::test]
async fn higher_confidence_inferred_wins() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact1 = create_has_preference_fact(&kg, alice).await;
    let fact2 = create_has_preference_fact(&kg, alice).await;

    let low = upsert_input(Some(alice), "theme", "light", 0.6, false, fact1, vec![]);

    let (pref, action) = kg.upsert_preference(low).await.unwrap();
    assert_eq!(action, UpsertAction::Created);
    let first_id = pref.id;

    let high = upsert_input(Some(alice), "theme", "dark", 0.8, false, fact2, vec![]);

    let (pref, action) = kg.upsert_preference(high).await.unwrap();
    assert_eq!(action, UpsertAction::Overwritten);
    assert_eq!(pref.value, "dark");
    assert_ne!(pref.id, first_id);

    let audit = kg.get_preference_audit_log(first_id).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.reason.as_deref() == Some("higher confidence inferred preference"))
    );
}

#[tokio::test]
async fn same_confidence_keeps_existing() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact1 = create_has_preference_fact(&kg, alice).await;
    let fact2 = create_has_preference_fact(&kg, alice).await;

    let first = upsert_input(Some(alice), "theme", "light", 0.7, false, fact1, vec![]);

    let (pref, action) = kg.upsert_preference(first).await.unwrap();
    assert_eq!(action, UpsertAction::Created);
    let first_id = pref.id;

    let second = upsert_input(Some(alice), "theme", "dark", 0.7, false, fact2, vec![]);

    let (pref, action) = kg.upsert_preference(second).await.unwrap();
    assert_eq!(action, UpsertAction::KeptAsPrimary);
    assert_eq!(pref.id, first_id);
    assert_eq!(pref.value, "light");
}

#[tokio::test]
async fn user_override_blocks_inferred_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact1 = create_has_preference_fact(&kg, alice).await;
    let fact2 = create_has_preference_fact(&kg, alice).await;

    let explicit = upsert_input(Some(alice), "theme", "dark", 0.5, true, fact1, vec![]);

    let (pref, action) = kg.upsert_preference(explicit).await.unwrap();
    assert_eq!(action, UpsertAction::Created);
    let first_id = pref.id;

    let inferred = upsert_input(Some(alice), "theme", "light", 0.9, false, fact2, vec![]);

    let (pref, action) = kg.upsert_preference(inferred).await.unwrap();
    assert_eq!(action, UpsertAction::Rejected);
    assert_eq!(pref.id, first_id);
    assert_eq!(pref.value, "dark");
}

// ---------------------------------------------------------------------------
// 5. Contextual lookup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contextual_lookup_default_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact = create_has_preference_fact(&kg, alice).await;

    let default = upsert_input(Some(alice), "theme", "light", 0.8, false, fact, vec![]);

    kg.upsert_preference(default).await.unwrap();

    let result = kg
        .get_preference(
            Some(alice),
            "theme",
            &[("time_of_day".to_string(), "morning".to_string())],
        )
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().value, "light");
}

#[tokio::test]
async fn contextual_lookup_specific_wins_over_default() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact1 = create_has_preference_fact(&kg, alice).await;
    let fact2 = create_has_preference_fact(&kg, alice).await;

    let default = upsert_input(Some(alice), "theme", "light", 0.8, false, fact1, vec![]);

    let specific = upsert_input(
        Some(alice),
        "theme",
        "dark",
        0.7,
        false,
        fact2,
        vec![("time_of_day", "evening")],
    );

    kg.upsert_preference(default).await.unwrap();
    kg.upsert_preference(specific).await.unwrap();

    let result = kg
        .get_preference(
            Some(alice),
            "theme",
            &[("time_of_day".to_string(), "evening".to_string())],
        )
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().value, "dark");
}

#[tokio::test]
async fn contextual_lookup_most_specific_wins() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact1 = create_has_preference_fact(&kg, alice).await;
    let fact2 = create_has_preference_fact(&kg, alice).await;

    let one_ctx = upsert_input(
        Some(alice),
        "theme",
        "blue",
        0.8,
        false,
        fact1,
        vec![("time_of_day", "evening")],
    );

    let two_ctx = upsert_input(
        Some(alice),
        "theme",
        "red",
        0.8,
        false,
        fact2,
        vec![("time_of_day", "evening"), ("mood", "relaxed")],
    );

    kg.upsert_preference(one_ctx).await.unwrap();
    kg.upsert_preference(two_ctx).await.unwrap();

    let result = kg
        .get_preference(
            Some(alice),
            "theme",
            &[
                ("time_of_day".to_string(), "evening".to_string()),
                ("mood".to_string(), "relaxed".to_string()),
            ],
        )
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().value, "red");
}

#[tokio::test]
async fn contextual_lookup_no_match_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;

    let result = kg.get_preference(Some(alice), "theme", &[]).await.unwrap();

    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// 6. Audit logging on overwrite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overwrite_writes_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact1 = create_has_preference_fact(&kg, alice).await;
    let fact2 = create_has_preference_fact(&kg, alice).await;

    let first = upsert_input(Some(alice), "theme", "light", 0.6, false, fact1, vec![]);

    let (pref, _) = kg.upsert_preference(first).await.unwrap();
    let first_id = pref.id;

    let second = upsert_input(Some(alice), "theme", "dark", 0.9, false, fact2, vec![]);

    kg.upsert_preference(second).await.unwrap();

    let audit = kg.get_preference_audit_log(first_id).await.unwrap();
    assert!(audit.len() >= 2);
    let overwrite_entry = audit
        .iter()
        .find(|a| a.change_type_id == 3)
        .expect("Expected confidence_change audit entry");
    assert_eq!(overwrite_entry.old_value.as_deref(), Some("light"));
    assert_eq!(overwrite_entry.new_value.as_deref(), Some("dark"));
}

// ---------------------------------------------------------------------------
// 7. Source tracking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_preference_sources_returns_all() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let fact = create_has_preference_fact(&kg, alice).await;

    let input = UpsertPreferenceInput {
        preference: NewPreference {
            entity_id: Some(alice),
            category: PreferenceCategory::General,
            key: "theme".to_string(),
            value: "dark".to_string(),
            confidence: 0.9,
            overridden_by_user: false,
            source_fact_id: fact,
        },
        changed_by: ChangedBy::User,
        contexts: vec![],
        sources: vec![
            (PreferenceSourceType::UserEdit, "source-a".to_string()),
            (PreferenceSourceType::Fact, "source-b".to_string()),
        ],
    };

    let pref = kg.insert_preference(input).await.unwrap();
    let sources = kg.get_preference_sources(pref.id).await.unwrap();

    assert_eq!(sources.len(), 2);
    let type_ids: Vec<i16> = sources.iter().map(|s| s.source_type_id).collect();
    assert!(type_ids.contains(&(PreferenceSourceType::UserEdit as i16)));
    assert!(type_ids.contains(&(PreferenceSourceType::Fact as i16)));
}

// ---------------------------------------------------------------------------
// 8. FK enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_source_fact_id_fails() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;

    let input = upsert_input(
        Some(alice),
        "theme",
        "dark",
        0.9,
        false,
        99999, // non-existent fact
        vec![],
    );

    let result = kg.insert_preference(input).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// 9. Preference without entity (global)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_preference_with_null_entity_id() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    // Need a fact with a subject - use a dummy entity
    let dummy = create_person(&kg, "Dummy").await;
    let fact = create_has_preference_fact(&kg, dummy).await;

    let input = upsert_input(None, "global_theme", "dark", 0.9, false, fact, vec![]);

    let pref = kg.insert_preference(input).await.unwrap();
    assert_eq!(pref.entity_id, None);

    let fetched = kg.get_preference_by_id(pref.id).await.unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().entity_id, None);
}
