//! Memory schema construction: ranking, budget fill, and bucketing.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::fact::FactStatus;
use crate::models::memory::{MemoryBucket, MemoryPriority, MemorySchema, RankedFact};
use crate::queries::memory::ranking::{
    compute_temporal_boost, determine_bucket, estimate_chars, truncate_fact,
};
use crate::queries::memory::{BuildMemoryOptions, RawRankedFact};

/// Build a ranked memory schema for the given subject.
///
/// 1. Pull facts for subject (≥ min_confidence, not pending, not superseded/forgotten).
/// 2. Enrich with categories, compute score per fact.
/// 3. Sort by score, fill budget greedily.
/// 4. Bucket into identity/relationships/preferences/upcoming/general.
pub async fn build_memory_schema(
    pool: &SqlitePool,
    subject_id: i32,
    budget: usize,
    min_confidence: f32,
    now: DateTime<Utc>,
    centrality_cache: &HashMap<i32, f32>,
) -> Result<MemorySchema, KnowledgeError> {
    build_memory_schema_with_opts(
        pool,
        subject_id,
        budget,
        min_confidence,
        now,
        centrality_cache,
        BuildMemoryOptions::default(),
    )
    .await
}

/// Build a ranked memory schema with filtering options.
pub async fn build_memory_schema_with_opts(
    pool: &SqlitePool,
    subject_id: i32,
    budget: usize,
    min_confidence: f32,
    now: DateTime<Utc>,
    centrality_cache: &HashMap<i32, f32>,
    opts: BuildMemoryOptions,
) -> Result<MemorySchema, KnowledgeError> {
    let mut sql = String::from(
        "SELECT f.id AS fact_id, s.name AS subject_name, rt.name AS relationship_type, COALESCE(o.name, f.object_literal) AS object_name, f.object_literal, f.confidence, f.valid_from, GROUP_CONCAT(fc.category_id) AS category_ids, MAX(c.memory_weight) AS memory_weight, f.memory_priority_id FROM facts f JOIN entities s ON s.id = f.subject_id JOIN relationship_types rt ON rt.id = f.relationship_type_id LEFT JOIN entities o ON o.id = f.object_id LEFT JOIN fact_categories fc ON fc.fact_id = f.id LEFT JOIN categories c ON c.id = fc.category_id WHERE f.subject_id = ? AND f.pending_confirmation = 0 AND f.fact_status_id NOT IN (?, ?) AND f.confidence >= ?",
    );
    if opts.exclude_sensitive {
        sql.push_str(" AND rt.sensitive = FALSE");
    }
    sql.push_str(" GROUP BY f.id ORDER BY f.confidence DESC");

    let rows: Vec<RawRankedFact> = sqlx::query_as::<_, RawRankedFact>(sqlx::AssertSqlSafe(&*sql))
        .bind(subject_id)
        .bind(FactStatus::Superseded as i16)
        .bind(FactStatus::Forgotten as i16)
        .bind(min_confidence)
        .fetch_all(pool)
        .await?;

    let mut ranked: Vec<RankedFact> = Vec::with_capacity(rows.len());

    for raw in rows {
        let cat_ids: Vec<i32> = raw
            .category_ids
            .as_ref()
            .map(|s| s.split(',').filter_map(|id| id.parse().ok()).collect())
            .unwrap_or_default();

        let temporal_boost = compute_temporal_boost(raw.valid_from, now);
        let memory_weight = raw.memory_weight.unwrap_or(0.50);
        let priority = match raw.memory_priority_id {
            1 => MemoryPriority::Critical,
            2 => MemoryPriority::High,
            3 => MemoryPriority::Normal,
            4 => MemoryPriority::Low,
            _ => MemoryPriority::Normal,
        };
        let priority_boost = priority.boost();

        // Centrality reflects how connected the memory subject is in the graph.
        // All facts in this schema share the same subject, so we use subject_id.
        let centrality_boost = centrality_cache.get(&subject_id).copied().unwrap_or(1.0);

        let score =
            raw.confidence * memory_weight * temporal_boost * priority_boost * centrality_boost;

        let object_display = raw
            .object_name
            .unwrap_or_else(|| raw.object_literal.unwrap_or_default());
        let char_estimate =
            estimate_chars(&raw.subject_name, &raw.relationship_type, &object_display);

        let bucket = determine_bucket(&cat_ids);

        ranked.push(RankedFact {
            fact_id: raw.fact_id,
            subject_name: raw.subject_name,
            relationship_type: raw.relationship_type,
            object_display,
            confidence: raw.confidence,
            score,
            temporal_boost,
            memory_weight,
            priority_boost,
            centrality_boost,
            category_ids: cat_ids,
            bucket,
            char_estimate,
        });
    }

    // Sort by score descending
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Fill budget greedily: identity first, then by score.
    let mut schema = MemorySchema::new();
    let mut remaining_budget = budget;

    // Phase 1: identity bucket always first (up to ~200 chars with rollover)
    let mut identity_facts: Vec<RankedFact> = Vec::new();
    let mut non_identity: Vec<RankedFact> = Vec::new();
    for fact in ranked {
        if fact.bucket == MemoryBucket::Identity {
            identity_facts.push(fact);
        } else {
            non_identity.push(fact);
        }
    }

    // Reserve ~200 chars for identity, but whatever is unused rolls over.
    let identity_budget = remaining_budget.min(200);
    let mut identity_used = 0usize;
    for fact in identity_facts {
        if identity_used + fact.char_estimate <= identity_budget {
            identity_used += fact.char_estimate;
            schema.identity.push(fact);
        } else {
            non_identity.push(fact); // unfitted identity falls back to general pool
        }
    }
    remaining_budget = remaining_budget.saturating_sub(identity_used);

    // Phase 2: fill remaining budget by score across all non-identity buckets.
    non_identity.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let excluded_budget: Vec<MemoryBucket> = opts.exclude_from_budget.clone();
    for fact in non_identity {
        let consumes_budget = !excluded_budget.contains(&fact.bucket);
        if consumes_budget && fact.char_estimate <= remaining_budget {
            remaining_budget -= fact.char_estimate;
            match fact.bucket {
                MemoryBucket::Relationships => schema.relationships.push(fact),
                MemoryBucket::Preferences => schema.preferences.push(fact),
                MemoryBucket::Upcoming => schema.upcoming.push(fact),
                _ => schema.general.push(fact),
            }
        } else if consumes_budget && remaining_budget > 0 {
            // Truncate last entry with …
            let truncated = truncate_fact(fact, remaining_budget);
            match truncated.bucket {
                MemoryBucket::Relationships => schema.relationships.push(truncated),
                MemoryBucket::Preferences => schema.preferences.push(truncated),
                MemoryBucket::Upcoming => schema.upcoming.push(truncated),
                _ => schema.general.push(truncated),
            }
            remaining_budget = 0;
            continue;
        } else if !consumes_budget {
            // Excluded from budget: always include, never count against budget
            match fact.bucket {
                MemoryBucket::Relationships => schema.relationships.push(fact),
                MemoryBucket::Preferences => schema.preferences.push(fact),
                MemoryBucket::Upcoming => schema.upcoming.push(fact),
                _ => schema.general.push(fact),
            }
        }
    }

    schema.char_count = budget - remaining_budget;
    schema.total_score = schema.all_facts().iter().map(|f| f.score).sum();

    Ok(schema)
}

// ---------------------------------------------------------------------------
// Scoring helpers
// ---------------------------------------------------------------------------
