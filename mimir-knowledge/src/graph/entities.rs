use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Entity CRUD delegates
    // ------------------------------------------------------------------

    /// Create an entity (returns existing on exact duplicate).
    pub async fn create_entity(
        &self,
        name: &str,
        entity_type: models::entity::EntityType,
        aliases: &[&str],
    ) -> Result<models::entity::Entity, KnowledgeError> {
        queries::entity::create_entity(&self.pool, name, entity_type, aliases).await
    }

    /// Get entity by ID.
    pub async fn get_entity(
        &self,
        id: i32,
    ) -> Result<Option<models::entity::Entity>, KnowledgeError> {
        queries::entity::get_by_id(&self.pool, id).await
    }

    /// Update entity name and type.
    pub async fn update_entity(
        &self,
        id: i32,
        name: &str,
        entity_type: models::entity::EntityType,
    ) -> Result<models::entity::Entity, KnowledgeError> {
        queries::entity::update_entity(&self.pool, id, name, entity_type as i16).await
    }

    /// Delete entity (rejected if referenced by facts).
    pub async fn delete_entity(&self, id: i32) -> Result<(), KnowledgeError> {
        queries::entity::delete_entity(&self.pool, id).await
    }

    /// Count facts that reference an entity (as subject or object).
    ///
    /// Count every fact that references the entity, regardless of
    /// `fact_status_id`.  We intentionally include revoked or deleted facts
    /// because any non-zero reference history indicates meaningful entity
    /// usage; the auto-merge gate in `seed_identity_facts` therefore treats a
    /// very low count (e.g. <= 2) as a signal of an accidental duplicate.
    ///
    /// Uses a `UNION` query so that separate indexes on `subject_id` and
    /// `object_id` can both be exploited.
    pub async fn count_entity_facts(&self, id: i32) -> Result<i64, KnowledgeError> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM (
                SELECT id FROM facts WHERE subject_id = ?
                UNION
                SELECT id FROM facts WHERE object_id = ?
            )",
        )
        .bind(id)
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    /// Search entities by name/alias.
    pub async fn search_entities(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<queries::entity::AliasSearchResult>, KnowledgeError> {
        queries::entity::search(&self.pool, query, limit).await
    }

    /// Add an alias to an entity.
    pub async fn add_alias(&self, entity_id: i32, alias: &str) -> Result<(), KnowledgeError> {
        queries::entity::add_alias(&self.pool, entity_id, alias).await
    }

    /// Remove an alias from an entity.
    pub async fn remove_alias(&self, entity_id: i32, alias: &str) -> Result<(), KnowledgeError> {
        queries::entity::remove_alias(&self.pool, entity_id, alias).await
    }
}
