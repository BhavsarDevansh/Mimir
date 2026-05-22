use serde::{Deserialize, Serialize};

/// Permission level for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolPermission {
    /// Run immediately without user confirmation.
    Auto,
    /// Requires user approval (Phase 1: returns permission-denied error).
    #[default]
    Ask,
    /// Tool is disabled and cannot be invoked.
    Disabled,
}

impl ToolPermission {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolPermission::Auto => "auto",
            ToolPermission::Ask => "ask",
            ToolPermission::Disabled => "disabled",
        }
    }
}

impl std::str::FromStr for ToolPermission {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(ToolPermission::Auto),
            "ask" => Ok(ToolPermission::Ask),
            "disabled" => Ok(ToolPermission::Disabled),
            _ => Err(format!("invalid permission: {s}")),
        }
    }
}
