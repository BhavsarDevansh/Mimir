//! Fact ranking helpers: temporal boost, bucket classification, and
//! budget-aware truncation.

use chrono::{DateTime, Utc};

use crate::models::memory::{MemoryBucket, RankedFact};
use crate::queries::memory::render::temporal_bounds_len;

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

/// Resolve a stored bucket id (`categories.memory_bucket_id`) to a bucket.
/// Unknown ids and unset buckets classify as `General`.
pub fn bucket_from_id(bucket_id: Option<i16>) -> MemoryBucket {
    bucket_id
        .and_then(|id| MemoryBucket::try_from(id).ok())
        .unwrap_or(MemoryBucket::General)
}

/// Rough character estimate for a rendered fact.
pub(super) fn estimate_chars(
    subject: &str,
    relationship: &str,
    object: &str,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> usize {
    subject.len()
        + relationship.len()
        + object.len()
        + 3
        + temporal_bounds_len(valid_from, valid_until)
}

/// Truncate a fact to fit the remaining budget, appending `…`.
/// Uses char-aware slicing to avoid panicking on multi-byte UTF-8 characters.
pub(super) fn truncate_fact(mut fact: RankedFact, budget: usize) -> RankedFact {
    let bounds_len = temporal_bounds_len(fact.valid_from, fact.valid_until);
    let max_obj = budget
        .saturating_sub(fact.subject_name.len() + fact.relationship_type.len() + 3 + bounds_len);
    if max_obj == 0 {
        fact.object_display = "…".to_string();
    } else if fact.object_display.len() > max_obj {
        let limit = max_obj.saturating_sub(1);
        let mut truncated = String::with_capacity(limit);
        for ch in fact.object_display.chars() {
            if truncated.len() + ch.len_utf8() > limit {
                break;
            }
            truncated.push(ch);
        }
        fact.object_display = format!("{}…", truncated);
    }
    fact.char_estimate = budget;
    fact
}

// ---------------------------------------------------------------------------
// Deterministic fallback renderer
// ---------------------------------------------------------------------------
