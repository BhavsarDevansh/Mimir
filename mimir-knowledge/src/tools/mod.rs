//! LLM-callable knowledge graph tools.

mod kg_expand_catalogue;
mod kg_facts_in_catalogue;
mod kg_query;
mod kg_related;
mod kg_search;
mod remember;

pub use kg_expand_catalogue::KgExpandCatalogueTool;
pub use kg_facts_in_catalogue::KgFactsInCatalogueTool;
pub use kg_query::KgQueryTool;
pub use kg_related::KgRelatedTool;
pub use kg_search::KgSearchTool;
pub use remember::RememberTool;

mod retrieve_context;
pub use retrieve_context::RetrieveContextTool;

/// Map a fact_status_id to its human-readable name.
pub(crate) fn fact_status_name(id: i16) -> String {
    match id {
        1 => "Active".to_string(),
        2 => "Inferred".to_string(),
        3 => "Disputed".to_string(),
        4 => "Corrected".to_string(),
        5 => "Superseded".to_string(),
        6 => "Forgotten".to_string(),
        _ => format!("Unknown({})", id),
    }
}

/// Map a source_type_id to its human-readable name.
pub(crate) fn source_type_name(id: i16) -> String {
    match id {
        1 => "UserEdit".to_string(),
        2 => "Connector".to_string(),
        3 => "Inference".to_string(),
        4 => "Interaction".to_string(),
        5 => "Import".to_string(),
        6 => "System".to_string(),
        _ => format!("Unknown({})", id),
    }
}

/// Map an entity_type_id to its human-readable name.
pub(crate) fn entity_type_name(id: i16) -> String {
    match id {
        1 => "Person".to_string(),
        2 => "Place".to_string(),
        3 => "Event".to_string(),
        4 => "Object".to_string(),
        5 => "Concept".to_string(),
        6 => "Organization".to_string(),
        7 => "Activity".to_string(),
        8 => "DateTime".to_string(),
        _ => format!("Unknown({})", id),
    }
}
