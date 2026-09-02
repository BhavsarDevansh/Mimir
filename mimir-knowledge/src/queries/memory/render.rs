//! Deterministic text rendering of a memory schema and the upcoming section.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::fact::FactStatus;
use crate::models::memory::{MemorySchema, RankedFact};

/// Render the request-local temporal anchor for the LLM-facing memory block.
pub(super) fn now_line(now: DateTime<Utc>) -> String {
    format!(
        "Now: {} ({})",
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        now.format("%A %-d %B %Y")
    )
}

/// Ensure a prompt starts with the current temporal anchor, replacing only a
/// previously-rendered leading anchor and preserving all other content.
pub fn refresh_now_line(memory: &str, now: DateTime<Utc>) -> String {
    let stamp = now_line(now);
    match now_anchor_range(memory) {
        Some((start, end)) => format!("{}{}{}", &memory[..start], stamp, &memory[end..]),
        None if memory.is_empty() => stamp,
        None => format!("{}\n\n{}", stamp, memory),
    }
}

fn now_anchor_range(memory: &str) -> Option<(usize, usize)> {
    if !memory.starts_with("Now: ") {
        return None;
    }

    let end = memory.find('\n').map_or(memory.len(), |offset| offset);
    let content = memory[..end].strip_prefix("Now: ")?;
    let (timestamp, prose) = content.split_once(" (")?;
    let timestamp = DateTime::parse_from_rfc3339(timestamp).ok()?;
    let prose = prose.strip_suffix(')')?;

    let expected_prose = timestamp
        .with_timezone(&Utc)
        .format("%A %-d %B %Y")
        .to_string();

    (prose == expected_prose).then_some((0, end))
}

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

pub(super) fn render_fact_line(fact: &RankedFact) -> String {
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
        "located_in" => format!("{} is in {}", fact.subject_name, fact.object_display),
        "resides_in" => format!("{} resides in {}", fact.subject_name, fact.object_display),
        "owns" => format!("{} owns {}", fact.subject_name, fact.object_display),
        "visited" => format!("{} visited {}", fact.subject_name, fact.object_display),
        "has_event" => format!("{} has an event {}", fact.subject_name, fact.object_display),
        "attending" => format!("{} is attending {}", fact.subject_name, fact.object_display),
        "took_photo_at" => format!(
            "{} took a photo at {}",
            fact.subject_name, fact.object_display
        ),
        "took_photo" => format!(
            "{} took a photo of {}",
            fact.subject_name, fact.object_display
        ),
        "has_flight" => format!("{} has a flight {}", fact.subject_name, fact.object_display),
        "departs_from" => format!("{} departs from {}", fact.subject_name, fact.object_display),
        "arrives_at" => format!("{} arrives at {}", fact.subject_name, fact.object_display),
        "operated_by" => format!(
            "{} is operated by {}",
            fact.subject_name, fact.object_display
        ),
        "has_booking" => format!(
            "{} has a booking {}",
            fact.subject_name, fact.object_display
        ),
        "has_order" => format!("{} has an order {}", fact.subject_name, fact.object_display),
        "purchased_from" => format!(
            "{} was purchased from {}",
            fact.subject_name, fact.object_display
        ),
        "has_delivery" => format!(
            "{} has a delivery {}",
            fact.subject_name, fact.object_display
        ),
        "shipped_by" => format!(
            "{} was shipped by {}",
            fact.subject_name, fact.object_display
        ),
        "delivered_to" => format!(
            "{} was delivered to {}",
            fact.subject_name, fact.object_display
        ),
        "has_ticket" => format!("{} has a ticket {}", fact.subject_name, fact.object_display),
        "issued_by" => format!(
            "{} was issued by {}",
            fact.subject_name, fact.object_display
        ),
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
// Upcoming section (fresh per request, no LLM)
// ---------------------------------------------------------------------------

/// Render upcoming events for the given subject entity.
///
/// Combines two event sources (issue #74):
/// 1. **One-time** — facts with a future `valid_from` whose event overlay (if
///    any) is not `Completed`/`Dismissed`.
/// 2. **Recurring** — active recurring event overlays whose (advanced)
///    `trigger_date` falls within the horizon.
///
/// Sorted by occurrence, capped at `limit`. Fresh per request; no LLM.
pub async fn render_upcoming_section(
    pool: &SqlitePool,
    subject_id: i32,
    now: DateTime<Utc>,
    days_ahead: i64,
    limit: usize,
) -> Result<String, KnowledgeError> {
    use crate::models::enums::{EventStatus, RecurrenceType};

    let horizon = now + chrono::Duration::days(days_ahead);
    let mut items: Vec<(DateTime<Utc>, String)> = Vec::new();

    // 1. One-time future-dated facts, suppressed only by a terminal overlay.
    #[derive(sqlx::FromRow)]
    struct UpcomingFactRow {
        subject_name: String,
        relationship_type: String,
        object_name: Option<String>,
        object_literal: Option<String>,
        valid_from: Option<DateTime<Utc>>,
    }

    let one_time: Vec<UpcomingFactRow> = sqlx::query_as(
        "SELECT \
            s.name AS subject_name, \
            rt.name AS relationship_type, \
            COALESCE(o.name, f.object_literal) AS object_name, \
            f.object_literal, \
            f.valid_from \
         FROM facts f \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         LEFT JOIN events e ON e.fact_id = f.id \
         WHERE f.subject_id = ? \
           AND f.pending_confirmation = 0 \
           AND f.fact_status_id NOT IN (?, ?) \
           AND f.confidence >= 0.5 \
           AND f.valid_from IS NOT NULL \
           AND f.valid_from >= ? \
           AND f.valid_from <= ? \
           AND (e.id IS NULL OR (e.status_id IN (?, ?, ?) AND e.recurrence_type_id = ?)) \
         ORDER BY f.valid_from \
         LIMIT ?",
    )
    .bind(subject_id)
    .bind(FactStatus::Superseded as i16)
    .bind(FactStatus::Forgotten as i16)
    .bind(now)
    .bind(horizon)
    .bind(EventStatus::Pending as i16)
    .bind(EventStatus::Active as i16)
    .bind(EventStatus::Snoozed as i16)
    .bind(RecurrenceType::None as i16)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    for row in one_time {
        if let Some(vf) = row.valid_from {
            items.push((
                vf,
                format_upcoming_line(
                    &row.subject_name,
                    &row.relationship_type,
                    row.object_name.as_deref(),
                    row.object_literal.as_deref(),
                    vf,
                    now,
                ),
            ));
        }
    }

    // 2. Recurring active events within the horizon.
    #[derive(sqlx::FromRow)]
    struct RecurringEventRow {
        subject_name: String,
        relationship_type: String,
        object_name: Option<String>,
        object_literal: Option<String>,
        trigger_date: DateTime<Utc>,
    }

    let recurring: Vec<RecurringEventRow> = sqlx::query_as(
        "SELECT \
            s.name AS subject_name, \
            rt.name AS relationship_type, \
            COALESCE(o.name, f.object_literal) AS object_name, \
            f.object_literal, \
            e.trigger_date \
         FROM events e \
         JOIN facts f ON f.id = e.fact_id \
         JOIN entities s ON s.id = f.subject_id \
         JOIN relationship_types rt ON rt.id = f.relationship_type_id \
         LEFT JOIN entities o ON o.id = f.object_id \
         WHERE e.entity_id = ? \
           AND e.status_id = ? \
           AND e.recurrence_type_id != ? \
           AND e.trigger_date >= ? \
           AND e.trigger_date <= ? \
           AND f.fact_status_id NOT IN (?, ?) \
         ORDER BY e.trigger_date \
         LIMIT ?",
    )
    .bind(subject_id)
    .bind(EventStatus::Active as i16)
    .bind(RecurrenceType::None as i16)
    .bind(now)
    .bind(horizon)
    .bind(FactStatus::Superseded as i16)
    .bind(FactStatus::Forgotten as i16)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    for row in recurring {
        items.push((
            row.trigger_date,
            format_upcoming_line(
                &row.subject_name,
                &row.relationship_type,
                row.object_name.as_deref(),
                row.object_literal.as_deref(),
                row.trigger_date,
                now,
            ),
        ));
    }

    items.sort_by_key(|a| a.0);
    items.truncate(limit);

    if items.is_empty() {
        Ok(String::new())
    } else {
        let lines: Vec<String> = items.into_iter().map(|(_, line)| line).collect();
        Ok(format!("Upcoming:\n{}\n", lines.join("\n")))
    }
}

/// Format a single upcoming line: `- subject predicate object (DD Month, in N days)`.
pub(super) fn format_upcoming_line(
    subject_name: &str,
    relationship_type: &str,
    object_name: Option<&str>,
    object_literal: Option<&str>,
    when: DateTime<Utc>,
    now: DateTime<Utc>,
) -> String {
    let object_display = object_name
        .unwrap_or_else(|| object_literal.unwrap_or(""))
        .to_string();
    let rel = relationship_type.replace('_', " ");
    let days = when
        .date_naive()
        .signed_duration_since(now.date_naive())
        .num_days();
    let when_str = if days == 0 {
        "today".to_string()
    } else if days == 1 {
        "in 1 day".to_string()
    } else {
        format!("in {} days", days)
    };
    format!(
        "- {} {} {} ({}, {})",
        subject_name,
        rel,
        object_display,
        when.format("%d %B"),
        when_str
    )
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
