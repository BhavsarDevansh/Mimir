//! Corroboration cascades and stale-confidence follow-ups (PR #174).

mod common;
use chrono::{TimeZone, Utc};
use common::{create_person, create_place};
use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::connector::UpsertConnectorInput;
use mimir_knowledge::models::enums::ConnectorType;
use mimir_knowledge::models::fact::{FactStatus, NewFact};
use mimir_knowledge::models::source::ExtractionMethod;
use mimir_knowledge::models::source::SourceType;
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

#[allow(dead_code)]
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

// ---------------------------------------------------------------------------
// PR #174 review follow-ups: stale_confidence and diamond-graph cascade.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn corroboration_at_cap_clears_stale_confidence_without_audit() {
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
    assert!((f1.confidence - 0.80).abs() < 1e-6);

    // Boost to the non-explicit cap: 0.80 -> 0.85 -> 0.90 -> 0.95.
    for (cid, rid) in [("s2", "r2"), ("s3", "r3"), ("s4", "r4")] {
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
    let at_cap = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((at_cap.confidence - 0.95).abs() < 1e-6);

    // Mark the capped fact stale, as the nightly optimiser would.
    sqlx::query("UPDATE facts SET stale_confidence = TRUE WHERE id = ?")
        .bind(f1.id)
        .execute(kg.pool())
        .await
        .unwrap();

    // A further independent corroboration cannot boost confidence (already at
    // the cap), but it adds provenance and must still clear the stale flag.
    kg.insert_fact(
        connector_new_fact(
            &kg,
            alice,
            "is_in",
            Some(london),
            None,
            Some(dt(2022, 1, 1)),
            None,
            "s5",
            "r5",
        )
        .await,
    )
    .await
    .unwrap();

    let after = kg.get_fact(f1.id).await.unwrap().unwrap();
    assert!((after.confidence - 0.95).abs() < 1e-6);
    assert!(
        !after.stale_confidence,
        "corroboration at the cap must clear stale_confidence"
    );
    // Four corroborating sources added (s2..s5); the original source is not
    // audited as a SourceAdded event.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::SourceAdded as i16).await,
        4
    );
    // Only the three actual boosts wrote a ConfidenceChange; the capped
    // corroboration must not record a no-op confidence change.
    assert_eq!(
        audit_count(&kg, f1.id, ChangeType::ConfidenceChange as i16).await,
        3
    );
}

#[tokio::test]
async fn corroboration_cascade_recalculates_through_diamond_graph() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();
    let alice = create_person(&kg, "Alice").await;
    let london = create_place(&kg, "London").await;

    // Root connector fact A at the connector default 0.80.
    let a = kg
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
    assert!((a.confidence - 0.80).abs() < 1e-6);

    // Inferred children B and C of A (single positive parent, depth 1):
    // 0.80 * 0.8^1 * breadth(1)=0.6 = 0.384.
    let insert_inferred = |subject: i32, rel: i16, obj: i32, depth: i32, conf: f32| {
        let pool = kg.pool().clone();
        async move {
            let fact: mimir_knowledge::models::fact::Fact = sqlx::query_as(
                "INSERT INTO facts \
                 (subject_id, relationship_type_id, object_id, confidence, fact_status_id, \
                  inferred, inference_depth, stale_confidence, pending_confirmation) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 RETURNING id, subject_id, relationship_type_id, object_id, object_literal, \
                 valid_from, valid_until, confidence, fact_status_id, inferred, \
                 inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at",
            )
            .bind(subject)
            .bind(rel)
            .bind(obj)
            .bind(conf)
            .bind(FactStatus::Inferred as i16)
            .bind(true)
            .bind(depth)
            .bind(false)
            .bind(false)
            .fetch_one(&pool)
            .await
            .unwrap();
            fact
        }
    };

    let b = insert_inferred(alice, a.relationship_type_id, london, 1, 0.384).await;
    let c = insert_inferred(alice, a.relationship_type_id, london, 1, 0.384).await;

    // Inferred child D of both B and C (two positive parents, depth 2):
    // (0.384 + 0.384) * 0.8^2 * breadth(2)=0.75 = 0.36864.
    let d = insert_inferred(alice, a.relationship_type_id, london, 2, 0.36864).await;

    for parent_id in [b.id, c.id] {
        sqlx::query(
            "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
             VALUES (?, ?, ?)",
        )
        .bind(parent_id)
        .bind(d.id)
        .bind(1i16) // InferredFrom
        .execute(kg.pool())
        .await
        .unwrap();
    }
    for parent_id in [a.id] {
        sqlx::query(
            "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
             VALUES (?, ?, ?)",
        )
        .bind(parent_id)
        .bind(b.id)
        .bind(1i16)
        .execute(kg.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fact_dependencies (parent_fact_id, child_fact_id, relation_type_id) \
             VALUES (?, ?, ?)",
        )
        .bind(parent_id)
        .bind(c.id)
        .bind(1i16)
        .execute(kg.pool())
        .await
        .unwrap();
    }

    // Corroborate A: 0.80 -> 0.85. The cascade must recalculate B and C to
    // 0.408 and then D from BOTH updated parents:
    // (0.408 + 0.408) * 0.64 * 0.75 = 0.39168.
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

    let b_after = kg.get_fact(b.id).await.unwrap().unwrap();
    let c_after = kg.get_fact(c.id).await.unwrap().unwrap();
    let d_after = kg.get_fact(d.id).await.unwrap().unwrap();
    assert!(
        (b_after.confidence - 0.408).abs() < 1e-6,
        "B: {}",
        b_after.confidence
    );
    assert!(
        (c_after.confidence - 0.408).abs() < 1e-6,
        "C: {}",
        c_after.confidence
    );
    assert!(
        (d_after.confidence - 0.39168).abs() < 1e-6,
        "D should reflect both updated parents, got {}",
        d_after.confidence
    );
    assert!(!d_after.stale_confidence);
}
