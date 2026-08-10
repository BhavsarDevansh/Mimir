//! Corroboration detection (#79): source rows, confidence boosts, and supersession boundaries.

mod common;
use chrono::{TimeZone, Utc};
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::ConnectorType;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::models::source::SourceType;
// ---------------------------------------------------------------------------
// Corroboration detection (#79)
//
// When a new non-explicit fact covers the same claim as an existing fact
// (same subject + predicate + object, temporally overlapping), the existing
// fact receives a new source row and a +0.05 confidence boost (capped at
// 0.95 for non-explicit, non-inferred facts). No new facts row is created.
// ---------------------------------------------------------------------------

/// Build a connector-sourced `NewFact` with distinct provenance so two facts
/// are independent sources (required to clear the `sources` UNIQUE index).
#[allow(clippy::too_many_arguments)]
async fn connector_new_fact(
    kg: &KnowledgeGraph,
    subject_id: i32,
    predicate: &str,
    object_id: Option<i32>,
    object_literal: Option<String>,
    valid_from: Option<chrono::DateTime<Utc>>,
    valid_until: Option<chrono::DateTime<Utc>>,
    connector_id: &str,
    raw_reference: &str,
) -> NewFact {
    // Each distinct `connector_id` label is a distinct registered Photos
    // connector instance (default reliability 0.80), so two facts with
    // different labels are independent sources for corroboration.
    let instance_id = kg
        .upsert_connector(UpsertConnectorInput {
            connector_type: ConnectorType::Photos,
            slug: connector_id.to_string(),
            backend: "test".to_string(),
            display_name: connector_id.to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap()
        .id;
    NewFact {
        subject_id,
        relationship_type: predicate.to_string(),
        object_id,
        object_literal,
        valid_from,
        valid_until,
        source_type: SourceType::Connector,
        connector_instance_id: Some(instance_id),
        connector_type: Some(ConnectorType::Photos),
        raw_reference: Some(raw_reference.to_string()),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        inferred: false,
        inference_depth: 0,
        confidence: None,
        parent_fact_ids: Vec::new(),
        category_ids: Vec::new(),
    }
}

async fn source_count(kg: &KnowledgeGraph, fact_id: i32) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sources WHERE fact_id = ?")
        .bind(fact_id)
        .fetch_one(kg.pool())
        .await
        .unwrap();
    count
}

async fn audit_count(kg: &KnowledgeGraph, fact_id: i32, change_type_id: i16) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM fact_audit_log WHERE fact_id = ? AND change_type_id = ?",
    )
    .bind(fact_id)
    .bind(change_type_id)
    .fetch_one(kg.pool())
    .await
    .unwrap();
    count
}

fn dt(y: i32, m: u32, d: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
}

#[tokio::test]
async fn corroboration_adds_source_not_new_fact() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "gmail",
                "msg-1",
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(source_count(&kg, f1.id).await, 1);

    // Second independent connector source, same claim, temporally overlapping.
    let corroborated = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 6, 1)),
                None,
                "calendar",
                "event-1",
            )
            .await,
        )
        .await
        .unwrap();

    // No new facts row; the existing fact is returned.
    assert_eq!(corroborated.id, f1.id);
    assert_eq!(source_count(&kg, f1.id).await, 2);

    let (facts,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE subject_id = ? AND object_id = ?")
            .bind(alice)
            .bind(london)
            .fetch_one(kg.pool())
            .await
            .unwrap();
    assert_eq!(facts, 1);
}

#[tokio::test]
async fn corroboration_boosts_confidence_capped_at_ninety_five() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    // Connector default confidence is 0.80.
    assert!((f1.confidence - 0.80).abs() < 1e-6);

    for (cid, rid) in [("s2", "r2"), ("s3", "r3"), ("s4", "r4"), ("s5", "r5")] {
        kg.insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                cid,
                rid,
            )
            .await,
        )
        .await
        .unwrap();
    }

    let final_fact = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!(
        (final_fact.confidence - 0.95).abs() < 1e-6,
        "expected 0.95, got {}",
        final_fact.confidence
    );
}

#[tokio::test]
async fn non_overlapping_temporal_ranges_stay_separate() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                Some(dt(2022, 1, 1)),
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    // Disjoint range — separate fact, no corroboration.
    let f2 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2023, 1, 1)),
                Some(dt(2024, 1, 1)),
                "s2",
                "r2",
            )
            .await,
        )
        .await
        .unwrap();

    assert_ne!(f1.id, f2.id);
    assert_eq!(f1.status().unwrap(), FactStatus::Active);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
    assert_eq!(source_count(&kg, f1.id).await, 1);
    assert_eq!(source_count(&kg, f2.id).await, 1);

    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((f1_after.confidence - 0.80).abs() < 1e-6);
}

#[tokio::test]
async fn explicit_new_overlapping_connector_supersedes_not_corroborates() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(source_count(&kg, f1.id).await, 1);

    // Explicit user edit of the same claim, overlapping → supersedes.
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2021, 1, 1)),
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

    assert_ne!(f1.id, f2.id);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert_eq!(f1_after.status().unwrap(), FactStatus::Superseded);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
    // No source added to the superseded fact.
    assert_eq!(source_count(&kg, f1.id).await, 1);
    assert_eq!(source_count(&kg, f2.id).await, 1);
}

#[tokio::test]
async fn duplicate_source_is_noop() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    // Identical provenance — not an independent source, so a no-op.
    let result = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    assert_eq!(result.id, f1.id);
    assert_eq!(source_count(&kg, f1.id).await, 1);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((f1_after.confidence - 0.80).abs() < 1e-6);

    // The duplicate-provenance fast path is audit-silent: it must not emit a
    // SourceAdded or ConfidenceChange row for the unchanged fact.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::SourceAdded as i16).await,
        0
    );
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::ConfidenceChange as i16).await,
        0
    );
}

#[tokio::test]
async fn explicit_existing_corroborated_no_boost() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2020, 1, 1)),
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
    assert!((f1.confidence - 1.0).abs() < 1e-6);
    assert_eq!(source_count(&kg, f1.id).await, 1);

    let result = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    assert_eq!(result.id, f1.id);
    // Source added for provenance, but confidence unchanged (explicit is capped).
    assert_eq!(source_count(&kg, f1.id).await, 2);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((f1_after.confidence - 1.0).abs() < 1e-6);
}

#[tokio::test]
async fn inferred_fact_corroboration_adds_source_no_boost() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let bob = create_person(&kg, "Bob").await;
    let london = create_place(&kg, "London").await;

    // Parent fact the inference is notionally derived from.
    let _parent = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2020, 1, 1)),
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

    // Manually insert an inferred fact (confidence is structural, not boosted).
    let inferred = kg
        .insert_fact(NewFact {
            subject_id: bob,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2020, 1, 1)),
            valid_until: None,
            source_type: SourceType::Inference,
            connector_instance_id: None,
            connector_type: None,
            raw_reference: None,
            extraction_method: Some(ExtractionMethod::InferenceRule),
            inferred: true,
            inference_depth: 1,
            confidence: Some(0.60),
            parent_fact_ids: Vec::new(),
            category_ids: Vec::new(),
        })
        .await
        .unwrap();
    assert!(inferred.inferred);
    assert!((inferred.confidence - 0.60).abs() < 1e-6);
    assert_eq!(source_count(&kg, inferred.id).await, 1);

    let result = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                bob,
                "is_in",
                Some(london),
                None,
                Some(dt(2021, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    assert_eq!(result.id, inferred.id);
    assert_eq!(source_count(&kg, inferred.id).await, 2);
    let after = kg.get_fact(inferred.id).await.unwrap().unwrap();
    // Inferred confidence is structural — corroboration must not boost it.
    assert!((after.confidence - 0.60).abs() < 1e-6);
}

#[tokio::test]
async fn corroboration_writes_audit_and_clears_stale_confidence() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();

    // Mark the fact's confidence as stale, as the nightly optimiser would.
    sqlx::query("UPDATE facts SET stale_confidence = TRUE WHERE id = ?")
        .bind(f1.id)
        .execute(kg.pool())
        .await
        .unwrap();

    kg.insert_fact(
        connector_new_fact(
            &kg,
            alice,
            "is_in",
            Some(london),
            None,
            Some(dt(2021, 1, 1)),
            None,
            "s2",
            "r2",
        )
        .await,
    )
    .await
    .unwrap();

    // SourceAdded = 5, ConfidenceChange = 3.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::SourceAdded as i16).await,
        1
    );
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::ConfidenceChange as i16).await,
        1
    );

    let after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((after.confidence - 0.85).abs() < 1e-6);
    assert!(!after.stale_confidence);
}

#[tokio::test]
async fn corroboration_cascades_to_inferred_child() {
    use mimir_knowledge::models::audit_log::ChangeType;

    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    // Parent connector fact (connector default confidence 0.80).
    let parent = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert!((parent.confidence - 0.80).abs() < 1e-6);

    // Inferred child linked to the parent via an InferredFrom edge. Its
    // structural confidence for a single positive parent at depth 1 is
    // 0.80 * 0.8^1 * breadth(1)=0.6 = 0.384.
    let child: mimir_knowledge::models::fact::Fact = sqlx::query_as(
        "INSERT INTO facts \
         (subject_id, relationship_type_id, object_id, confidence, fact_status_id, \
          inferred, inference_depth, stale_confidence, pending_confirmation) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
    )
    .bind(alice)
    .bind(parent.relationship_type_id)
    .bind(london)
    .bind(0.384f32)
    .bind(FactStatus::Inferred as i16)
    .bind(true)
    .bind(1i32)
    .bind(false)
    .bind(false)
    .fetch_one(kg.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
         VALUES (?, ?, ?)",
    )
    .bind(parent.id)
    .bind(child.id)
    .bind(1i16) // InferredFrom
    .execute(kg.pool())
    .await
    .unwrap();

    // A corroborating connector fact boosts the parent 0.80 -> 0.85, which
    // must cascade and recalculate the child to 0.85 * 0.8 * 0.6 = 0.408.
    kg.insert_fact(
        connector_new_fact(
            &kg,
            alice,
            "is_in",
            Some(london),
            None,
            Some(dt(2021, 1, 1)),
            None,
            "s2",
            "r2",
        )
        .await,
    )
    .await
    .unwrap();

    let parent_after = kg.get_fact(parent.id).await.unwrap().unwrap();
    assert!((parent_after.confidence - 0.85).abs() < 1e-6);

    let child_after = kg.get_fact(child.id).await.unwrap().unwrap();
    assert!(
        (child_after.confidence - 0.408).abs() < 1e-6,
        "expected child confidence 0.408, got {}",
        child_after.confidence
    );
    assert!(!child_after.stale_confidence);

    // The cascade writes a ConfidenceChange audit entry for the child.
    assert_eq!(
        audit_count(&kg, child.id, ChangeType::ConfidenceChange as i16).await,
        1
    );
}

#[tokio::test]
async fn explicit_system_overlapping_supersedes_not_corroborates() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    let f1 = kg
        .insert_fact(
            connector_new_fact(
                &kg,
                alice,
                "is_in",
                Some(london),
                None,
                Some(dt(2020, 1, 1)),
                None,
                "s1",
                "r1",
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(source_count(&kg, f1.id).await, 1);

    // An overlapping System assertion is explicit, so it supersedes rather
    // than corroborating (mirrors the UserEdit path).
    let f2 = kg
        .insert_fact(NewFact {
            subject_id: alice,
            relationship_type: "is_in".to_string(),
            object_id: Some(london),
            object_literal: None,
            valid_from: Some(dt(2021, 1, 1)),
            valid_until: None,
            source_type: SourceType::System,
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

    assert_ne!(f1.id, f2.id);
    let f1_after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert_eq!(f1_after.status().unwrap(), FactStatus::Superseded);
    assert_eq!(f2.status().unwrap(), FactStatus::Active);
    // No source added to the superseded fact; the new fact owns one source.
    assert_eq!(source_count(&kg, f1.id).await, 1);
    assert_eq!(source_count(&kg, f2.id).await, 1);
}
