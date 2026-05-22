use super::super::ToolPermission;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::path::PathBuf;

fn default_cli_timeout() -> u64 {
    30
}

fn deserialize_non_zero_timeout<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        Ok(default_cli_timeout())
    } else {
        Ok(value)
    }
}

/// Configuration for a CLI tool loaded from `tools.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliToolConfig {
    pub name: String,
    pub description: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub schema: Value,
    #[serde(
        default = "default_cli_timeout",
        deserialize_with = "deserialize_non_zero_timeout"
    )]
    pub timeout_secs: u64,
    #[serde(default)]
    pub permission: ToolPermission,
}
