use super::super::ToolPermission;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Configuration for a CLI tool loaded from `tools.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliToolConfig {
    pub name: String,
    pub description: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub schema: Value,
    #[serde(default = "default_cli_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub permission: ToolPermission,
}

fn default_cli_timeout() -> u64 {
    30
}
