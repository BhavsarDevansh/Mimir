use chrono::Utc;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::{
    AutoCompletePolicy, ConnectorType, EventType, RecurrenceType,
};
use mimir_knowledge::models::event::NewEvent;
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::queries::source::AddSourceRequest;
use mimir_knowledge::{KnowledgeGraph, forget};

async fn create_person(kg: &KnowledgeGraph, name: &str) -> i32 {
    let e = kg
        .create_entity(
            name,
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();
    e.id
}

async fn create_place(kg: &KnowledgeGraph, name: &str) -> i32 {
    let e = kg
        .create_entity(
            name,
            mimir_knowledge::models::entity::EntityType::Place,
            &[],
        )
        .await
        .unwrap();
    e.id
}

/// Insert a connector-provenanced fact (instance + raw reference) and return
/// its id. `instance_id` must be a registered connector row (the insert path
/// resolves the instance).
async fn connector_fact(
    kg: &KnowledgeGraph,
    subject_id: i32,
    object_id: i32,
    relationship_type: &str,
    instance_id: i32,
    raw_reference: &str,
) -> i32 {
    kg.insert_fact(NewFact {
        subject_id,
        relationship_type: relationship_type.to_string(),
        object_id: Some(object_id),
        object_literal: None,
        valid_from: None,
        valid_until: None,
        source_type: SourceType::Connector,
        connector_instance_id: Some(instance_id),
        connector_type: Some(ConnectorType::Calendar),
        raw_reference: Some(raw_reference.to_string()),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    })
    .await
    .unwrap()
    .id
}

/// Register a Calendar connector instance and return its row id.
async fn register_connector(kg: &KnowledgeGraph, slug: &str) -> i32 {
    kg.upsert_connector(UpsertConnectorInput {
        connector_type: ConnectorType::Calendar,
        slug: slug.to_string(),
        backend: "caldav".to_string(),
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
async fn bulk_forget_by_predicate() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;

    let mut ids = Vec::new();
    for _ in 0..5 {
        let f = kg
            .insert_fact(NewFact {
                subject_id: alice,
                relationship_type: "visited".to_string(),
                object_id: Some(london),
                object_literal: None,
                valid_from: None,
                valid_until: None,
                source_type: SourceType::UserEdit,
                connector_instance_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: Vec::new(),
                category_ids: Vec::new(),
            })
            .await
            .unwrap();
        ids.push(f.id);
    }

    // Insert a different predicate to ensure it stays.
    let _other = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    let filters = forget::ForgetFilters {
        predicate: Some("visited".to_string()),
        ..Default::default()
    };
    let opts = forget::ForgetOptions {
        yes: true,
        ..Default::default()
    };

    let result = kg
        .forget_facts(filters, opts, ChangedBy::User)
        .await
        .unwrap();
    assert_eq!(result.forgotten_count, 5);

    for id in ids {
        assert!(kg.get_fact(id).await.unwrap().is_none());
    }

    // Other predicate fact should remain.
    let remaining = kg
        .get_facts_by_relationship_type(
            kg.get_relationship_type_id("is_in").await.unwrap().unwrap(),
            10,
        )
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
}

#[tokio::test]
async fn bulk_safeguard_over_100() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    for _ in 0..150 {
        kg.insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "visited".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    }

    let filters = forget::ForgetFilters {
        predicate: Some("visited".to_string()),
        ..Default::default()
    };
    let opts = forget::ForgetOptions {
        yes: false,
        ..Default::default()
    };

    let err = kg
        .forget_facts(filters, opts, ChangedBy::User)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Refusing to forget"));
}

#[tokio::test]
async fn sensitive_safeguard() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;

    // allergy is seeded as sensitive by migration 029, but predicates auto-created by insert_fact are created after migrations run, so the manual UPDATE below is required.
    kg.insert_fact(NewFact {
        subject_id: alice,
        relationship_type: "allergy".to_string(),
        object_id: None,
        object_literal: Some("peanuts".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    })
    .await
    .unwrap();

    // Mark the auto-created predicate as sensitive.
    let allergy_pred_id = kg
        .get_relationship_type_id("allergy")
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE relationship_types SET sensitive = TRUE WHERE id = ?")
        .bind(allergy_pred_id)
        .execute(kg.pool())
        .await
        .unwrap();

    let filters = forget::ForgetFilters {
        predicate: Some("allergy".to_string()),
        ..Default::default()
    };
    let opts = forget::ForgetOptions {
        yes: true,
        confirm_sensitive: false,
        ..Default::default()
    };

    let err = kg
        .forget_facts(filters, opts, ChangedBy::User)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("sensitive"));
}

#[tokio::test]
async fn full_reset_wrong_phrase() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let filters = forget::ForgetFilters {
        all: true,
        ..Default::default()
    };
    let opts = forget::ForgetOptions {
        confirmation_phrase: Some("NOPE".to_string()),
        ..Default::default()
    };

    let err = kg
        .forget_facts(filters, opts, ChangedBy::User)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("DELETE EVERYTHING"));
}

#[tokio::test]
async fn restore_single_fact() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    kg.forget_fact(fact.id, ChangedBy::User).await.unwrap();
    assert!(kg.get_fact(fact.id).await.unwrap().is_none());

    let trash_items = kg.list_trash(50, 0).await.unwrap();
    assert_eq!(trash_items.len(), 1);

    let restored = kg
        .restore_fact(trash_items[0].trash_id, ChangedBy::User)
        .await
        .unwrap();
    assert!(kg.get_fact(restored.id).await.unwrap().is_some());
}

#[tokio::test]
async fn restore_all_facts() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let mut fact_ids = Vec::new();
    for _ in 0..3 {
        let f = kg
            .insert_fact(NewFact {
                subject_id: alice,
                relationship_type: "visited".to_string(),
                object_id: Some(london),
                object_literal: None,
                valid_from: None,
                valid_until: None,
                source_type: SourceType::UserEdit,
                connector_instance_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: Vec::new(),
                category_ids: Vec::new(),
            })
            .await
            .unwrap();
        fact_ids.push(f.id);
    }

    for id in &fact_ids {
        kg.forget_fact(*id, ChangedBy::User).await.unwrap();
    }

    let restored = kg.restore_all(ChangedBy::User).await.unwrap();
    assert_eq!(restored.len(), 3);
    for f in &restored {
        assert!(kg.get_fact(f.id).await.unwrap().is_some());
    }
}

#[tokio::test]
async fn empty_trash_hard_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    kg.forget_fact(fact.id, ChangedBy::User).await.unwrap();
    assert_eq!(kg.list_trash(50, 0).await.unwrap().len(), 1);

    kg.empty_trash().await.unwrap();
    assert_eq!(kg.list_trash(50, 0).await.unwrap().len(), 0);
}

#[tokio::test]
async fn expired_trash_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let fact = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    kg.forget_fact(fact.id, ChangedBy::User).await.unwrap();

    // Manually set expires_at to the past.
    sqlx::query("UPDATE trash SET expires_at = ? WHERE original_table = 'facts'")
        .bind(Utc::now() - chrono::Duration::days(1))
        .execute(kg.pool())
        .await
        .unwrap();

    let deleted = mimir_knowledge::queries::trash::hard_delete_expired_trash(kg.pool(), Utc::now())
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(kg.list_trash(50, 0).await.unwrap().len(), 0);
}

#[tokio::test]
async fn cascade_after_bulk_forget() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let parent = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    // Insert inferred child manually.
    let child = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "visited".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Inference,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: true,
            inference_depth: 1,
            confidence: None,
            parent_fact_ids: vec![parent.id],
            category_ids: Vec::new(),
        })
        .await
        .unwrap();

    let filters = forget::ForgetFilters {
        fact_id: Some(parent.id),
        ..Default::default()
    };
    let opts = forget::ForgetOptions {
        yes: true,
        ..Default::default()
    };

    kg.forget_facts(filters, opts, ChangedBy::User)
        .await
        .unwrap();

    assert!(kg.get_fact(parent.id).await.unwrap().is_none());
    assert!(kg.get_fact(child.id).await.unwrap().is_none());
}

/// Issue #247: trashing facts by `(connector_instance_id, raw_reference)`
/// removes exactly the matching facts — facts from the same instance with a
/// different raw reference, and facts from another instance with the same raw
/// reference, must survive.
#[tokio::test]
async fn connector_raw_reference_forget_trashes_only_matching_facts() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;
    let instance_1 = register_connector(&kg, "calendar-1").await;
    let instance_2 = register_connector(&kg, "calendar-2").await;

    let target = connector_fact(&kg, alice, london, "visited", instance_1, "raw-1").await;
    let keep_same_instance = connector_fact(&kg, alice, paris, "is_in", instance_1, "raw-2").await;
    let keep_other_instance =
        connector_fact(&kg, alice, london, "lives_in", instance_2, "raw-1").await;

    let result = kg
        .forget_connector_facts_by_raw_reference(
            instance_1,
            &["raw-1".to_string()],
            ChangedBy::System,
        )
        .await
        .unwrap();
    assert_eq!(result.forgotten_count, 1);

    assert!(
        kg.get_fact(target).await.unwrap().is_none(),
        "matching (instance, raw_reference) fact must be trashed"
    );
    assert!(kg.get_fact(keep_same_instance).await.unwrap().is_some());
    assert!(kg.get_fact(keep_other_instance).await.unwrap().is_some());

    let trash = kg.list_trash(10, 0).await.unwrap();
    assert!(
        trash.iter().any(|t| t.fact_id == target),
        "trashed fact is recoverable from trash"
    );
}

/// Issue #247: a raw reference reported twice must not error (idempotent,
/// mirroring the `delete_event` 404-is-success semantics), and trashing a fact
/// removes its events-subsystem overlay (the `events.fact_id` FK cascades), so
/// a phantom event cannot keep surfacing.
#[tokio::test]
async fn connector_raw_reference_forget_is_idempotent_and_dismisses_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let event = kg
        .create_entity(
            "Trip to Rome",
            mimir_knowledge::models::entity::EntityType::Event,
            &[],
        )
        .await
        .unwrap()
        .id;
    let instance = register_connector(&kg, "calendar-1").await;
    let fact_id = connector_fact(&kg, alice, event, "has_event", instance, "raw-1").await;

    let trigger = Utc::now() + chrono::Duration::days(5);
    kg.insert_event(NewEvent {
        fact_id,
        entity_id: alice,
        trigger_date: trigger,
        recurrence: RecurrenceType::None,
        event_type: EventType::Appointment,
        auto_complete_policy: AutoCompletePolicy::AutoCompleteOnDate,
        requires_user_action: false,
    })
    .await
    .unwrap();

    let first = kg
        .forget_connector_facts_by_raw_reference(
            instance,
            &["raw-1".to_string()],
            ChangedBy::System,
        )
        .await
        .unwrap();
    assert_eq!(first.forgotten_count, 1);

    assert!(
        kg.get_event_by_fact(fact_id).await.unwrap().is_none(),
        "trashing a fact must cascade-remove its event overlay"
    );

    let second = kg
        .forget_connector_facts_by_raw_reference(
            instance,
            &["raw-1".to_string()],
            ChangedBy::System,
        )
        .await
        .unwrap();
    assert_eq!(
        second.forgotten_count, 0,
        "a re-reported tombstone is a no-op"
    );
}

/// PR #313 review: a tombstone must only remove the matching `sources` rows.
/// A fact still corroborated by another connector instance or by a
/// non-connector source is preserved; only a fact with no remaining sources
/// is trashed.
#[tokio::test]
async fn connector_raw_reference_forget_preserves_facts_with_other_sources() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;
    let paris = create_place(&kg, "Paris").await;
    let instance_1 = register_connector(&kg, "calendar-1").await;
    let instance_2 = register_connector(&kg, "calendar-2").await;

    // Solely sourced by instance_1/raw-1: trashed by the tombstone.
    let sole = connector_fact(&kg, alice, london, "visited", instance_1, "raw-1").await;

    // Corroborated by another connector instance: the tombstoned source row
    // is removed but the fact survives on the remaining source.
    let corroborated = connector_fact(&kg, alice, paris, "is_in", instance_1, "raw-2").await;
    kg.add_source_to_fact(AddSourceRequest {
        fact_id: corroborated,
        source_type: SourceType::Connector,
        connector_instance_id: Some(instance_2),
        connector_type: Some(ConnectorType::Calendar),
        raw_reference: Some("raw-other".to_string()),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        changed_by: ChangedBy::System,
    })
    .await
    .unwrap();

    // Corroborated by a non-connector (user) source: preserved too.
    let user_corroborated =
        connector_fact(&kg, alice, london, "lives_in", instance_1, "raw-3").await;
    kg.add_source_to_fact(AddSourceRequest {
        fact_id: user_corroborated,
        source_type: SourceType::UserEdit,
        connector_instance_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: Some(ExtractionMethod::UserInput),
        changed_by: ChangedBy::System,
    })
    .await
    .unwrap();

    let result = kg
        .forget_connector_facts_by_raw_reference(
            instance_1,
            &[
                "raw-1".to_string(),
                "raw-2".to_string(),
                "raw-3".to_string(),
            ],
            ChangedBy::System,
        )
        .await
        .unwrap();
    assert_eq!(
        result.forgotten_count, 1,
        "only the sole-sourced fact is trashed"
    );

    assert!(
        kg.get_fact(sole).await.unwrap().is_none(),
        "the sole-sourced fact is trashed"
    );
    assert!(
        kg.get_fact(corroborated).await.unwrap().is_some(),
        "a fact corroborated by another connector instance survives"
    );
    assert!(
        kg.get_fact(user_corroborated).await.unwrap().is_some(),
        "a fact corroborated by a non-connector source survives"
    );

    // Only the tombstoned source rows are gone; corroborating rows remain.
    let corroborated_sources = kg.get_sources_for_fact(corroborated).await.unwrap();
    assert_eq!(corroborated_sources.len(), 1);
    assert_eq!(
        corroborated_sources[0].connector_instance_id,
        Some(instance_2)
    );
    assert_eq!(
        corroborated_sources[0].raw_reference.as_deref(),
        Some("raw-other")
    );

    let user_sources = kg.get_sources_for_fact(user_corroborated).await.unwrap();
    assert_eq!(user_sources.len(), 1);
    assert_eq!(user_sources[0].source_type_id, SourceType::UserEdit as i16);
}
