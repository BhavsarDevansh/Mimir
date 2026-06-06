//! Memory ranking, selection, budget fill, and deterministic fallback rendering.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::KnowledgeError;
use crate::models::fact::FactStatus;
use crate::models::memory::{MemoryBucket, MemoryPriority, MemorySchema, RankedFact};

// ---------------------------------------------------------------------------
// Raw row from the enriched fact query
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct RawRankedFact {
    fact_id: i32,
    subject_name: String,
    relationship_type: String,
    object_name: Option<String>,
    object_literal: Option<String>,
    confidence: f32,
    valid_from: Option<DateTime<Utc>>,
    category_ids: Option<String>, // comma-separated
    memory_weight: Option<f32>,
    memory_priority_id: i16,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

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
    let rows: Vec<RawRankedFact> = sqlx::query_as(
        "SELECT \
            f.id AS fact_id, \
            s.name AS subject_name, \
            rt.name AS relationship_type, \
            COALESCE(o.name, f.object_literal) AS object_name, \
            f.object_literal, \
            f.confidence, \
            f.valid_from, \
            GROUP_CONCAT(fc.category_id) AS category_ids, \
            MAX(c.memory_weight) AS memory_weight, \
            f.memory_priority_id \
         FROM facts f \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         LEFT JOIN fact_categories fc ON fc.fact_id = f.id \
         LEFT JOIN categories c ON c.id = fc.category_id \
         WHERE f.subject_id = ? \
           AND f.pending_confirmation = 0 \
           AND f.fact_status_id NOT IN (?, ?) \
           AND f.confidence >= ? \
         GROUP BY f.id \
         ORDER BY f.confidence DESC",
    )
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

        // Centrality uses the subject entity's connection count.
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

    for fact in non_identity {
        if fact.char_estimate <= remaining_budget {
            remaining_budget -= fact.char_estimate;
            match fact.bucket {
                MemoryBucket::Relationships => schema.relationships.push(fact),
                MemoryBucket::Preferences => schema.preferences.push(fact),
                MemoryBucket::Upcoming => schema.upcoming.push(fact),
                _ => schema.general.push(fact),
            }
        } else if remaining_budget > 0 {
            // Truncate last entry with …
            let truncated = truncate_fact(fact, remaining_budget);
            match truncated.bucket {
                MemoryBucket::Relationships => schema.relationships.push(truncated),
                MemoryBucket::Preferences => schema.preferences.push(truncated),
                MemoryBucket::Upcoming => schema.upcoming.push(truncated),
                _ => schema.general.push(truncated),
            }
            remaining_budget = 0;
            break;
        }
    }

    schema.char_count = budget - remaining_budget;
    schema.total_score = schema.all_facts().iter().map(|f| f.score).sum();

    Ok(schema)
}

// ---------------------------------------------------------------------------
// Scoring helpers
// ---------------------------------------------------------------------------

/// Temporal boost for facts with a future valid_from date.
/// boost = 10.0 / sqrt(max(days, 0.5))
/// If no future date, boost = 1.0.
pub fn compute_temporal_boost(valid_from: Option<DateTime<Utc>>, now: DateTime<Utc>) -> f32 {
    let Some(from) = valid_from else {
        return 1.0;
    };
    if from <= now {
        return 1.0;
    }
    let days = (from - now).num_seconds() as f64 / 86400.0;
    let days = days.max(0.5);
    (10.0 / days.sqrt()) as f32
}

/// Determine the memory bucket from category IDs.
/// Priority: identity > upcoming > relationships > preferences > general.
pub fn determine_bucket(category_ids: &[i32]) -> MemoryBucket {
    let mut has_identity = false;
    let mut has_upcoming = false;
    let mut has_relationships = false;
    let mut has_preferences = false;

    for &id in category_ids {
        if (100..=199).contains(&id) {
            has_identity = true;
        } else if (900..=999).contains(&id) {
            has_upcoming = true;
        } else if (400..=499).contains(&id) {
            has_relationships = true;
        } else if [
            300, 301, 302, 303, 304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316,
            317, 318, 319, 320, 321, 322, 323, 324, 325, 326, 327, 328, 329, 330, 331, 332, 333,
            334, 335, 336, 337, 338, 339, 340, 341, 342, 343, 344, 345, 346, 347, 348, 349, 350,
            351, 352, 353, 354, 355, 356, 357, 358, 359, 360, 361, 362, 363, 364, 365, 366, 367,
            368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378, 379, 380, 381, 382, 383, 384,
            385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 398, 399, 460, 480,
            570, 670, 680, 690, 830, 870,
        ]
        .contains(&id)
        {
            has_preferences = true;
        }
    }

    if has_identity {
        MemoryBucket::Identity
    } else if has_upcoming {
        MemoryBucket::Upcoming
    } else if has_relationships {
        MemoryBucket::Relationships
    } else if has_preferences {
        MemoryBucket::Preferences
    } else {
        MemoryBucket::General
    }
}

/// Rough character estimate for a rendered fact.
fn estimate_chars(subject: &str, relationship: &str, object: &str) -> usize {
    // Template: "{subject} {relationship} {object}. "
    subject.len() + relationship.len() + object.len() + 3
}

/// Truncate a fact to fit the remaining budget, appending `…`.
fn truncate_fact(mut fact: RankedFact, budget: usize) -> RankedFact {
    let max_obj = budget.saturating_sub(fact.subject_name.len() + fact.relationship_type.len() + 3);
    if max_obj == 0 {
        fact.object_display = "…".to_string();
    } else if fact.object_display.len() > max_obj {
        fact.object_display = format!("{}…", &fact.object_display[..max_obj.saturating_sub(1)]);
    }
    fact.char_estimate = budget;
    fact
}

// ---------------------------------------------------------------------------
// Deterministic fallback renderer
// ---------------------------------------------------------------------------

/// Render a MemorySchema into concise plain text.
/// Identity facts are rendered first without a header; other buckets get headers.
pub fn render_memory_schema(schema: &MemorySchema) -> String {
    let mut out = String::new();

    for fact in &schema.identity {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&render_fact_line(fact));
        out.push('.');
    }

    render_bucket(&mut out, "Relationships", &schema.relationships);
    render_bucket(&mut out, "Preferences", &schema.preferences);
    render_bucket(&mut out, "Upcoming", &schema.upcoming);
    render_bucket(&mut out, "General", &schema.general);

    out
}

fn render_bucket(out: &mut String, header: &str, facts: &[RankedFact]) {
    if facts.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(header);
    out.push_str(": ");
    for (i, fact) in facts.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&render_fact_line(fact));
        out.push('.');
    }
}

fn render_fact_line(fact: &RankedFact) -> String {
    let rel = &fact.relationship_type;
    match rel.as_str() {
        "has_partner" => format!(
            "{} is partnered with {}",
            fact.subject_name, fact.object_display
        ),
        "has_parent" => format!("{} has parent {}", fact.subject_name, fact.object_display),
        "born_on" => format!("{} was born on {}", fact.subject_name, fact.object_display),
        "died_on" => format!("{} died on {}", fact.subject_name, fact.object_display),
        "works_as" => format!("{} works as {}", fact.subject_name, fact.object_display),
        "located_in" | "is_in" => format!("{} is in {}", fact.subject_name, fact.object_display),
        "owns" => format!("{} owns {}", fact.subject_name, fact.object_display),
        "visited" => format!("{} visited {}", fact.subject_name, fact.object_display),
        "created_on" => format!("{} created on {}", fact.subject_name, fact.object_display),
        "rejected_action" => format!(
            "{} rejected action {}",
            fact.subject_name, fact.object_display
        ),
        _ => format!(
            "{} {} {}",
            fact.subject_name,
            rel.replace('_', " "),
            fact.object_display
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporal_boost_zero_days() {
        let now = Utc::now();
        let boost = compute_temporal_boost(Some(now + chrono::Duration::seconds(1)), now);
        assert!(
            (boost - 14.14).abs() < 0.01,
            "expected ~14.14, got {}",
            boost
        );
    }

    #[test]
    fn temporal_boost_one_day() {
        let now = Utc::now();
        let boost = compute_temporal_boost(Some(now + chrono::Duration::days(1)), now);
        assert!((boost - 10.0).abs() < 0.01, "expected ~10.0, got {}", boost);
    }

    #[test]
    fn temporal_boost_past_date() {
        let now = Utc::now();
        let boost = compute_temporal_boost(Some(now - chrono::Duration::days(10)), now);
        assert_eq!(boost, 1.0);
    }

    #[test]
    fn temporal_boost_no_date() {
        let now = Utc::now();
        let boost = compute_temporal_boost(None, now);
        assert_eq!(boost, 1.0);
    }

    #[test]
    fn priority_boost_values() {
        assert_eq!(MemoryPriority::Critical.boost(), 2.0);
        assert_eq!(MemoryPriority::High.boost(), 1.5);
        assert_eq!(MemoryPriority::Normal.boost(), 1.0);
        assert_eq!(MemoryPriority::Low.boost(), 0.5);
    }

    #[test]
    fn bucket_identity_wins() {
        assert_eq!(determine_bucket(&[150, 400]), MemoryBucket::Identity);
    }

    #[test]
    fn bucket_upcoming_second() {
        assert_eq!(determine_bucket(&[910, 400]), MemoryBucket::Upcoming);
    }

    #[test]
    fn bucket_relationships_third() {
        assert_eq!(determine_bucket(&[420, 300]), MemoryBucket::Relationships);
    }

    #[test]
    fn bucket_preferences_fourth() {
        assert_eq!(determine_bucket(&[300, 500]), MemoryBucket::Preferences);
    }

    #[test]
    fn bucket_general_fallback() {
        assert_eq!(determine_bucket(&[500, 600]), MemoryBucket::General);
    }

    #[test]
    fn render_memory_schema_basic() {
        let schema = MemorySchema {
            identity: vec![RankedFact {
                fact_id: 1,
                subject_name: "Devansh".to_string(),
                relationship_type: "works_as".to_string(),
                object_display: "software developer".to_string(),
                confidence: 0.95,
                score: 1.0,
                temporal_boost: 1.0,
                memory_weight: 1.0,
                priority_boost: 1.0,
                centrality_boost: 1.0,
                category_ids: vec![150],
                bucket: MemoryBucket::Identity,
                char_estimate: 40,
            }],
            relationships: vec![RankedFact {
                fact_id: 2,
                subject_name: "Devansh".to_string(),
                relationship_type: "has_partner".to_string(),
                object_display: "Alice".to_string(),
                confidence: 0.95,
                score: 1.0,
                temporal_boost: 1.0,
                memory_weight: 1.0,
                priority_boost: 1.0,
                centrality_boost: 1.0,
                category_ids: vec![420],
                bucket: MemoryBucket::Relationships,
                char_estimate: 30,
            }],
            preferences: vec![],
            upcoming: vec![],
            general: vec![],
            total_score: 2.0,
            char_count: 70,
        };
        let rendered = render_memory_schema(&schema);
        assert!(rendered.contains("Devansh works as software developer"));
        assert!(rendered.contains("Relationships: Devansh is partnered with Alice"));
    }

    #[test]
    fn render_unknown_relationship() {
        let fact = RankedFact {
            fact_id: 1,
            subject_name: "Devansh".to_string(),
            relationship_type: "loves_eating".to_string(),
            object_display: "sushi".to_string(),
            confidence: 0.5,
            score: 1.0,
            temporal_boost: 1.0,
            memory_weight: 1.0,
            priority_boost: 1.0,
            centrality_boost: 1.0,
            category_ids: vec![300],
            bucket: MemoryBucket::Preferences,
            char_estimate: 30,
        };
        let line = render_fact_line(&fact);
        assert_eq!(line, "Devansh loves eating sushi");
    }

    #[test]
    fn estimate_chars_basic() {
        assert_eq!(estimate_chars("Alice", "has_partner", "Bob"), 22);
    }
}
