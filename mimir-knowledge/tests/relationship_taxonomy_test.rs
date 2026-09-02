//! Closed relationship-taxonomy contracts (#468).

mod common;
use common::TestGraph;

#[tokio::test]
async fn migration_adds_closed_taxonomy_tables() {
    let tg = TestGraph::new().await;

    let names: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(tg.kg.pool())
            .await
            .unwrap();

    let names: Vec<String> = names.into_iter().map(|(name,)| name).collect();
    for table in ["relationship_type_category_rules", "unrecognized_facts"] {
        assert!(
            names.contains(&table.to_string()),
            "migration must create {table}"
        );
    }
}

#[tokio::test]
async fn seeded_roots_are_query_only_and_leaves_are_emit_eligible() {
    let tg = TestGraph::new().await;

    for root in [
        "identity",
        "relationship",
        "preference",
        "employment",
        "education",
        "residence",
        "location",
        "ownership",
        "event",
        "travel",
        "commerce",
        "health",
        "credential",
        "communication",
        "document",
    ] {
        let row: Option<(bool,)> =
            sqlx::query_as("SELECT emit_eligible FROM relationship_types WHERE name = ?")
                .bind(root)
                .fetch_optional(tg.kg.pool())
                .await
                .unwrap();
        let (emit_eligible,) = row.unwrap_or_else(|| panic!("root {root} must be seeded"));
        assert!(!emit_eligible, "root {root} must be query-only");
    }
}

#[tokio::test]
async fn every_canonical_leaf_is_emittable_with_parent_and_category() {
    let tg = TestGraph::new().await;

    for name in mimir_knowledge::CANONICAL_PREDICATES {
        let row: Option<(bool, Option<i64>, i64)> = sqlx::query_as(
            "SELECT emit_eligible, parent_id, depth FROM relationship_types WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(tg.kg.pool())
        .await
        .unwrap();
        let (emit_eligible, parent_id, depth) =
            row.unwrap_or_else(|| panic!("leaf {name} must be seeded"));
        assert!(emit_eligible, "{name} must remain emit-eligible");
        assert!(parent_id.is_some(), "{name} must have a taxonomy parent");
        assert!(depth > 0, "{name} must not be a root");

        let category_rule: Option<(i64,)> = sqlx::query_as(
            "SELECT category_id FROM relationship_type_category_rules WHERE relationship_type_id = ?",
        )
        .bind(
            sqlx::query_scalar::<_, i64>("SELECT id FROM relationship_types WHERE name = ?")
                .bind(name)
                .fetch_one(tg.kg.pool())
                .await
                .unwrap(),
        )
        .fetch_optional(tg.kg.pool())
        .await
        .unwrap();
        assert!(
            category_rule.is_some(),
            "{name} must have a deterministic category rule"
        );
    }
}

#[tokio::test]
async fn facts_without_categories_receive_the_taxonomy_rule_category() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::enums::RecurrenceType;
    use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
    use mimir_knowledge::normalize::{NormalizedFact, Provenance, normalize_and_insert};

    let tg = TestGraph::new().await;
    let fact = NormalizedFact {
        confidence: None,
        source_type: SourceType::Interaction,
        subject: "Alice".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "prefers".to_string(),
        object: "Tennis".to_string(),
        object_is_entity: true,
        object_type: Some(EntityType::Activity),
        valid_from: None,
        valid_until: None,
        is_sensitive: false,
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
    };

    let outcome = normalize_and_insert(
        &tg.kg,
        vec![fact],
        Provenance::chat(ExtractionMethod::LlmExtraction),
    )
    .await
    .unwrap();
    assert_eq!(outcome.inserted.len(), 1, "{:?}", outcome.errors);

    let categories = tg
        .kg
        .get_categories_for_fact(outcome.inserted[0].id)
        .await
        .unwrap();
    assert_eq!(categories.len(), 1, "every fact must receive a category");
    assert_eq!(
        categories[0].id, 700,
        "preference leaves map to Entertainment & Leisure"
    );
}

#[tokio::test]
async fn unrecognized_facts_are_staged_and_resolvable() {
    let tg = TestGraph::new().await;
    let payload = serde_json::json!({"subject": "Alice", "object": "Bank"}).to_string();
    let id = tg
        .kg
        .stage_unrecognized_fact(None, Some("17:8"), "owes", &payload, None)
        .await
        .unwrap();

    let staged = tg
        .kg
        .list_unrecognized_facts(Some("unmapped"))
        .await
        .unwrap();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].id, id);
    assert_eq!(staged[0].relationship_type_raw, "owes");
    assert_eq!(staged[0].payload_json, payload);

    let leaf = tg
        .kg
        .get_relationship_type_id("has_event")
        .await
        .unwrap()
        .unwrap();
    tg.kg
        .resolve_unrecognized_fact(id, leaf, Some("mapped for review"))
        .await
        .unwrap();
    let mapped = tg.kg.list_unrecognized_facts(Some("mapped")).await.unwrap();
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].proposed_relationship_type_id, Some(leaf));
}

#[tokio::test]
async fn staged_fact_resolution_is_governed() {
    let tg = TestGraph::new().await;
    let id = tg
        .kg
        .stage_unrecognized_fact(None, Some("17:8"), "owes", "{}", None)
        .await
        .unwrap();
    let preference_root = tg
        .kg
        .get_relationship_type_id("preference")
        .await
        .unwrap()
        .unwrap();

    assert!(
        tg.kg
            .resolve_unrecognized_fact(id, preference_root, Some("query-only"))
            .await
            .is_err(),
        "a staged fact must not map to a query-only taxonomy node"
    );
    tg.kg
        .reject_unrecognized_fact(id, Some("not relevant"))
        .await
        .unwrap();
    let rejected = tg
        .kg
        .list_unrecognized_facts(Some("rejected"))
        .await
        .unwrap();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].status, "rejected");
    assert_eq!(rejected[0].resolution_note.as_deref(), Some("not relevant"));
}

#[tokio::test]
async fn distinct_unknown_facts_with_the_same_source_predicate_are_all_staged() {
    let tg = TestGraph::new().await;
    for object in ["Acme Bank", "First Bank"] {
        let payload = serde_json::json!({"object": object}).to_string();
        tg.kg
            .stage_unrecognized_fact(None, Some("17:8"), "owes", &payload, None)
            .await
            .unwrap();
    }

    let staged = tg
        .kg
        .list_unrecognized_facts(Some("unmapped"))
        .await
        .unwrap();
    assert_eq!(staged.len(), 2, "each distinct payload must survive");
}

#[tokio::test]
async fn direct_inserts_receive_the_deterministic_category_fallback() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;

    let tg = TestGraph::new().await;
    let person = tg
        .kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap();
    let fact = tg
        .kg
        .insert_fact(NewFact {
            object_literal: Some("Tennis".to_string()),
            object_id: None,
            ..NewFact::new(person.id, "prefers")
        })
        .await
        .unwrap();
    let categories = tg.kg.get_categories_for_fact(fact.id).await.unwrap();
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].id, 700);
}

#[tokio::test]
async fn batch_inserts_receive_the_deterministic_category_fallback() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;

    let tg = TestGraph::new().await;
    let person = tg
        .kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap();
    let facts = tg
        .kg
        .insert_facts_batch(vec![NewFact {
            object_literal: Some("Tennis".to_string()),
            ..NewFact::new(person.id, "likes")
        }])
        .await
        .unwrap();
    let categories = tg.kg.get_categories_for_fact(facts[0].id).await.unwrap();
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0].id, 700);
}

#[tokio::test]
async fn normalization_stages_unknown_predicates_instead_of_inserting_them() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::enums::{ConnectorType, RecurrenceType};
    use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
    let tg = TestGraph::new().await;
    let fact = mimir_knowledge::normalize::NormalizedFact {
        confidence: None,
        source_type: SourceType::Connector,
        subject: "Alice".to_string(),
        subject_type: EntityType::Person,
        relationship_type: "owes".to_string(),
        object: "Acme Bank".to_string(),
        object_is_entity: true,
        object_type: Some(EntityType::Organization),
        valid_from: None,
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: RecurrenceType::None,
        recurrence_rule: None,
        recurrence_interval: 1,
        recurrence_until: None,
        requires_user_action: false,
        raw_reference: Some("gmail:17:8".to_string()),
        extraction_method: Some(ExtractionMethod::LlmExtraction),
        event_type: None,
        location: None,
    };
    let connector = tg
        .kg
        .upsert_connector(mimir_knowledge::models::connector::UpsertConnectorInput {
            connector_type: ConnectorType::Email,
            slug: "gmail-test".to_string(),
            backend: "imap".to_string(),
            display_name: "Test Gmail".to_string(),
            config_json: "{}".to_string(),
            status: None,
            auth_state: None,
        })
        .await
        .unwrap();
    let provenance = mimir_knowledge::normalize::Provenance::connector(
        connector.id,
        ConnectorType::Email,
        ExtractionMethod::LlmExtraction,
    );

    let outcome = mimir_knowledge::normalize::normalize_and_insert(&tg.kg, vec![fact], provenance)
        .await
        .unwrap();

    assert!(outcome.inserted.is_empty());
    assert!(!outcome.errors.is_empty());
    let staged = tg
        .kg
        .list_unrecognized_facts(Some("unmapped"))
        .await
        .unwrap();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].connector_instance_id, Some(1));
    assert_eq!(staged[0].raw_reference.as_deref(), Some("gmail:17:8"));
    assert_eq!(staged[0].relationship_type_raw, "owes");
    assert!(staged[0].payload_json.contains("Acme Bank"));
}

#[tokio::test]
async fn direct_fact_inserts_reject_unknown_predicates_without_auto_creating_them() {
    use mimir_knowledge::models::entity::EntityType;
    use mimir_knowledge::models::fact::NewFact;

    let tg = TestGraph::new().await;
    let person = tg
        .kg
        .create_entity("Alice", EntityType::Person, &[])
        .await
        .unwrap();
    let unknown = NewFact::new(person.id, "owes");

    assert!(tg.kg.insert_fact(unknown).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM relationship_types WHERE name = 'owes'")
            .fetch_one(tg.kg.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn emit_eligible_resolution_rejects_query_only_nodes() {
    let tg = TestGraph::new().await;

    let _preference_root = tg
        .kg
        .get_relationship_type_id("preference")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tg.kg
            .resolve_emit_eligible_relationship_type("preference")
            .await
            .unwrap(),
        None,
        "taxonomy roots must never be emitted as facts"
    );

    let likes_leaf = tg
        .kg
        .get_relationship_type_id("likes")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tg.kg
            .resolve_emit_eligible_relationship_type("likes")
            .await
            .unwrap(),
        Some(likes_leaf)
    );
}

#[tokio::test]
async fn legacy_aliases_resolve_to_emit_eligible_leaves() {
    let tg = TestGraph::new().await;
    let resides_in = tg
        .kg
        .get_relationship_type_id("resides_in")
        .await
        .unwrap()
        .unwrap();

    for alias in ["lives_in", "likes", "loves"] {
        assert_eq!(
            tg.kg
                .resolve_emit_eligible_relationship_type(alias)
                .await
                .unwrap(),
            Some(if alias == "likes" || alias == "loves" {
                tg.kg
                    .get_relationship_type_id("prefers")
                    .await
                    .unwrap()
                    .unwrap()
            } else {
                resides_in
            }),
            "legacy alias {alias} must map to a controlled leaf"
        );
    }
}

#[tokio::test]
async fn extraction_schemas_are_generated_from_the_db_taxonomy() {
    let tg = TestGraph::new().await;
    let names = tg
        .kg
        .list_emit_eligible_relationship_type_names()
        .await
        .unwrap();

    assert!(
        names.contains(&"prefers".to_string()),
        "schema vocabulary must include the canonical positive-preference leaf"
    );
    assert!(
        !names.contains(&"preference".to_string()),
        "schema vocabulary must exclude query-only taxonomy roots"
    );

    let schema = mimir_knowledge::extract::remember_tool_schema(&names);
    let values = schema["function"]["parameters"]["properties"]["facts"]["items"]
        ["properties"]["relationship_type"]["enum"]
        .as_array()
        .expect("remember schema has a closed enum")
        .iter()
        .map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    assert_eq!(values.len(), names.len());
}
