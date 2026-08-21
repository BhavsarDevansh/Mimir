//! LLM-callable knowledge graph tools.

mod kg_expand_catalogue;
mod kg_facts_in_catalogue;
mod kg_query;
mod kg_related;
mod kg_search;

pub use kg_expand_catalogue::KgExpandCatalogueTool;
pub use kg_facts_in_catalogue::KgFactsInCatalogueTool;
pub use kg_query::KgQueryTool;
pub use kg_related::KgRelatedTool;
pub use kg_search::KgSearchTool;

mod retrieve_context;
pub use retrieve_context::RetrieveContextTool;

use crate::models::entity::EntityType;
use crate::models::fact::FactStatus;
use crate::models::source::SourceType;

/// Map a fact_status_id to its human-readable name.
pub(crate) fn fact_status_name(id: i16) -> String {
    FactStatus::try_from(id)
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|_| format!("Unknown({})", id))
}

/// Map a source_type_id to its human-readable name.
pub(crate) fn source_type_name(id: i16) -> String {
    SourceType::try_from(id)
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|_| format!("Unknown({})", id))
}

/// Map an entity_type_id to its human-readable name.
pub(crate) fn entity_type_name(id: i16) -> String {
    EntityType::try_from(id)
        .map(|e| e.as_str().to_string())
        .unwrap_or_else(|_| format!("Unknown({})", id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_helpers_match_wire_contract() {
        assert_eq!(fact_status_name(FactStatus::Active as i16), "Active");
        assert_eq!(fact_status_name(FactStatus::Forgotten as i16), "Forgotten");
        assert_eq!(source_type_name(SourceType::UserEdit as i16), "UserEdit");
        assert_eq!(source_type_name(SourceType::System as i16), "System");
        assert_eq!(entity_type_name(EntityType::Person as i16), "Person");
        assert_eq!(entity_type_name(EntityType::DateTime as i16), "DateTime");
        assert_eq!(fact_status_name(99), "Unknown(99)");
        assert_eq!(source_type_name(99), "Unknown(99)");
        assert_eq!(entity_type_name(99), "Unknown(99)");
    }
}
