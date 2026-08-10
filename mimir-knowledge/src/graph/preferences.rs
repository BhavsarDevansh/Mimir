use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Preference delegates
    // ------------------------------------------------------------------

    /// Insert a new preference with context and sources.
    pub async fn insert_preference(
        &self,
        input: models::preference::UpsertPreferenceInput,
    ) -> Result<models::preference::Preference, KnowledgeError> {
        queries::preference::insert_preference(&self.pool, &input, self.now()).await
    }

    /// Upsert a preference with conflict resolution.
    pub async fn upsert_preference(
        &self,
        input: models::preference::UpsertPreferenceInput,
    ) -> Result<
        (
            models::preference::Preference,
            models::preference::UpsertAction,
        ),
        KnowledgeError,
    > {
        queries::preference::upsert_preference(&self.pool, &input, self.now()).await
    }

    /// Contextual preference lookup.
    pub async fn get_preference(
        &self,
        entity_id: Option<i32>,
        key: &str,
        query_context: &[(String, String)],
    ) -> Result<Option<models::preference::Preference>, KnowledgeError> {
        queries::preference::get_preference(&self.pool, entity_id, key, query_context).await
    }

    /// Get preference by ID.
    pub async fn get_preference_by_id(
        &self,
        id: i32,
    ) -> Result<Option<models::preference::Preference>, KnowledgeError> {
        queries::preference::get_preference_by_id(&self.pool, id).await
    }

    /// Get contexts for a preference.
    pub async fn get_preference_contexts(
        &self,
        preference_id: i32,
    ) -> Result<Vec<models::preference::PreferenceContext>, KnowledgeError> {
        queries::preference::get_contexts_for_preference(&self.pool, preference_id).await
    }

    /// Get sources for a preference.
    pub async fn get_preference_sources(
        &self,
        preference_id: i32,
    ) -> Result<Vec<models::preference::PreferenceSource>, KnowledgeError> {
        queries::preference::get_sources_for_preference(&self.pool, preference_id).await
    }

    /// Get audit log for a preference.
    pub async fn get_preference_audit_log(
        &self,
        preference_id: i32,
    ) -> Result<Vec<models::preference::PreferenceAuditLogEntry>, KnowledgeError> {
        queries::preference::get_preference_audit_log(&self.pool, preference_id).await
    }
}
