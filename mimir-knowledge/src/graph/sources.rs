use crate::graph::KnowledgeGraph;
use crate::*;

impl KnowledgeGraph {
    // ------------------------------------------------------------------
    // Source CRUD delegates
    // ------------------------------------------------------------------

    /// Retrieve all sources linked to a fact.
    pub async fn get_sources_for_fact(
        &self,
        fact_id: i32,
    ) -> Result<Vec<models::source::Source>, KnowledgeError> {
        queries::source::get_sources_for_fact(&self.pool, fact_id).await
    }

    /// Add a new source to an existing fact and write a `source_added` audit entry.
    pub async fn add_source_to_fact(
        &self,
        request: queries::source::AddSourceRequest,
    ) -> Result<models::source::Source, KnowledgeError> {
        let input = queries::source::SourceInput {
            fact_id: request.fact_id,
            source_type_id: request.source_type as i16,
            connector_instance_id: request.connector_instance_id,
            connector_type_id: request.connector_type.map(|c| c as i16),
            raw_reference: request.raw_reference,
            extraction_method_id: request.extraction_method.map(|e| e as i16),
        };
        queries::source::add_source_to_fact(&self.pool, &input, self.now(), request.changed_by)
            .await
    }
}
