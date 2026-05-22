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
