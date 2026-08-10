//! Fact ranking helpers: temporal boost, bucket classification, and
//! budget-aware truncation.

use chrono::{DateTime, Utc};

use crate::models::memory::{MemoryBucket, RankedFact};
use crate::queries::memory::{
    IDENTITY_CATEGORY_RANGE, PREFERENCE_CATEGORY_EXTRAS, PREFERENCE_CATEGORY_RANGE,
    RELATIONSHIP_CATEGORY_RANGE, UPCOMING_CATEGORY_RANGE,
};

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
        if IDENTITY_CATEGORY_RANGE.contains(&id) {
            has_identity = true;
        } else if UPCOMING_CATEGORY_RANGE.contains(&id) {
            has_upcoming = true;
        } else if RELATIONSHIP_CATEGORY_RANGE.contains(&id) {
            has_relationships = true;
        } else if PREFERENCE_CATEGORY_RANGE.contains(&id)
            || PREFERENCE_CATEGORY_EXTRAS.contains(&id)
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
pub(super) fn estimate_chars(subject: &str, relationship: &str, object: &str) -> usize {
    // Template: "{subject} {relationship} {object}. "
    subject.len() + relationship.len() + object.len() + 3
}

/// Truncate a fact to fit the remaining budget, appending `…`.
/// Uses char-aware slicing to avoid panicking on multi-byte UTF-8 characters.
pub(super) fn truncate_fact(mut fact: RankedFact, budget: usize) -> RankedFact {
    let max_obj = budget.saturating_sub(fact.subject_name.len() + fact.relationship_type.len() + 3);
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
