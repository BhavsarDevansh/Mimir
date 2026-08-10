use crate::graph::KnowledgeGraph;
use crate::*;

use std::sync::Arc;

use tokio::sync::Notify;

use super::{alias_conflicts_with_canonical_name, canonical_name_conflicts_with_alias};

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Relationship type DAG
    // ------------------------------------------------------------------

    /// Add a parent edge to the relationship type hierarchy.
    /// Rejects self-loops and any cycle that would be created.
    pub async fn insert_relationship_type_hierarchy(
        &self,
        child_id: i16,
        parent_id: i16,
    ) -> Result<(), KnowledgeError> {
        if child_id == parent_id {
            return Err(KnowledgeError::RelationshipTypeCycle);
        }

        let mut tx = self.pool.begin().await?;

        if Self::relationship_type_reaches(&mut tx, parent_id, child_id).await? {
            return Err(KnowledgeError::RelationshipTypeCycle);
        }

        sqlx::query("INSERT INTO relationship_type_hierarchy (child_id, parent_id) VALUES (?, ?)")
            .bind(child_id)
            .bind(parent_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Add an English alias for a relationship type.
    ///
    /// Aliases are globally unique and must not shadow an existing canonical
    /// relationship type name.
    pub async fn insert_relationship_type_alias(
        &self,
        alias: &str,
        relationship_type_id: i16,
    ) -> Result<(), KnowledgeError> {
        let Some(normalized) = normalize_alias(alias) else {
            return Err(KnowledgeError::Validation(
                "alias cannot be empty".to_string(),
            ));
        };

        let mut tx = self.pool.begin().await?;

        if alias_conflicts_with_canonical_name(&mut *tx, alias).await? {
            return Err(KnowledgeError::Validation(format!(
                "alias '{}' conflicts with an existing relationship type name",
                normalized
            )));
        }

        sqlx::query(
            "INSERT INTO relationship_type_aliases (alias, relationship_type_id) VALUES (?, ?)",
        )
        .bind(&normalized)
        .bind(relationship_type_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let mut cache = self.relationship_type_cache.write().await;
        cache.alias_to_id.insert(normalized, relationship_type_id);
        Ok(())
    }

    /// Resolve an alias to a relationship type id.
    pub async fn resolve_relationship_type_alias(
        &self,
        alias: &str,
    ) -> Result<Option<i16>, KnowledgeError> {
        let Some(normalized) = normalize_alias(alias) else {
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

    /// Return all descendant ids of the given relationship type (recursive).
    pub async fn get_descendant_relationship_type_ids(
        &self,
        ancestor_id: i16,
    ) -> Result<Vec<i16>, KnowledgeError> {
        let rows: Vec<(i16,)> = sqlx::query_as(
            r#"WITH RECURSIVE descendants(id) AS (
             SELECT child_id FROM relationship_type_hierarchy WHERE parent_id = ?
             UNION
             SELECT h.child_id FROM relationship_type_hierarchy h
             JOIN descendants d ON h.parent_id = d.id
             )
             SELECT id FROM descendants"#,
        )
        .bind(ancestor_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Return all ancestor ids of the given relationship type (recursive).
    pub async fn get_ancestor_relationship_type_ids(
        &self,
        descendant_id: i16,
    ) -> Result<Vec<i16>, KnowledgeError> {
        let rows: Vec<(i16,)> = sqlx::query_as(
            r#"WITH RECURSIVE ancestors(id) AS (
             SELECT parent_id FROM relationship_type_hierarchy WHERE child_id = ?
             UNION
             SELECT h.parent_id FROM relationship_type_hierarchy h
             JOIN ancestors a ON h.child_id = a.id
             )
             SELECT id FROM ancestors"#,
        )
        .bind(descendant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Return true if `source_id` can reach `target_id` through parent edges.
    async fn relationship_type_reaches(
        tx: &mut sqlx::SqliteTransaction<'_>,
        source_id: i16,
        target_id: i16,
    ) -> Result<bool, KnowledgeError> {
        let rows: Vec<(i16,)> = sqlx::query_as(
            r#"WITH RECURSIVE reachable(id) AS (
             SELECT ?
             UNION ALL
             SELECT h.parent_id FROM relationship_type_hierarchy h
             JOIN reachable r ON h.child_id = r.id
             )
             SELECT id FROM reachable WHERE id = ?"#,
        )
        .bind(source_id)
        .bind(target_id)
        .fetch_all(&mut **tx)
        .await?;

        Ok(!rows.is_empty())
    }

    /// Load a relationship type with its parents and aliases.
    pub async fn get_relationship_type(
        &self,
        id: i16,
    ) -> Result<Option<crate::models::relationship_type::RelationshipType>, KnowledgeError> {
        let row: Option<(i16, String, Option<String>, bool, i16)> = sqlx::query_as(
            r#"SELECT id, name, description, sensitive, default_memory_priority_id
             FROM relationship_types WHERE id = ?"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        let Some((id, name, description, sensitive, default_memory_priority_id)) = row else {
            return Ok(None);
        };

        let parent_ids: Vec<i16> = sqlx::query_scalar(
            "SELECT parent_id FROM relationship_type_hierarchy WHERE child_id = ?",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        let aliases: Vec<String> = sqlx::query_scalar(
            "SELECT alias FROM relationship_type_aliases WHERE relationship_type_id = ?",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        Ok(Some(crate::models::relationship_type::RelationshipType {
            id,
            name,
            description,
            sensitive,
            default_memory_priority_id,
            parent_ids,
            aliases,
        }))
    }

    /// Insert a new relationship type with optional parents and aliases in a single call.
    /// Any parent/alias edge that would create a cycle or conflict is rejected.
    pub async fn insert_relationship_type(
        &self,
        new: crate::models::relationship_type::NewRelationshipType,
    ) -> Result<crate::models::relationship_type::RelationshipType, KnowledgeError> {
        let mut tx = self.pool.begin().await?;

        let default_memory_priority_id = new.default_memory_priority_id.unwrap_or(3);

        if canonical_name_conflicts_with_alias(&mut *tx, &new.name).await? {
            return Err(KnowledgeError::Validation(format!(
                "relationship type name '{}' conflicts with an existing alias",
                new.name
            )));
        }

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO relationship_types (name, description, sensitive, default_memory_priority_id)              VALUES (?, ?, ?, ?)              ON CONFLICT (name) DO UPDATE SET name = relationship_types.name RETURNING id",
        )
        .bind(&new.name)
        .bind(new.description.as_deref())
        .bind(new.sensitive)
        .bind(default_memory_priority_id)
        .fetch_one(&mut *tx)
        .await?;
        let id = id as i16;

        for parent_id in &new.parent_ids {
            if *parent_id == id {
                return Err(KnowledgeError::RelationshipTypeCycle);
            }
            if Self::relationship_type_reaches(&mut tx, *parent_id, id).await? {
                return Err(KnowledgeError::RelationshipTypeCycle);
            }
            sqlx::query(
                "INSERT INTO relationship_type_hierarchy (child_id, parent_id) VALUES (?, ?)",
            )
            .bind(id)
            .bind(parent_id)
            .execute(&mut *tx)
            .await?;
        }

        for alias in &new.aliases {
            let Some(normalized) = normalize_alias(alias) else {
                return Err(KnowledgeError::Validation(
                    "alias cannot be empty".to_string(),
                ));
            };
            if alias_conflicts_with_canonical_name(&mut *tx, alias).await? {
                return Err(KnowledgeError::Validation(format!(
                    "alias '{}' conflicts with an existing relationship type name",
                    normalized
                )));
            }
            sqlx::query(
                "INSERT INTO relationship_type_aliases (alias, relationship_type_id) VALUES (?, ?)",
            )
            .bind(&normalized)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(crate::models::relationship_type::RelationshipType {
            id,
            name: new.name,
            description: new.description,
            sensitive: new.sensitive,
            default_memory_priority_id,
            parent_ids: new.parent_ids,
            aliases: new
                .aliases
                .into_iter()
                .filter_map(|a| normalize_alias(&a))
                .collect(),
        })
    }

    /// Current timestamp according to the configured clock.
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }

    /// Read whether condensation needs to run.
    pub fn condensation_dirty(&self) -> bool {
        self.condensation_dirty
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Access the notify channel that fires whenever condensation becomes dirty.
    pub fn condensation_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.condensation_notify)
    }

    /// Mark condensation as dirty (call after any fact mutation).
    pub fn set_condensation_dirty(&self) {
        self.condensation_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.condensation_notify.notify_one();
    }

    /// Clear the condensation dirty flag.
    pub fn clear_condensation_dirty(&self) {
        self.condensation_dirty
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}
