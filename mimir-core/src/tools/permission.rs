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
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn default_permission_is_ask() {
        assert_eq!(ToolPermission::default(), ToolPermission::Ask);
    }

    #[test]
    fn as_str_roundtrips_with_from_str() {
        for p in [
            ToolPermission::Auto,
            ToolPermission::Ask,
            ToolPermission::Disabled,
        ] {
            assert_eq!(ToolPermission::from_str(p.as_str()).unwrap(), p);
        }
    }

    #[test]
    fn from_str_is_case_insensitive() {
        assert_eq!(
            ToolPermission::from_str("AUTO").unwrap(),
            ToolPermission::Auto
        );
        assert_eq!(
            ToolPermission::from_str("Ask").unwrap(),
            ToolPermission::Ask
        );
        assert_eq!(
            ToolPermission::from_str("Disabled").unwrap(),
            ToolPermission::Disabled
        );
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(ToolPermission::from_str("yes").is_err());
        assert!(ToolPermission::from_str("").is_err());
    }

    #[test]
    fn serde_uses_lowercase_rename() {
        let json = serde_json::to_string(&ToolPermission::Auto).unwrap();
        assert_eq!(json, "\"auto\"");
        let back: ToolPermission = serde_json::from_str("\"disabled\"").unwrap();
        assert_eq!(back, ToolPermission::Disabled);
    }
}
