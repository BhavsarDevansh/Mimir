//! Query-string parameter structs for the KB routes.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct QueryParams {
    pub entity: String,
    pub predicate: Option<String>,
    pub min_confidence: Option<f32>,
    pub offset: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQueryParams {
    pub entity: String,
    #[serde(default = "default_depth")]
    pub depth: u32,
    pub offset: Option<u32>,
    pub limit: Option<u32>,
}

fn default_depth() -> u32 {
    2
}

#[derive(Debug, Deserialize)]
pub struct ProfileQueryParams {
    pub entity: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuditQueryParams {
    pub entity: Option<String>,
    pub predicate: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub change_type: Option<String>,
    pub offset: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct TrashQueryParams {
    #[serde(default)]
    pub offset: u32,
    #[serde(default = "default_trash_limit")]
    pub limit: u32,
}

fn default_trash_limit() -> u32 {
    50
}
