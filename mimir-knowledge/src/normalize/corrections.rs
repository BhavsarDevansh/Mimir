//! Conversational corrections: retrospective and temporal correction scopes.

use chrono::{DateTime, Utc};

use crate::models::audit_log::ChangedBy;
use crate::models::fact::{Fact, FactStatus, NewFact};
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};
// ---------------------------------------------------------------------------

/// Apply a conversational correction to the in-flight `NewFact`.
///
/// `Some("always")` — retrospective correction: the old fact was never true, so
/// every overlapping active fact is marked `Corrected`, moved to trash, and its
/// orphaned children re-evaluated. Any other string is parsed as a datetime and
/// used as the new `valid_from` (the existing insert temporal-overlap logic
/// closes the sole open-ended predecessor automatically). `None` (reached from
/// the chat adapter when the LLM emitted `Correction` with no scope) defaults to
/// a temporal correction at `now`. The handler is only called when the
/// producer flagged the fact as a correction (`is_correction`), so the `None`
/// arm is reachable and exercised by chat corrections with no scope.
pub(super) async fn handle_correction(
    kg: &KnowledgeGraph,
    scope: Option<&str>,
    subject_id: i32,
    relationship_type_id: i16,
    new_fact: &mut NewFact,
    now: DateTime<Utc>,
) -> Result<(), KnowledgeError> {
    match scope {
        Some("always") => {
            let overlapping = find_active_overlapping(
                kg,
                subject_id,
                relationship_type_id,
                new_fact.valid_from,
                new_fact.valid_until,
            )
            .await?;

            let mut tx = kg.pool().begin().await?;
            let mut all_children: Vec<(i32, bool)> = Vec::new();

            for old in overlapping {
                queries::fact::set_status_tx(
                    &mut tx,
                    old.id,
                    FactStatus::Corrected,
                    now,
                    ChangedBy::System,
                )
                .await?;

                let children =
                    crate::forget::forget_fact_tx(&mut tx, old.id, ChangedBy::System, now).await?;
                all_children.extend(children);
            }

            tx.commit().await?;

            let mut seen = std::collections::HashSet::new();
            all_children.retain(|(id, _)| seen.insert(*id));

            crate::forget::evaluate_children(kg.pool(), all_children, now).await?;
        }
        Some(datetime_str) => match DateTime::parse_from_rfc3339(datetime_str) {
            Ok(dt) => {
                new_fact.valid_from = Some(dt.with_timezone::<Utc>(&Utc));
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to parse correction_scope datetime '{}': {}. valid_from will not be set.",
                    datetime_str,
                    e
                );
            }
        },
        None => {
            new_fact.valid_from = Some(now);
        }
    }

    Ok(())
}

/// Find active facts with the same subject + relationship_type that overlap.
pub(super) async fn find_active_overlapping(
    kg: &KnowledgeGraph,
    subject_id: i32,
    relationship_type_id: i16,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
) -> Result<Vec<Fact>, KnowledgeError> {
    let rows: Vec<Fact> = sqlx::query_as::<_, Fact>(
        "SELECT id, subject_id, relationship_type_id, object_id, object_literal, \
         valid_from, valid_until, confidence, fact_status_id, inferred, \
         inference_depth, stale_confidence, pending_confirmation, memory_priority_id, created_at, updated_at \
         FROM facts \
         WHERE subject_id = ? AND relationship_type_id = ? AND fact_status_id = ?",
    )
    .bind(subject_id)
    .bind(relationship_type_id)
    .bind(FactStatus::Active as i16)
    .fetch_all(kg.pool())
    .await?;

    Ok(rows
        .into_iter()
        .filter(|f| {
            queries::fact::ranges_overlap(f.valid_from, f.valid_until, valid_from, valid_until)
        })
        .collect())
}

// ---------------------------------------------------------------------------
