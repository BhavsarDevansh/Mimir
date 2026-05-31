use mimir_knowledge::KnowledgeGraph;

#[tokio::test]
async fn list_source_types() {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("knowledge.db"))
        .await
        .unwrap();

    let rows: Vec<(i32, String)> = sqlx::query_as("SELECT id, name FROM source_types ORDER BY id")
        .fetch_all(kg.pool())
        .await
        .unwrap();

    for (id, name) in &rows {
        println!("{}: {}", id, name);
    }

    assert!(rows.iter().any(|(id, _)| *id == 8), "Missing CasualMention");
    assert!(rows.iter().any(|(id, _)| *id == 9), "Missing Import");
    assert!(rows.iter().any(|(id, _)| *id == 10), "Missing System");
}
