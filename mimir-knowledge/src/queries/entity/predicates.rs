//! Relationship-type constraint validation (subject/object entity types).
//!
//! Migration 013 seeded a permissive allow-list of (subject, object) entity-type
//! pairs per predicate ("strict enforcement in app code"), renamed to
//! `relationship_constraints` by migration 031. Enforcement lives here and is
//! wired into every insert path (`insert_fact_in_tx`,
//! `insert_sensitive_fact`) so a fact can never bypass it (issue #402).

use sqlx::SqlitePool;
use sqlx::Transaction;

use crate::KnowledgeError;
use crate::models::entity::EntityType;

/// The allowed (subject, object) entity-type pairs seeded for a relationship
/// type.
///
/// `None` means the predicate is unconstrained — the permissive default for
/// predicates without seeded rows (auto-created and connector-emitted types,
/// tracked by the ontology consolidation, issues #403/#412).
async fn allowed_combos<'a, E>(
    executor: E,
    relationship_type_id: i16,
) -> Result<Option<Vec<(i16, i16)>>, sqlx::Error>
where
    E: sqlx::Executor<'a, Database = sqlx::Sqlite>,
{
    let rows: Vec<(i16, i16)> = sqlx::query_as(
        "SELECT allowed_subject_type_id, allowed_object_type_id \
         FROM relationship_constraints WHERE relationship_type_id = ?",
    )
    .bind(relationship_type_id)
    .fetch_all(executor)
    .await?;
    if rows.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rows))
    }
}

fn combo_is_allowed(combos: &[(i16, i16)], subject_type: i16, object_type: i16) -> bool {
    combos.contains(&(subject_type, object_type))
}

/// Validate a relationship type against pre-resolved subject/object entity
/// types.
///
/// Predicates without seeded constraints accept any combination; predicates
/// with seeded constraints require the exact pair.
pub async fn validate_predicate(
    pool: &SqlitePool,
    subject_type: EntityType,
    relationship_type_id: i16,
    object_type: EntityType,
) -> Result<(), KnowledgeError> {
    let Some(combos) = allowed_combos(pool, relationship_type_id).await? else {
        return Ok(());
    };
    if combo_is_allowed(&combos, subject_type as i16, object_type as i16) {
        Ok(())
    } else {
        Err(KnowledgeError::InvalidRelationshipConstraint(
            relationship_type_id,
        ))
    }
}

/// Insert-path validation: look up the stored subject/object entity types and
/// enforce the seeded constraints for entity-object facts.
///
/// Literal-object facts carry no object type and always pass. Called from the
/// shared insert paths (`insert_fact_in_tx`, `insert_sensitive_fact`) so every
/// fact — conversational, connector, inference, batch — is checked exactly
/// once at the write boundary (issue #402).
pub(crate) async fn validate_predicate_in_tx(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    relationship_type_id: i16,
    subject_id: i32,
    object_id: Option<i32>,
) -> Result<(), KnowledgeError> {
    let Some(object_id) = object_id else {
        return Ok(());
    };
    let Some(combos) = allowed_combos(&mut **tx, relationship_type_id).await? else {
        return Ok(());
    };
    let subject_type: i16 = sqlx::query_scalar("SELECT entity_type_id FROM entities WHERE id = ?")
        .bind(subject_id)
        .fetch_one(&mut **tx)
        .await?;
    let object_type: i16 = sqlx::query_scalar("SELECT entity_type_id FROM entities WHERE id = ?")
        .bind(object_id)
        .fetch_one(&mut **tx)
        .await?;
    if combo_is_allowed(&combos, subject_type, object_type) {
        Ok(())
    } else {
        Err(KnowledgeError::InvalidRelationshipConstraint(
            relationship_type_id,
        ))
    }
}
