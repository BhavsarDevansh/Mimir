//! BFS graph traversal for related-entity discovery.

use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashSet;

use crate::KnowledgeError;
use crate::queries::entity::get_entity_names;

/// A single edge in the traversal result.
#[derive(Debug, Clone, Serialize)]
pub struct TraversalEdge {
    pub depth: u32,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
}

/// Result of a graph traversal.
#[derive(Debug, Clone)]
pub struct TraversalResult {
    pub edges: Vec<TraversalEdge>,
    pub max_depth_reached: u32,
    pub nodes_found: usize,
}

/// Breadth-first traversal from `root_id` following entity-to-entity facts.
pub async fn traverse_graph(
    pool: &SqlitePool,
    root_id: u32,
    max_depth: u32,
    max_nodes: u32,
    predicate_filter: Option<&[i16]>,
) -> Result<TraversalResult, KnowledgeError> {
    let mut visited: HashSet<u32> = HashSet::new();
    visited.insert(root_id);
    let mut edges: Vec<TraversalEdge> = Vec::new();
    let mut max_depth_reached: u32 = 0;
    let mut frontier: Vec<u32> = vec![root_id];

    for depth in 0..max_depth {
        if frontier.is_empty() || visited.len() >= max_nodes as usize {
            break;
        }

        let remaining_budget = max_nodes as usize - visited.len();
        let sql_limit = (remaining_budget * 2) as i64;

        // Guard against empty predicate_filter which would emit "IN ()" that SQLite rejects.
        if let Some(predicates) = predicate_filter {
            if predicates.is_empty() {
                return Ok(TraversalResult {
                    edges: Vec::new(),
                    max_depth_reached: 0,
                    nodes_found: visited.len(),
                });
            }
        }

        // Build the IN clause for the frontier.
        let placeholders: Vec<&str> = frontier.iter().map(|_| "?").collect();
        let sql = if let Some(predicates) = predicate_filter {
            let pred_placeholders: Vec<&str> = predicates.iter().map(|_| "?").collect();
            format!(
                "SELECT f.subject_id, f.relationship_type_id, rt.name as predicate_name, \
                        f.object_id, f.object_literal, f.confidence \
                 FROM facts f \
                 JOIN relationship_types rt ON rt.id = f.relationship_type_id \
                 WHERE f.subject_id IN ({}) \
                   AND f.pending_confirmation = 0 \
                   AND f.fact_status_id NOT IN (5, 6) \
                   AND f.relationship_type_id IN ({}) \
                 ORDER BY f.confidence DESC \
                 LIMIT {}",
                placeholders.join(","),
                pred_placeholders.join(","),
                sql_limit
            )
        } else {
            format!(
                "SELECT f.subject_id, f.relationship_type_id, rt.name as predicate_name, \
                        f.object_id, f.object_literal, f.confidence \
                 FROM facts f \
                 JOIN relationship_types rt ON rt.id = f.relationship_type_id \
                 WHERE f.subject_id IN ({}) \
                   AND f.pending_confirmation = 0 \
                   AND f.fact_status_id NOT IN (5, 6) \
                 ORDER BY f.confidence DESC \
                 LIMIT {}",
                placeholders.join(","),
                sql_limit
            )
        };

        let mut query = sqlx::query_as::<_, (i32, i16, String, Option<i32>, Option<String>, f32)>(
            sqlx::AssertSqlSafe(&*sql),
        );
        for &id in &frontier {
            query = query.bind(id as i32);
        }
        if let Some(predicates) = predicate_filter {
            for &pid in predicates {
                query = query.bind(pid);
            }
        }

        let rows = query.fetch_all(pool).await?;

        // Collect all subject and object IDs that need name resolution.
        let mut ids_to_resolve: Vec<u32> = Vec::new();
        for (subject_id, _, _, object_id, _, _) in &rows {
            ids_to_resolve.push(*subject_id as u32);
            if let Some(oid) = object_id {
                ids_to_resolve.push(*oid as u32);
            }
        }
        ids_to_resolve.sort_unstable();
        ids_to_resolve.dedup();

        let names = get_entity_names(pool, &ids_to_resolve).await?;

        let mut next_frontier: Vec<u32> = Vec::new();
        for (subject_id, _, predicate_name, object_id, object_literal, confidence) in &rows {
            let subject_name = names
                .get(&(*subject_id as u32))
                .cloned()
                .unwrap_or_else(|| format!("entity:{}", subject_id));
            let object_str = if let Some(oid) = object_id {
                names
                    .get(&(*oid as u32))
                    .cloned()
                    .unwrap_or_else(|| format!("entity:{}", oid))
            } else {
                object_literal.clone().unwrap_or_default()
            };

            edges.push(TraversalEdge {
                depth,
                subject: subject_name,
                predicate: predicate_name.clone(),
                object: object_str,
                confidence: *confidence,
            });

            if let Some(oid) = object_id {
                let oid_u32 = *oid as u32;
                if !visited.contains(&oid_u32) && visited.len() < max_nodes as usize {
                    visited.insert(oid_u32);
                    next_frontier.push(oid_u32);
                }
            }
        }

        if !rows.is_empty() {
            max_depth_reached = depth + 1;
        }
        frontier = next_frontier;
    }

    Ok(TraversalResult {
        nodes_found: visited.len(),
        max_depth_reached,
        edges,
    })
}
