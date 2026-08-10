use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Predicate registry
    // ------------------------------------------------------------------

    /// Look up a relationship type by name without creating it.
    /// Returns `None` if the type does not exist.
    pub async fn relationship_type_id(&self, name: &str) -> Option<i16> {
        match self.get_relationship_type_id(name).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("relationship_type_id lookup failed for '{}': {}", name, e);
                None
            }
        }
    }

    /// Ensure a relationship type exists in the database, returning its stable id.
    /// Creates the row silently if missing.
    ///
    /// Resolution order:
    /// 1. Normalize the incoming name.
    /// 2. Query `relationship_type_aliases` for the normalized name; return the
    ///    canonical id on hit.
    /// 3. Fall back to creating a new canonical type and register the normalized
    ///    name as its own alias.
    pub async fn ensure_relationship_type(&self, name: &str) -> Result<i16, KnowledgeError> {
        let mut tx = self.pool.begin().await?;
        let id = self.ensure_relationship_type_in_tx(&mut tx, name).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Same as [`Self::ensure_relationship_type`] but operates inside an existing transaction.
    pub(crate) async fn ensure_relationship_type_in_tx(
        &self,
        tx: &mut sqlx::SqliteTransaction<'_>,
        name: &str,
    ) -> Result<i16, KnowledgeError> {
        let Some(normalized) = normalize_alias(name) else {
            return Err(KnowledgeError::Validation(
                "relationship type name cannot be empty".to_string(),
            ));
        };

        // 1. In-memory cache.
        {
            let cache = self.relationship_type_cache.read().await;
            if let Some(&id) = cache.alias_to_id.get(&normalized) {
                return Ok(id);
            }
        }

        // 2. Alias table is the single source of truth.
        let row: Option<(i16,)> = sqlx::query_as(
            "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
        )
        .bind(&normalized)
        .fetch_optional(&mut **tx)
        .await?;

        if let Some((id,)) = row {
            let mut cache = self.relationship_type_cache.write().await;
            cache.alias_to_id.insert(normalized.clone(), id);
            cache.name_to_id.insert(normalized, id);
            return Ok(id);
        }

        // 3. Alias miss: create new canonical type, then register self-alias.
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO relationship_types (name, description) VALUES (?, ?) ON CONFLICT (name) DO UPDATE SET name = relationship_types.name RETURNING id",
        )
        .bind(&normalized)
        .bind(format!("Auto-created relationship_type: {}", normalized))
        .fetch_one(&mut **tx)
        .await?;
        let id = id as i16;

        // Use INSERT OR IGNORE because concurrent transactions may race to create
        // the same new canonical type; both can upsert `relationship_types`, but
        // only one can insert the self-alias. The loser must commit cleanly.
        sqlx::query(
            "INSERT OR IGNORE INTO relationship_type_aliases (alias, relationship_type_id) VALUES (?, ?)",
        )
        .bind(&normalized)
        .bind(id)
        .execute(&mut **tx)
        .await?;

        let mut cache = self.relationship_type_cache.write().await;
        cache.name_to_id.insert(normalized.clone(), id);
        cache.alias_to_id.insert(normalized, id);
        Ok(id)
    }

    /// Look up a relationship type id by name without creating it.
    ///
    /// The alias table is the single source of truth: aliases resolve to their
    /// canonical relationship type id, and every canonical name is also a
    /// self-alias.
    pub async fn get_relationship_type_id(
        &self,
        name: &str,
    ) -> Result<Option<i16>, KnowledgeError> {
        let Some(normalized) = normalize_alias(name) else {
            return Ok(None);
        };

        {
            let cache = self.relationship_type_cache.read().await;
            if let Some(&id) = cache.alias_to_id.get(&normalized) {
                return Ok(Some(id));
            }
        }

        let row: Option<(i16,)> = sqlx::query_as(
            "SELECT relationship_type_id FROM relationship_type_aliases WHERE alias = ?",
        )
        .bind(&normalized)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id,)) = row {
            let mut cache = self.relationship_type_cache.write().await;
            cache.alias_to_id.insert(normalized, id);
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Reverse lookup: get the relationship_type name for a given id.
    pub async fn relationship_type_name(&self, id: i16) -> Option<String> {
        {
            let cache = self.relationship_type_cache.read().await;
            if let Some(name) = cache.id_to_name.get(&id) {
                return Some(name.clone());
            }
        }

        let row: Option<(String,)> =
            match sqlx::query_as("SELECT name FROM relationship_types WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("relationship_type_name lookup failed for id {}: {}", id, e);
                    return None;
                }
            };

        if let Some((ref name,)) = row {
            let mut cache = self.relationship_type_cache.write().await;
            cache.name_to_id.insert(name.clone(), id);
            cache.id_to_name.insert(id, name.clone());
        }

        row.map(|r| r.0)
    }
}
