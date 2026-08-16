//! Inference engine integration tests (Issue #54).

use mimir_knowledge::inference::InferenceRule;
mod common;

use mimir_knowledge::models::fact::FactStatus;
use mimir_knowledge::models::preference::PreferenceCategory;
use mimir_knowledge::models::source::SourceType;

// ---------------------------------------------------------------------------
// Transitivity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn transitivity_visited_plus_is_in() {
    let tg = common::TestGraph::new().await;
    let devansh = tg.create_person("Devansh").await;
    let rome = tg.create_place("Rome").await;
    let italy = tg.create_place("Italy").await;

    // Seed: Rome is_in Italy
    let is_in_rome = tg
        .create_fact(rome, "is_in", Some(italy), SourceType::UserEdit)
        .await;

    // Insert: Devansh visited Rome
    let visited = tg
        .create_fact(devansh, "visited", Some(rome), SourceType::UserEdit)
        .await;

    // Verify inferred: Devansh visited Italy
    let facts = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let inferred: Vec<_> = facts.iter().filter(|f| f.inferred).collect();
    assert_eq!(inferred.len(), 1);
    assert_eq!(inferred[0].subject_id, devansh);
    assert_eq!(inferred[0].object_id, Some(italy));
    assert_eq!(inferred[0].inference_depth, 1);
    assert!(inferred[0].confidence < 1.0);
    assert!(inferred[0].confidence > 0.0);

    // Verify InferredFrom dependency edges
    let deps: Vec<(i32, i32, i16, bool)> = sqlx::query_as(
        "SELECT parent_fact_id, child_fact_id, relation_type_id, is_positive \
         FROM fact_dependencies WHERE child_fact_id = ? AND relation_type_id = ?",
    )
    .bind(inferred[0].id)
    .bind(mimir_knowledge::models::enums::RelationType::InferredFrom as i16)
    .fetch_all(tg.kg.pool())
    .await
    .unwrap();
    assert_eq!(deps.len(), 2);
    let parent_ids: Vec<i32> = deps.iter().map(|d| d.0).collect();
    assert!(parent_ids.contains(&visited.id));
    assert!(parent_ids.contains(&is_in_rome.id));
}

#[tokio::test]
async fn transitivity_depth_increases_correctly() {
    let tg = common::TestGraph::new().await;
    let a = tg.create_place("A").await;
    let b = tg.create_place("B").await;
    let c = tg.create_place("C").await;
    let d = tg.create_place("D").await;

    // Chain: A is_in B, B is_in C, C is_in D
    tg.create_fact(a, "is_in", Some(b), SourceType::UserEdit)
        .await;
    tg.create_fact(b, "is_in", Some(c), SourceType::UserEdit)
        .await;
    tg.create_fact(c, "is_in", Some(d), SourceType::UserEdit)
        .await;

    // Insert: A visited B
    tg.create_fact(a, "visited", Some(b), SourceType::UserEdit)
        .await;

    // Should infer A visited C (depth 1) and A visited D (depth 2)
    let facts = tg.kg.get_facts_by_subject(a, 10).await.unwrap();
    let inferred: Vec<_> = facts.iter().filter(|f| f.inferred).collect();
    assert_eq!(inferred.len(), 2);

    let depths: Vec<i32> = inferred.iter().map(|f| f.inference_depth).collect();
    assert!(depths.contains(&1));
    assert!(depths.contains(&2));
}

// ---------------------------------------------------------------------------
// Contradiction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contradiction_overlapping_facts_both_disputed() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;
    let paris = tg.create_place("Paris").await;

    let from = chrono::Utc::now();
    let until = from + chrono::Duration::days(7);

    // First fact: Alice is_in London (explicit)
    let f1 = tg
        .create_fact_with_temporal(
            alice,
            "is_in",
            Some(london),
            Some(from),
            Some(until),
            SourceType::UserEdit,
        )
        .await;

    // Second fact: Alice is_in Paris (connector, overlapping)
    let f2 = tg
        .create_fact_with_temporal(
            alice,
            "is_in",
            Some(paris),
            Some(from),
            Some(until),
            SourceType::Connector,
        )
        .await;

    // Both should be Disputed
    let updated1 = tg.kg.get_fact(f1.id).await.unwrap().unwrap();
    let updated2 = tg.kg.get_fact(f2.id).await.unwrap().unwrap();
    assert_eq!(updated1.status(), Some(FactStatus::Disputed));
    assert_eq!(updated2.status(), Some(FactStatus::Disputed));

    // Verify Contradicts edges in both directions
    let deps: Vec<(i32, i32, i16)> = sqlx::query_as(
        "SELECT parent_fact_id, child_fact_id, relation_type_id \
         FROM fact_dependencies \
         WHERE (parent_fact_id = ? AND child_fact_id = ?) \
            OR (parent_fact_id = ? AND child_fact_id = ?)",
    )
    .bind(f1.id)
    .bind(f2.id)
    .bind(f2.id)
    .bind(f1.id)
    .fetch_all(tg.kg.pool())
    .await
    .unwrap();
    assert_eq!(deps.len(), 2);
    for (_, _, relation_type_id) in &deps {
        assert_eq!(
            *relation_type_id,
            mimir_knowledge::models::enums::RelationType::Contradicts as i16
        );
    }
}

#[tokio::test]
async fn non_contradiction_sequential_facts_remain_active() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;
    let paris = tg.create_place("Paris").await;

    let t1 = chrono::Utc::now();
    let t2 = t1 + chrono::Duration::days(7);
    let t3 = t2 + chrono::Duration::days(7);

    // Sequential non-overlapping facts
    let f1 = tg
        .create_fact_with_temporal(
            alice,
            "is_in",
            Some(london),
            Some(t1),
            Some(t2),
            SourceType::Connector,
        )
        .await;
    let f2 = tg
        .create_fact_with_temporal(
            alice,
            "is_in",
            Some(paris),
            Some(t2),
            Some(t3),
            SourceType::Connector,
        )
        .await;

    assert_eq!(f1.status(), Some(FactStatus::Active));
    assert_eq!(f2.status(), Some(FactStatus::Active));
}

// ---------------------------------------------------------------------------
// Threshold
// ---------------------------------------------------------------------------

#[tokio::test]
async fn threshold_three_rejections_creates_preference() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let hiking = tg.create_activity("hiking").await;

    let t1 = chrono::Utc::now();
    let t2 = t1 + chrono::Duration::days(1);
    let t3 = t2 + chrono::Duration::days(1);
    // Insert 3 rejected_action facts with non-overlapping ranges
    tg.create_fact_with_temporal(
        alice,
        "rejected_action",
        Some(hiking),
        Some(t1),
        Some(t2),
        SourceType::UserEdit,
    )
    .await;
    tg.create_fact_with_temporal(
        alice,
        "rejected_action",
        Some(hiking),
        Some(t2),
        Some(t3),
        SourceType::UserEdit,
    )
    .await;
    tg.create_fact_with_temporal(
        alice,
        "rejected_action",
        Some(hiking),
        Some(t3),
        None,
        SourceType::UserEdit,
    )
    .await;

    // Preference should be created
    let pref = tg
        .kg
        .get_preference(Some(alice), "reject_hiking", &[])
        .await
        .unwrap();
    assert!(pref.is_some());
    let pref = pref.unwrap();
    assert_eq!(pref.value, "true");
    assert_eq!(pref.category_id, PreferenceCategory::General as i16);
    assert!((pref.confidence - 0.70).abs() < 0.001);
}

#[tokio::test]
async fn threshold_two_rejections_no_preference() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let hiking = tg.create_activity("hiking").await;

    let t1 = chrono::Utc::now();
    let t2 = t1 + chrono::Duration::days(1);
    // Insert only 2 rejected_action facts with non-overlapping ranges
    tg.create_fact_with_temporal(
        alice,
        "rejected_action",
        Some(hiking),
        Some(t1),
        Some(t2),
        SourceType::UserEdit,
    )
    .await;
    tg.create_fact_with_temporal(
        alice,
        "rejected_action",
        Some(hiking),
        Some(t2),
        None,
        SourceType::UserEdit,
    )
    .await;

    // No preference should be created
    let pref = tg
        .kg
        .get_preference(Some(alice), "reject_hiking", &[])
        .await
        .unwrap();
    assert!(pref.is_none());
}

// ---------------------------------------------------------------------------
// Cascade
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cascade_insert_visited_then_is_in() {
    let tg = common::TestGraph::new().await;
    let devansh = tg.create_person("Devansh").await;
    let rome = tg.create_place("Rome").await;
    let italy = tg.create_place("Italy").await;

    // Insert visited first
    tg.create_fact(devansh, "visited", Some(rome), SourceType::UserEdit)
        .await;

    // No inference yet because Rome is_in Italy doesn't exist
    let facts_before = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let inferred_before: Vec<_> = facts_before.iter().filter(|f| f.inferred).collect();
    assert_eq!(inferred_before.len(), 0);

    // Now insert Rome is_in Italy
    tg.create_fact(rome, "is_in", Some(italy), SourceType::UserEdit)
        .await;

    // Inference should trigger on the is_in insertion
    let facts_after = tg.kg.get_facts_by_subject(devansh, 10).await.unwrap();
    let inferred_after: Vec<_> = facts_after.iter().filter(|f| f.inferred).collect();
    assert_eq!(inferred_after.len(), 1);
    assert_eq!(inferred_after[0].object_id, Some(italy));
}

// ---------------------------------------------------------------------------
// Cycle safety
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cycle_safety_cyclic_is_in_no_infinite_loop() {
    let tg = common::TestGraph::new().await;
    let a = tg.create_place("A").await;
    let b = tg.create_place("B").await;
    let c = tg.create_place("C").await;

    // Create a cycle: A is_in B, B is_in C, C is_in A
    tg.create_fact(a, "is_in", Some(b), SourceType::UserEdit)
        .await;
    tg.create_fact(b, "is_in", Some(c), SourceType::UserEdit)
        .await;
    tg.create_fact(c, "is_in", Some(a), SourceType::UserEdit)
        .await;

    // Insert A visited B - should not infinite loop
    tg.create_fact(a, "visited", Some(b), SourceType::UserEdit)
        .await;

    // Should terminate and have some inferred facts (but not infinite)
    let all_facts: Vec<mimir_knowledge::models::fact::Fact> = sqlx::query_as(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts ORDER BY id",
    )
    .fetch_all(tg.kg.pool())
    .await
    .unwrap();
    for f in &all_facts {
        eprintln!(
            "ALLFACT: id={} subject={} pred_id={} object={} inferred={} depth={} status={:?}",
            f.id,
            f.subject_id,
            f.relationship_type_id,
            f.object_id.unwrap_or(-1),
            f.inferred,
            f.inference_depth,
            f.status()
        );
    }
    let facts = tg.kg.get_facts_by_subject(a, 100).await.unwrap();
    let inferred_facts: Vec<_> = facts.iter().filter(|f| f.inferred).collect();
    let inferred_count = inferred_facts.len();
    // With cycle A->B->C->A, visiting A should infer A visited C (via B is_in C).
    // A visited C would trigger rule to find C is_in A, but C is_in A was marked
    // Disputed when the inferred C is_in B overlapped with it. Since the rule only
    // consults Active is_in facts, A visited A is not inferred.
    // The cascade terminates safely (no infinite loop).
    assert!(
        inferred_count <= 1,
        "expected at most 1 inferred fact, got {}",
        inferred_count
    );
    let unique_inferred: std::collections::HashSet<_> =
        inferred_facts.iter().map(|f| f.id).collect();
    assert_eq!(
        inferred_count,
        unique_inferred.len(),
        "inferred facts must be unique"
    );
}

// ---------------------------------------------------------------------------
// Nightly batch deduplication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nightly_batch_does_not_duplicate_inferred_facts() {
    let tg = common::TestGraph::new().await;
    let devansh = tg.create_person("Devansh").await;
    let rome = tg.create_place("Rome").await;
    let italy = tg.create_place("Italy").await;

    // Seed facts
    tg.create_fact(rome, "is_in", Some(italy), SourceType::UserEdit)
        .await;
    tg.create_fact(devansh, "visited", Some(rome), SourceType::UserEdit)
        .await;

    // Count inferred facts after initial cascade
    let facts_before = tg.kg.get_facts_by_subject(devansh, 100).await.unwrap();
    let inferred_before: Vec<_> = facts_before.iter().filter(|f| f.inferred).collect();
    let count_before = inferred_before.len();

    // Run nightly optimization once
    mimir_knowledge::optimization::run_nightly_optimization(&tg.kg, &tg.backup_dir())
        .await
        .unwrap();

    let facts_after_first = tg.kg.get_facts_by_subject(devansh, 100).await.unwrap();
    let inferred_after_first: Vec<_> = facts_after_first.iter().filter(|f| f.inferred).collect();
    let count_after_first = inferred_after_first.len();

    // Should still be the same number of inferred facts
    assert_eq!(
        count_after_first, count_before,
        "nightly run should not create new inferred facts"
    );

    // Run nightly optimization again
    mimir_knowledge::optimization::run_nightly_optimization(&tg.kg, &tg.backup_dir())
        .await
        .unwrap();

    let facts_after_second = tg.kg.get_facts_by_subject(devansh, 100).await.unwrap();
    let inferred_after_second: Vec<_> = facts_after_second.iter().filter(|f| f.inferred).collect();
    let count_after_second = inferred_after_second.len();

    // Should still not have duplicated anything
    assert_eq!(
        count_after_second, count_before,
        "second nightly run should not duplicate inferred facts"
    );
}

#[tokio::test]
async fn threshold_evaluate_returns_no_facts() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let hiking = tg.create_activity("hiking").await;

    let fact = tg
        .create_fact(alice, "rejected_action", Some(hiking), SourceType::UserEdit)
        .await;

    // ThresholdRule::evaluate should be a pure function with no side effects now.
    let rule = mimir_knowledge::inference::rules::threshold::ThresholdRule;
    let result = rule.evaluate(&fact, &tg.kg).await.unwrap();
    assert!(result.is_empty());

    // No preference should be created from evaluate alone.
    let pref = tg
        .kg
        .get_preference(Some(alice), "reject_hiking", &[])
        .await
        .unwrap();
    assert!(pref.is_none());
}
