//! Duplicate detection and merging: exact duplicates, overlapping aliases,
//! semantic-dedup queue.

use std::collections::HashSet;
use std::sync::Arc;

use mimir_core::llm::LlmBackend;
use mimir_core::llm::types::Message;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::entity::{Entity, EntityType};
use crate::models::enums::MergeWorkflowStatus;
use crate::queries::entity::crud::get_by_id;

pub async fn find_exact_duplicates(
    pool: &SqlitePool,
) -> Result<Vec<(Entity, Entity)>, KnowledgeError> {
    let rows: Vec<(i32, i32)> = sqlx::query_as(
        "WITH dup_names AS ( \
            SELECT LOWER(name) AS lower_name \
            FROM entities \
            GROUP BY LOWER(name) \
            HAVING COUNT(*) > 1 \
         ) \
         SELECT a.id, b.id \
         FROM entities a \
         JOIN dup_names d ON LOWER(a.name) = d.lower_name \
         JOIN entities b ON LOWER(b.name) = d.lower_name AND a.id < b.id",
    )
    .fetch_all(pool)
    .await?;

    let mut pairs = Vec::new();
    for (id_a, id_b) in rows {
        if let (Some(a), Some(b)) = (get_by_id(pool, id_a).await?, get_by_id(pool, id_b).await?) {
            pairs.push((a, b));
        }
    }
    Ok(pairs)
}

/// Auto-merge two exact-duplicate entities.
///
/// - Facts referencing the merged entity are repointed to the survivor.
/// - Aliases from the merged entity are appended to the survivor.
/// - The merged entity is hard-deleted.
pub async fn auto_merge_pair(
    pool: &SqlitePool,
    survivor_id: i32,
    merged_id: i32,
) -> Result<(), KnowledgeError> {
    if survivor_id == merged_id {
        return Err(KnowledgeError::Validation(
            "survivor and merged IDs must differ".to_string(),
        ));
    }

    // Pick survivor as the one with the most facts (if tied, prefer the lower ID as stable heuristic).
    let (survivor_facts,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE subject_id = ? OR object_id = ?")
            .bind(survivor_id)
            .bind(survivor_id)
            .fetch_one(pool)
            .await?;

    let (merged_facts,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM facts WHERE subject_id = ? OR object_id = ?")
            .bind(merged_id)
            .bind(merged_id)
            .fetch_one(pool)
            .await?;

    let (actual_survivor, actual_merged) = if survivor_facts >= merged_facts {
        (survivor_id, merged_id)
    } else {
        (merged_id, survivor_id)
    };

    let mut tx = pool.begin().await?;

    // 1. Repoint facts.
    sqlx::query("UPDATE facts SET subject_id = ? WHERE subject_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    sqlx::query("UPDATE facts SET object_id = ? WHERE object_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 2. Move aliases from merged to survivor (ignore duplicates).
    let aliases: Vec<(String,)> =
        sqlx::query_as("SELECT alias FROM entity_aliases WHERE entity_id = ?")
            .bind(actual_merged)
            .fetch_all(&mut *tx)
            .await?;

    for (alias,) in aliases {
        sqlx::query("INSERT OR IGNORE INTO entity_aliases (entity_id, alias) VALUES (?, ?)")
            .bind(actual_survivor)
            .bind(alias)
            .execute(&mut *tx)
            .await?;
    }

    // 3. Refresh survivor aliases JSON.
    let survivor_aliases: Vec<(String,)> =
        sqlx::query_as("SELECT alias FROM entity_aliases WHERE entity_id = ?")
            .bind(actual_survivor)
            .fetch_all(&mut *tx)
            .await?;

    let json = if survivor_aliases.is_empty() {
        None
    } else {
        let vec: Vec<String> = survivor_aliases.into_iter().map(|a| a.0).collect();
        Some(serde_json::to_string(&vec).unwrap_or_else(|_| "[]".to_string()))
    };

    sqlx::query("UPDATE entities SET aliases = ? WHERE id = ?")
        .bind(json)
        .bind(actual_survivor)
        .execute(&mut *tx)
        .await?;

    // 4. Migrate event overlays to survivor (denormalized entity_id; the
    //    underlying facts were already repointed in step 1).
    sqlx::query("UPDATE events SET entity_id = ? WHERE entity_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 5. Migrate entity_locations to survivor.
    sqlx::query("UPDATE entity_locations SET entity_id = ? WHERE entity_id = ?")
        .bind(actual_survivor)
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 6. Remove preferences for merged entity to avoid FK violation.
    sqlx::query("DELETE FROM preferences WHERE entity_id = ?")
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    // 7. Remove merge-queue entries referencing merged entity.
    sqlx::query(
        "DELETE FROM entity_merge_queue WHERE primary_entity_id = ? OR duplicate_entity_id = ?",
    )
    .bind(actual_merged)
    .bind(actual_merged)
    .execute(&mut *tx)
    .await?;

    // 8. Delete merged entity (cascades entity_aliases thanks to ON DELETE CASCADE).
    sqlx::query("DELETE FROM entities WHERE id = ?")
        .bind(actual_merged)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

/// Find pairs of entities that share at least one alias string.
pub async fn find_overlapping_aliases(
    pool: &SqlitePool,
) -> Result<Vec<(Entity, Entity, String)>, KnowledgeError> {
    let rows: Vec<(i32, i32, String)> = sqlx::query_as(
        "SELECT a.entity_id, b.entity_id, a.alias \
         FROM entity_aliases a \
         JOIN entity_aliases b ON LOWER(a.alias) = LOWER(b.alias) AND a.entity_id < b.entity_id",
    )
    .fetch_all(pool)
    .await?;

    let mut pairs = Vec::new();
    for (id_a, id_b, alias) in rows {
        if let (Some(a), Some(b)) = (get_by_id(pool, id_a).await?, get_by_id(pool, id_b).await?) {
            pairs.push((a, b, alias));
        }
    }
    Ok(pairs)
}

/// Flag overlapping aliases in the entity_merge_queue for human review.
pub async fn flag_overlapping_aliases(pool: &SqlitePool) -> Result<(), KnowledgeError> {
    let overlaps = find_overlapping_aliases(pool).await?;
    for (a, b, _alias) in overlaps {
        sqlx::query(
            "INSERT OR IGNORE INTO entity_merge_queue (primary_entity_id, duplicate_entity_id, status_id) \
             VALUES (?, ?, ?)",
        )
        .bind(a.id)
        .bind(b.id)
        .bind(MergeWorkflowStatus::Pending as i16)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Deterministic pre-filter for the LLM semantic-dedup pass (issue #282):
/// same-type entity pairs that share an alias string or whose names are
/// equal / one contained in the other (plain substring via `INSTR` — no
/// wildcard semantics), excluding pairs that are already LLM-evaluated or
/// human-resolved in the queue. The result is capped so a single pass sends
/// a bounded number of candidates to the LLM.
pub async fn find_semantic_candidates(
    pool: &SqlitePool,
    cap: i64,
) -> Result<Vec<(Entity, Entity)>, KnowledgeError> {
    let rows: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT e1.id, e2.id \
         FROM entities e1 \
         JOIN entities e2 ON e2.id > e1.id \
          AND e1.entity_type_id = e2.entity_type_id \
         WHERE ( \
            LOWER(e1.name) = LOWER(e2.name) \
            OR (LENGTH(e1.name) >= 3 AND INSTR(LOWER(e2.name), LOWER(e1.name)) > 0) \
            OR (LENGTH(e2.name) >= 3 AND INSTR(LOWER(e1.name), LOWER(e2.name)) > 0) \
            OR EXISTS ( \
                SELECT 1 FROM entity_aliases a1 \
                JOIN entity_aliases a2 ON LOWER(a2.alias) = LOWER(a1.alias) \
                WHERE a1.entity_id = e1.id AND a2.entity_id = e2.id \
            ) \
         ) \
         AND NOT EXISTS ( \
            SELECT 1 FROM entity_merge_queue q \
            WHERE q.primary_entity_id = e1.id AND q.duplicate_entity_id = e2.id \
              AND (q.status_id != ? OR q.llm_confidence IS NOT NULL) \
         ) \
         ORDER BY e1.id, e2.id \
         LIMIT ?",
    )
    .bind(MergeWorkflowStatus::Pending as i16)
    .bind(cap)
    .fetch_all(pool)
    .await?;

    let mut pairs = Vec::new();
    for (id_a, id_b) in rows {
        if let (Some(a), Some(b)) = (get_by_id(pool, id_a).await?, get_by_id(pool, id_b).await?) {
            pairs.push((a, b));
        }
    }
    Ok(pairs)
}

/// Evaluate candidate entity pairs with the LLM under a strict tool schema
/// and write the results into `entity_merge_queue` for human review.
///
/// Rust-side validation (per the "logic in Rust" rule): only pairs that were
/// actually in `candidate_pairs` are accepted, the suggested action must be
/// one of the schema's enum values, and the confidence must be a finite
/// number in `[0, 1]`. Pairs are stored id-ordered so the
/// `UNIQUE(primary_entity_id, duplicate_entity_id)` constraint can never be
/// bypassed with a mirrored row; re-evaluation of an existing pending row
/// updates its LLM fields instead of duplicating it.
///
/// Returns the number of queue rows written or enriched.
pub async fn enqueue_semantic_dedup(
    pool: &SqlitePool,
    candidate_pairs: Vec<(Entity, Entity)>,
    llm: &Arc<dyn LlmBackend>,
) -> Result<u32, KnowledgeError> {
    if candidate_pairs.is_empty() {
        return Ok(0);
    }

    let candidate_json: Vec<serde_json::Value> = candidate_pairs
        .iter()
        .map(|(a, b)| {
            serde_json::json!({
                "entity_a_id": a.id,
                "entity_b_id": b.id,
                "name_a": a.name,
                "name_b": b.name,
                "type_a": EntityType::try_from(a.entity_type_id).map(EntityType::as_str).unwrap_or("Unknown"),
                "type_b": EntityType::try_from(b.entity_type_id).map(EntityType::as_str).unwrap_or("Unknown"),
            })
        })
        .collect();

    let tool = entity_dedup_tool_schema();
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: "Use the evaluate_entity_dedup_candidates tool to return your evaluation."
                .to_string(),
            tool_calls: None,
            tool_call_id: None,
        },
        Message {
            role: "user".to_string(),
            content: serde_json::to_string(&candidate_json)
                .map_err(|e| crate::KnowledgeError::Validation(e.to_string()))?,
            tool_calls: None,
            tool_call_id: None,
        },
    ];
    let (assistant_msg, _) = llm
        .chat_message(messages, Some(vec![tool]))
        .await
        .map_err(|e| {
            crate::KnowledgeError::Validation(format!("entity semantic dedup LLM error: {e}"))
        })?;

    let tool_calls = assistant_msg.tool_calls.as_ref().ok_or_else(|| {
        crate::KnowledgeError::Validation(
            "entity semantic dedup: no tool calls in LLM response".to_string(),
        )
    })?;
    let first = tool_calls.first().ok_or_else(|| {
        crate::KnowledgeError::Validation(
            "entity semantic dedup: empty tool calls in LLM response".to_string(),
        )
    })?;
    let response: EntitySemanticDedupResponse = serde_json::from_str(&first.function.arguments)
        .map_err(|e| {
            crate::KnowledgeError::Validation(format!("entity semantic dedup JSON error: {e}"))
        })?;

    let valid_pairs: HashSet<(i32, i32)> = candidate_pairs
        .iter()
        .map(|(a, b)| ordered_pair(a.id, b.id))
        .collect();

    let mut queued = 0;
    let mut written_pairs: HashSet<(i32, i32)> = HashSet::new();
    for candidate in response.candidates {
        let pair = ordered_pair(candidate.entity_a_id, candidate.entity_b_id);
        if !valid_pairs.contains(&pair) {
            tracing::warn!(
                "entity semantic dedup: skipping pair {:?} not in candidate set",
                pair
            );
            continue;
        }
        let valid_action = match candidate.suggested_action.as_str() {
            "merge" | "keep_separate" => true,
            _ => {
                tracing::warn!(
                    "entity semantic dedup: skipping pair {:?} with invalid action `{}`",
                    pair,
                    candidate.suggested_action
                );
                false
            }
        };
        if !valid_action {
            continue;
        }
        if !candidate.llm_confidence.is_finite() || !(0.0..=1.0).contains(&candidate.llm_confidence)
        {
            tracing::warn!(
                "entity semantic dedup: skipping pair {:?} with out-of-range confidence {}",
                pair,
                candidate.llm_confidence
            );
            continue;
        }
        if written_pairs.contains(&pair) {
            continue;
        }

        let written = sqlx::query(
            "INSERT INTO entity_merge_queue \
             (primary_entity_id, duplicate_entity_id, status_id, suggested_action, llm_confidence) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(primary_entity_id, duplicate_entity_id) DO UPDATE SET \
                suggested_action = excluded.suggested_action, \
                llm_confidence = excluded.llm_confidence \
             WHERE entity_merge_queue.status_id = ?",
        )
        .bind(pair.0)
        .bind(pair.1)
        .bind(MergeWorkflowStatus::Pending as i16)
        .bind(candidate.suggested_action)
        .bind(candidate.llm_confidence)
        .bind(MergeWorkflowStatus::Pending as i16)
        .execute(pool)
        .await?;
        if written.rows_affected() > 0 {
            written_pairs.insert(pair);
            queued += 1;
        }
    }
    Ok(queued)
}

fn entity_dedup_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "evaluate_entity_dedup_candidates",
            "description": "Evaluate candidate entity pairs for semantic deduplication and return structured results.",
            "parameters": {
                "type": "object",
                "properties": {
                    "candidates": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "entity_a_id": { "type": "integer" },
                                "entity_b_id": { "type": "integer" },
                                "suggested_action": {
                                    "type": "string",
                                    "enum": ["merge", "keep_separate"]
                                },
                                "llm_confidence": { "type": "number" }
                            },
                            "required": ["entity_a_id", "entity_b_id", "suggested_action", "llm_confidence"]
                        }
                    }
                },
                "required": ["candidates"]
            }
        }
    })
}

#[derive(Debug, Deserialize)]
struct EntitySemanticDedupResponse {
    candidates: Vec<EntitySemanticDedupCandidate>,
}

#[derive(Debug, Deserialize)]
struct EntitySemanticDedupCandidate {
    entity_a_id: i32,
    entity_b_id: i32,
    suggested_action: String,
    llm_confidence: f32,
}

/// Normalize a candidate pair to ascending id order so the
/// `UNIQUE(primary_entity_id, duplicate_entity_id)` constraint cannot be
/// bypassed by mirrored rows.
pub(crate) fn ordered_pair(a: i32, b: i32) -> (i32, i32) {
    if a <= b { (a, b) } else { (b, a) }
}
