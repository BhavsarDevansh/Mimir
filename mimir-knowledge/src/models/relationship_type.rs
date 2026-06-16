//! Relationship type model and DAG metadata.

use serde::{Deserialize, Serialize};

/// A canonical relationship type with its hierarchy parents and English aliases.
///
/// This struct is assembled from `relationship_types` plus joined hierarchy and
/// alias rows; it is not directly `FromRow`-mappable to the `relationship_types`
/// table because `parent_ids` and `aliases` live in separate tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationshipType {
    pub id: i16,
    pub name: String,
    pub description: Option<String>,
    pub sensitive: bool,
    pub default_memory_priority_id: i16,
    #[serde(default)]
    pub parent_ids: Vec<i16>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Input for inserting a new relationship type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewRelationshipType {
    pub name: String,
    pub description: Option<String>,
    pub sensitive: bool,
    pub default_memory_priority_id: Option<i16>,
    #[serde(default)]
    pub parent_ids: Vec<i16>,
    #[serde(default)]
    pub aliases: Vec<String>,
}
