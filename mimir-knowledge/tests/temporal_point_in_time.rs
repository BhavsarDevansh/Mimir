//! Temporal point-in-time queries (issue #63).

use chrono::{DateTime, Utc};
use mimir_knowledge::models::fact::NewFact;
use mimir_knowledge::models::source::SourceType;

mod common;

#[tokio::test]
async fn temporal_fact_at_midpoint() {
    let tg = common::TestGraph::new().await;
    let alice = tg.create_person("Alice").await;
    let london = tg.create_place("London").await;
    let paris = tg.create_place("Paris").await;

    let is_in_id = tg.kg.ensure_relationship_type("is_in").await.unwrap();

    // Alice lives_in London from Jan 1 to Jun 1 2023
    let mut f1 = NewFact::new(alice, "is_in");
    f1.object_id = Some(london);
    f1.valid_from = Some(
        DateTime::parse_from_rfc3339("2023-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    );
    f1.valid_until = Some(
        DateTime::parse_from_rfc3339("2023-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    );
    f1.source_type = SourceType::UserEdit;
    tg.kg.insert_fact(f1).await.unwrap();

    // Alice lives_in Paris from Jun 1 onwards
    let mut f2 = NewFact::new(alice, "is_in");
    f2.object_id = Some(paris);
    f2.valid_from = Some(
        DateTime::parse_from_rfc3339("2023-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    );
    f2.valid_until = None;
    f2.source_type = SourceType::UserEdit;
    tg.kg.insert_fact(f2).await.unwrap();

    // Query at Mar 15 2023 → only London
    let mar15 = DateTime::parse_from_rfc3339("2023-03-15T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let facts =
        mimir_knowledge::queries::fact::get_active_facts_at(tg.kg.pool(), alice, is_in_id, mar15)
            .await
            .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].object_id, Some(london));

    // Query at Aug 1 2023 → only Paris
    let aug1 = DateTime::parse_from_rfc3339("2023-08-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let facts =
        mimir_knowledge::queries::fact::get_active_facts_at(tg.kg.pool(), alice, is_in_id, aug1)
            .await
            .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].object_id, Some(paris));

    // Query exactly at boundary Jun 1 → Paris (valid_until > at, valid_from <= at)
    let jun1 = DateTime::parse_from_rfc3339("2023-06-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let facts =
        mimir_knowledge::queries::fact::get_active_facts_at(tg.kg.pool(), alice, is_in_id, jun1)
            .await
            .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].object_id, Some(paris));
}

#[tokio::test]
async fn temporal_no_range_matches_open_end() {
    let tg = common::TestGraph::new().await;
    let bob = tg.create_person("Bob").await;
    let berlin = tg.create_place("Berlin").await;

    let is_in_id = tg.kg.ensure_relationship_type("is_in").await.unwrap();

    // Bob lives_in Berlin with no temporal bounds
    let mut f = NewFact::new(bob, "is_in");
    f.object_id = Some(berlin);
    f.source_type = SourceType::UserEdit;
    tg.kg.insert_fact(f).await.unwrap();

    let any_time = DateTime::parse_from_rfc3339("1999-12-31T23:59:59Z")
        .unwrap()
        .with_timezone(&Utc);
    let facts =
        mimir_knowledge::queries::fact::get_active_facts_at(tg.kg.pool(), bob, is_in_id, any_time)
            .await
            .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].object_id, Some(berlin));
}

#[tokio::test]
async fn temporal_open_ended_start() {
    let tg = common::TestGraph::new().await;
    let carol = tg.create_person("Carol").await;
    let nyc = tg.create_place("NYC").await;

    let is_in_id = tg.kg.ensure_relationship_type("is_in").await.unwrap();

    // Carol lives_in NYC with no valid_from but an open-ended valid_until
    let mut f = NewFact::new(carol, "is_in");
    f.object_id = Some(nyc);
    f.valid_from = None;
    f.valid_until = Some(
        DateTime::parse_from_rfc3339("2025-12-31T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc),
    );
    f.source_type = SourceType::UserEdit;
    tg.kg.insert_fact(f).await.unwrap();

    // Query at a random past time → should match because valid_from is NULL
    let query_time = DateTime::parse_from_rfc3339("1999-06-15T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let facts = mimir_knowledge::queries::fact::get_active_facts_at(
        tg.kg.pool(),
        carol,
        is_in_id,
        query_time,
    )
    .await
    .unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].object_id, Some(nyc));

    // Query after valid_until → should NOT match
    let after = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let facts =
        mimir_knowledge::queries::fact::get_active_facts_at(tg.kg.pool(), carol, is_in_id, after)
            .await
            .unwrap();
    assert!(facts.is_empty());
}
