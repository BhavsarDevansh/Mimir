//! Predicate-string validation.

use sqlx::SqlitePool;

use crate::KnowledgeError;
use crate::models::entity::EntityType;

pub async fn validate_predicate(
    pool: &SqlitePool,
    subject_type: EntityType,
    relationship_type_id: i16,
    object_type: EntityType,
) -> Result<(), KnowledgeError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM relationship_constraints \
         WHERE relationship_type_id = ? AND allowed_subject_type_id = ? AND allowed_object_type_id = ? \
         LIMIT 1",
    )
    .bind(relationship_type_id)
    .bind(subject_type as i16)
    .bind(object_type as i16)
    .fetch_optional(pool)
    .await?;

    if row.is_none() {
        return Err(KnowledgeError::InvalidRelationshipType(
            relationship_type_id,
        ));
    }
    Ok(())
}
