use chrono::Utc;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::SourceType;
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
                predicate: "visited".to_string(),
                object_id: Some(london),
                object_literal: None,
                valid_from: None,
                valid_until: None,
                source_type: SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: Vec::new(),
            })
            .await
            .unwrap();
        ids.push(f.id);
    }

    // Insert a different predicate to ensure it stays.
    let _other = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: "is_in".to_string(),
            object_id: Some(paris),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
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
        .get_facts_by_predicate(kg.get_predicate_id("is_in").await.unwrap().unwrap(), 10)
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
            predicate: "visited".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
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
        predicate: "allergy".to_string(),
        object_id: None,
        object_literal: Some("peanuts".to_string()),
        valid_from: None,
        valid_until: None,
        source_type: SourceType::UserEdit,
        connector_id: None,
        connector_type: None,
        raw_reference: None,
        extraction_method: None,
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
    })
    .await
    .unwrap();

    // Mark the auto-created predicate as sensitive.
    let allergy_pred_id = kg.get_predicate_id("allergy").await.unwrap().unwrap();
    sqlx::query("UPDATE predicates SET sensitive = TRUE WHERE id = ?")
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
            predicate: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
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
                predicate: "visited".to_string(),
                object_id: Some(london),
                object_literal: None,
                valid_from: None,
                valid_until: None,
                source_type: SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: Vec::new(),
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
            predicate: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
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
            predicate: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
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
            predicate: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::UserEdit,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: false,
            inference_depth: 0,
            confidence: None,
            parent_fact_ids: Vec::new(),
        })
        .await
        .unwrap();

    // Insert inferred child manually.
    let child = kg
        .insert_fact(NewFact {
            subject_id: alice,
            predicate: "visited".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: None,
            valid_until: None,
            source_type: SourceType::Inference,
            connector_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: None,
            inferred: true,
            inference_depth: 1,
            confidence: None,
            parent_fact_ids: vec![parent.id],
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
