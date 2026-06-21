use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ToolError {
    #[error("permission denied for tool '{0}'")]
    PermissionDenied(String),

    #[error("tool '{0}' execution failed: {1}")]
    ExecutionFailed(String, String),

    #[error("tool '{0}' timed out after {1:?}")]
    Timeout(String, Duration),

    #[error("tool '{0}' received invalid arguments: {1}")]
    InvalidArguments(String, String),

    #[error("tool '{0}' not found")]
    NotFound(String),

    #[error("tool '{0}' is disabled")]
    Disabled(String),

    #[error("CLI tool '{0}' process error: {1}")]
    CliProcessError(String, String),

    #[error("schema error for tool '{0}': {1}")]
    SchemaError(String, String),

    #[error("tool '{0}' is already registered")]
    AlreadyRegistered(String),
}

impl ToolError {
    pub fn permission_denied(name: impl Into<String>) -> Self {
        Self::PermissionDenied(name.into())
    }

    pub fn execution_failed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ExecutionFailed(name.into(), message.into())
    }

    pub fn timeout(name: impl Into<String>, timeout: Duration) -> Self {
        Self::Timeout(name.into(), timeout)
    }

    pub fn invalid_arguments(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidArguments(name.into(), message.into())
    }

    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound(name.into())
    }

    pub fn disabled(name: impl Into<String>) -> Self {
        Self::Disabled(name.into())
    }

    pub fn cli_process_error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::CliProcessError(name.into(), message.into())
    }

    pub fn schema_error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::SchemaError(name.into(), message.into())
    }

   pub fn already_registered(name: impl Into<String>) -> Self {
       Self::AlreadyRegistered(name.into())
   }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_build_correct_variants() {
        assert_eq!(
            ToolError::permission_denied("echo"),
            ToolError::PermissionDenied("echo".to_string())
        );
        assert_eq!(
            ToolError::execution_failed("echo", "boom"),
            ToolError::ExecutionFailed("echo".to_string(), "boom".to_string())
        );
        assert_eq!(
            ToolError::timeout("echo", Duration::from_secs(1)),
            ToolError::Timeout("echo".to_string(), Duration::from_secs(1))
        );
        assert_eq!(
            ToolError::invalid_arguments("echo", "bad"),
            ToolError::InvalidArguments("echo".to_string(), "bad".to_string())
        );
        assert_eq!(
            ToolError::not_found("echo"),
            ToolError::NotFound("echo".to_string())
        );
        assert_eq!(
            ToolError::disabled("echo"),
            ToolError::Disabled("echo".to_string())
        );
        assert_eq!(
            ToolError::cli_process_error("echo", "io"),
            ToolError::CliProcessError("echo".to_string(), "io".to_string())
        );
        assert_eq!(
            ToolError::schema_error("echo", "bad"),
            ToolError::SchemaError("echo".to_string(), "bad".to_string())
        );
        assert_eq!(
            ToolError::already_registered("echo"),
            ToolError::AlreadyRegistered("echo".to_string())
        );
    }

    #[test]
    fn constructors_accept_into_string() {
        let s: String = "echo".to_string();
        assert_eq!(ToolError::not_found(s.clone()), ToolError::NotFound(s));
        assert_eq!(ToolError::not_found("echo"), ToolError::NotFound("echo".to_string()));
    }

    #[test]
    fn display_messages_are_human_readable() {
        assert_eq!(
            ToolError::PermissionDenied("echo".to_string()).to_string(),
            "permission denied for tool 'echo'"
        );
        assert_eq!(
            ToolError::Timeout("echo".to_string(), Duration::from_secs(2)).to_string(),
            "tool 'echo' timed out after 2s"
        );
        assert_eq!(
            ToolError::NotFound("echo".to_string()).to_string(),
            "tool 'echo' not found"
        );
    }
}
