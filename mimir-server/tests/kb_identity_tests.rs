mod common;
use common::*;

#[tokio::test]
async fn test_seed_identity_facts_creates_name_and_preferred_name() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

    // Create a user entity manually since test_state does not resolve identity.
    let entity = state
        .knowledge_graph
        .create_entity(
            "Alice Smith",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    // Seed identity facts
    mimir_server::state::seed_identity_facts(
        &state.knowledge_graph,
        entity.id,
        "Alice Smith",
        "Alice",
    )
    .await
    .unwrap();

    // Verify facts exist
    let facts = state
        .knowledge_graph
        .get_facts_by_subject(entity.id, 1000)
        .await
        .unwrap();

    let mut found_name = false;
    let mut found_preferred = false;
    for fact in &facts {
        let pred = state
            .knowledge_graph
            .relationship_type_name(fact.relationship_type_id)
            .await;
        if pred.as_deref() == Some("has_name")
            && fact.object_literal.as_deref() == Some("Alice Smith")
        {
            found_name = true;
        }
        if pred.as_deref() == Some("preferred_name")
            && fact.object_literal.as_deref() == Some("Alice")
        {
            found_preferred = true;
        }
    }
    assert!(found_name, "expected has_name fact for Alice Smith");
    assert!(found_preferred, "expected preferred_name fact for Alice");
}
#[tokio::test]
async fn test_seed_identity_facts_is_idempotent() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

    let entity = state
        .knowledge_graph
        .create_entity(
            "Bob",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    // Call twice with same values
    mimir_server::state::seed_identity_facts(&state.knowledge_graph, entity.id, "Bob", "Bobby")
        .await
        .unwrap();
    mimir_server::state::seed_identity_facts(&state.knowledge_graph, entity.id, "Bob", "Bobby")
        .await
        .unwrap();

    let facts = state
        .knowledge_graph
        .get_facts_by_subject(entity.id, 1000)
        .await
        .unwrap();

    let mut name_count = 0;
    let mut pref_count = 0;
    for f in &facts {
        let pred = state
            .knowledge_graph
            .relationship_type_name(f.relationship_type_id)
            .await;
        if f.status() == Some(mimir_knowledge::models::fact::FactStatus::Active) {
            if pred.as_deref() == Some("has_name") && f.object_literal.as_deref() == Some("Bob") {
                name_count += 1;
            }
            if pred.as_deref() == Some("preferred_name")
                && f.object_literal.as_deref() == Some("Bobby")
            {
                pref_count += 1;
            }
        }
    }

    assert_eq!(name_count, 1, "expected exactly one active has_name fact");
    assert_eq!(
        pref_count, 1,
        "expected exactly one active preferred_name fact"
    );
}
#[tokio::test]
async fn test_seed_identity_facts_adds_alias_and_merges_duplicate() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

    // Canonical entity
    let canonical = state
        .knowledge_graph
        .create_entity(
            "Devansh Bhavsar",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    // Bare-name duplicate (simulating old bug)
    let duplicate = state
        .knowledge_graph
        .create_entity(
            "Devansh",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    // Seed identity facts – should add alias and auto-merge duplicate
    mimir_server::state::seed_identity_facts(
        &state.knowledge_graph,
        canonical.id,
        "Devansh Bhavsar",
        "Devansh",
    )
    .await
    .unwrap();

    // Alias should now exist
    let resolved =
        mimir_knowledge::queries::entity::get_by_name(state.knowledge_graph.pool(), "Devansh")
            .await
            .unwrap();
    assert!(!resolved.is_empty());
    assert_eq!(resolved[0].entity.id, canonical.id);
    assert_eq!(
        resolved[0].match_kind,
        mimir_knowledge::queries::entity::MatchKind::ExactAlias
    );

    // Duplicate entity should have been merged away
    let gone = state
        .knowledge_graph
        .get_entity(duplicate.id)
        .await
        .unwrap();
    assert!(gone.is_none(), "expected duplicate entity to be merged");
}
#[tokio::test]
async fn test_seed_identity_facts_preserves_canonical_when_duplicate_has_more_facts() {
    let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

    // Canonical entity with no facts yet
    let canonical = state
        .knowledge_graph
        .create_entity(
            "Devansh Bhavsar",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    // Duplicate entity that already has a couple of facts
    let duplicate = state
        .knowledge_graph
        .create_entity(
            "Devansh",
            mimir_knowledge::models::entity::EntityType::Person,
            &[],
        )
        .await
        .unwrap();

    // Give the duplicate two facts so it outranks the canonical pre-fix
    use mimir_knowledge::models::fact::NewFact;
    use mimir_knowledge::models::source::SourceType;
    let mut f1 = NewFact::new(duplicate.id, "has_name");
    f1.object_literal = Some("Devansh".to_string());
    f1.source_type = SourceType::System;
    let mut f2 = NewFact::new(duplicate.id, "works_at");
    f2.object_literal = Some("Acme".to_string());
    f2.source_type = SourceType::System;
    state
        .knowledge_graph
        .insert_facts_batch(vec![f1, f2])
        .await
        .unwrap();

    // Seed identity facts – canonical should survive because its facts are
    // inserted *before* the auto-merge check.
    mimir_server::state::seed_identity_facts(
        &state.knowledge_graph,
        canonical.id,
        "Devansh Bhavsar",
        "Devansh",
    )
    .await
    .unwrap();

    // Canonical entity must still exist
    let canonical_still = state
        .knowledge_graph
        .get_entity(canonical.id)
        .await
        .unwrap();
    assert!(
        canonical_still.is_some(),
        "canonical entity must survive auto-merge"
    );

    // Duplicate entity should have been merged away
    let gone = state
        .knowledge_graph
        .get_entity(duplicate.id)
        .await
        .unwrap();
    assert!(gone.is_none(), "expected duplicate entity to be merged");
}
// ------------------------------------------------------------------
// Pending sensitive-fact confirmation lifecycle (issue #141)
