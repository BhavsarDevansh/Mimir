//! FTS5 entity search and top-fact retrieval.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::entity::EntityType;
use crate::queries::entity::escape_fts5;

/// Summary of a matched entity.
#[derive(Debug, Clone, Serialize)]
pub struct EntitySummary {
    pub id: u32,
    pub name: String,
    pub entity_type: String,
}

/// Summary of a fact for search results.
#[derive(Debug, Clone, Serialize)]
pub struct FactSummary {
    pub predicate: String,
    pub object_name: Option<String>,
    pub object_literal: Option<String>,
    pub confidence: f32,
}

/// Result of a single entity match.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub entity: EntitySummary,
    pub match_score: f32,
    pub top_facts: Vec<FactSummary>,
}

/// Search entities via FTS5 and surface top facts per match.
pub async fn search_entities(
    pool: &SqlitePool,
    query: &str,
    entity_type_filter: Option<EntityType>,
    limit: i64,
) -> Result<Vec<SearchResult>, KnowledgeError> {
    let escaped = escape_fts5(query);

    // FTS5 search joined to entities and entity types.
    let rows = if let Some(et) = entity_type_filter {
        sqlx::query_as::<_, (i32, String, i16, String, f64)>(
            "SELECT e.id, e.name, e.entity_type_id, t.name as entity_type_name, entity_fts.rank as match_score \
             FROM entity_fts \
             JOIN entities e ON e.id = entity_fts.rowid \
             JOIN entity_types t ON t.id = e.entity_type_id \
             WHERE entity_fts MATCH ? AND e.entity_type_id = ? \
             ORDER BY entity_fts.rank \
             LIMIT ?",
        )
        .bind(&escaped)
        .bind(et as i16)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, (i32, String, i16, String, f64)>(
            "SELECT e.id, e.name, e.entity_type_id, t.name as entity_type_name, entity_fts.rank as match_score \
             FROM entity_fts \
             JOIN entities e ON e.id = entity_fts.rowid \
             JOIN entity_types t ON t.id = e.entity_type_id \
             WHERE entity_fts MATCH ? \
             ORDER BY entity_fts.rank \
             LIMIT ?",
        )
        .bind(&escaped)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let entity_ids: Vec<i32> = rows.iter().map(|r| r.0).collect();

    // Batch-fetch top-5 facts per matched entity using a window function.
    let placeholders: Vec<&str> = entity_ids.iter().map(|_| "?").collect();
    let facts_sql = format!(
        "SELECT subject_id, predicate_name, object_id, object_literal, confidence FROM ( \
            SELECT f.subject_id, p.name as predicate_name, f.object_id, f.object_literal, f.confidence, \
                   ROW_NUMBER() OVER (PARTITION BY f.subject_id ORDER BY f.confidence DESC) as rn \
             FROM facts f \
             JOIN predicates p ON p.id = f.predicate_id \
             WHERE f.subject_id IN ({}) \
               AND f.pending_confirmation = 0 \
               AND f.fact_status_id NOT IN (5, 6) \
         ) WHERE rn <= 5",
        placeholders.join(",")
    );
    let mut facts_query = sqlx::query_as::<_, (i32, String, Option<i32>, Option<String>, f32)>(
        sqlx::AssertSqlSafe(&*facts_sql),
    );
    for id in &entity_ids {
        facts_query = facts_query.bind(id);
    }
    let fact_rows = facts_query.fetch_all(pool).await?;

    // Batch-fetch object names for facts that reference entities.
    let object_ids: Vec<i32> = fact_rows
        .iter()
        .filter_map(|r| r.2)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let object_names = if object_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        let op: Vec<&str> = object_ids.iter().map(|_| "?").collect();
        let on_sql = format!(
            "SELECT id, name FROM entities WHERE id IN ({})",
            op.join(",")
        );
        let mut on_query = sqlx::query_as::<_, (i32, String)>(sqlx::AssertSqlSafe(&*on_sql));
        for id in &object_ids {
            on_query = on_query.bind(id);
        }
        on_query.fetch_all(pool).await?.into_iter().collect()
    };

    // Group facts by subject_id and keep top 5 per entity.
    let mut facts_by_subject: std::collections::HashMap<i32, Vec<FactSummary>> =
        std::collections::HashMap::new();
    for (subject_id, predicate_name, object_id, object_literal, confidence) in fact_rows {
        let object_name = object_id.and_then(|oid| object_names.get(&oid).cloned());
        facts_by_subject
            .entry(subject_id)
            .or_default()
            .push(FactSummary {
                predicate: predicate_name,
                object_name,
                object_literal,
                confidence,
            });
    }
    for facts in facts_by_subject.values_mut() {
        facts.truncate(5);
    }

    // Assemble results preserving FTS5 order.
    let mut results = Vec::with_capacity(rows.len());
    for (id, name, _, entity_type, rank) in rows {
        results.push(SearchResult {
            entity: EntitySummary {
                id: id as u32,
                name,
                entity_type,
            },
            match_score: rank as f32,
            top_facts: facts_by_subject.remove(&id).unwrap_or_default(),
        });
    }

    Ok(results)
}
