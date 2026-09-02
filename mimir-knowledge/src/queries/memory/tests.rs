//! Memory ranking/rendering tests.

use super::ranking::{bucket_from_id, compute_temporal_boost, estimate_chars};
use super::render::{format_upcoming_line, render_fact_line};
use super::*;
use crate::models::memory::{MemoryBucket, MemoryPriority, MemorySchema, RankedFact};
use chrono::TimeZone;

#[test]
fn upcoming_suffix_uses_calendar_days() {
    use chrono::TimeZone;

    // 23:00 today -> 01:00 tomorrow is only 2 hours, but it crosses a
    // calendar boundary so the suffix must say "in 1 day", not "today".
    let now = Utc.with_ymd_and_hms(2026, 6, 25, 23, 0, 0).unwrap();
    let when = Utc.with_ymd_and_hms(2026, 6, 26, 1, 0, 0).unwrap();
    let line = format_upcoming_line("Ada", "is_in", Some("Paris"), None, when, now);
    assert!(line.contains("in 1 day"), "line was: {line}");
}

#[test]
fn upcoming_suffix_today_same_calendar_day() {
    use chrono::TimeZone;

    // Same calendar day, ~1h apart -> "today" regardless of hour delta.
    let now = Utc.with_ymd_and_hms(2026, 6, 25, 10, 0, 0).unwrap();
    let when = Utc.with_ymd_and_hms(2026, 6, 25, 23, 0, 0).unwrap();
    let line = format_upcoming_line("Ada", "is_in", Some("Paris"), None, when, now);
    assert!(line.contains("today"), "line was: {line}");
}

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
fn bucket_from_id_maps_every_seeded_bucket() {
    assert_eq!(
        bucket_from_id(Some(MemoryBucket::Identity as i16)),
        MemoryBucket::Identity
    );
    assert_eq!(
        bucket_from_id(Some(MemoryBucket::Upcoming as i16)),
        MemoryBucket::Upcoming
    );
    assert_eq!(
        bucket_from_id(Some(MemoryBucket::Relationships as i16)),
        MemoryBucket::Relationships
    );
    assert_eq!(
        bucket_from_id(Some(MemoryBucket::Preferences as i16)),
        MemoryBucket::Preferences
    );
    assert_eq!(
        bucket_from_id(Some(MemoryBucket::General as i16)),
        MemoryBucket::General
    );
}

#[test]
fn bucket_from_id_falls_back_to_general() {
    assert_eq!(bucket_from_id(None), MemoryBucket::General);
    assert_eq!(bucket_from_id(Some(0)), MemoryBucket::General);
    assert_eq!(bucket_from_id(Some(99)), MemoryBucket::General);
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
fn now_line_uses_utc_and_plain_english_date() {
    let now = chrono::Utc
        .with_ymd_and_hms(2026, 8, 23, 21, 30, 0)
        .unwrap();
    assert_eq!(
        super::render::now_line(now),
        "Now: 2026-08-23T21:30:00Z (Sunday 23 August 2026)"
    );
}

#[test]
fn prepend_now_line_adds_exactly_one_leading_stamp() {
    let now = chrono::Utc
        .with_ymd_and_hms(2026, 8, 23, 21, 30, 0)
        .unwrap();
    let expected = "Now: 2026-08-23T21:30:00Z (Sunday 23 August 2026)\n\nMemory facts.";

    assert_eq!(
        super::render::prepend_now_line("Memory facts.", now),
        expected
    );
    assert_eq!(super::render::prepend_now_line(expected, now), expected);
}

#[test]
fn refresh_now_line_replaces_existing_stamp_without_duplicating_it() {
    let first = chrono::Utc
        .with_ymd_and_hms(2026, 8, 23, 21, 30, 0)
        .unwrap();
    let second = chrono::Utc.with_ymd_and_hms(2026, 8, 24, 6, 15, 0).unwrap();
    let original = super::render::prepend_now_line("Memory facts.", first);
    let expected = "Now: 2026-08-24T06:15:00Z (Monday 24 August 2026)\n\nMemory facts.";

    assert_eq!(super::render::refresh_now_line(&original, second), expected);
}

#[test]
fn refresh_now_line_replaces_inline_stamp_without_eating_memory() {
    let second = chrono::Utc.with_ymd_and_hms(2026, 8, 24, 6, 15, 0).unwrap();
    let original =
        "Core facts:Now: 2026-08-23T21:30:00Z (Sunday 23 August 2026)User enjoys hiking.";

    assert_eq!(
        super::render::refresh_now_line(original, second),
        "Core facts:Now: 2026-08-24T06:15:00Z (Monday 24 August 2026)User enjoys hiking."
    );
}

#[test]
fn render_connector_predicates_with_complete_grammar() {
    fn render(subject_name: &str, relationship_type: &str, object_display: &str) -> String {
        render_fact_line(&RankedFact {
            fact_id: 1,
            subject_name: subject_name.to_string(),
            relationship_type: relationship_type.to_string(),
            object_display: object_display.to_string(),
            confidence: 0.5,
            score: 1.0,
            temporal_boost: 1.0,
            memory_weight: 1.0,
            priority_boost: 1.0,
            centrality_boost: 1.0,
            category_ids: vec![300],
            bucket: MemoryBucket::Preferences,
            char_estimate: 30,
        })
    }

    let cases = [
        (
            "has_event",
            "the Berlin conference",
            "Devansh has an event the Berlin conference",
        ),
        (
            "attending",
            "the Berlin conference",
            "Devansh is attending the Berlin conference",
        ),
        (
            "took_photo_at",
            "the Matterhorn",
            "Devansh took a photo at the Matterhorn",
        ),
        (
            "took_photo",
            "the Matterhorn",
            "Devansh took a photo of the Matterhorn",
        ),
        ("has_flight", "BA123", "Devansh has a flight BA123"),
        ("departs_from", "Geneva", "Devansh departs from Geneva"),
        ("arrives_at", "Zurich", "Devansh arrives at Zurich"),
        ("operated_by", "SWISS", "Devansh is operated by SWISS"),
        (
            "has_booking",
            "hotel room 12",
            "Devansh has a booking hotel room 12",
        ),
        (
            "has_order",
            "package 4521",
            "Devansh has an order package 4521",
        ),
        ("purchased_from", "Shop", "Devansh was purchased from Shop"),
        (
            "has_delivery",
            "parcel 88",
            "Devansh has a delivery parcel 88",
        ),
        ("shipped_by", "DHL", "Devansh was shipped by DHL"),
        ("delivered_to", "Geneva", "Devansh was delivered to Geneva"),
        ("has_ticket", "seat 14A", "Devansh has a ticket seat 14A"),
        (
            "issued_by",
            "UK Passport Office",
            "Devansh was issued by UK Passport Office",
        ),
    ];
    for (relationship_type, object_display, expected) in cases {
        assert_eq!(
            render("Devansh", relationship_type, object_display),
            expected
        );
    }
}

#[test]
fn estimate_chars_basic() {
    assert_eq!(estimate_chars("Alice", "has_partner", "Bob"), 22);
}
