use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum SkillError {
    #[error("permission denied for skill '{0}'")]
    PermissionDenied(String),

    #[error("skill '{0}' execution failed: {1}")]
    ExecutionFailed(String, String),

    #[error("skill '{0}' timed out after {1:?}")]
    Timeout(String, Duration),

    #[error("skill '{0}' received invalid arguments: {1}")]
    InvalidArguments(String, String),

    #[error("skill '{0}' not found")]
    NotFound(String),

    #[error("skill '{0}' is disabled")]
    Disabled(String),

    #[error("skill '{0}' is already registered")]
    AlreadyRegistered(String),

    #[error("parse error for skill '{0}': {1}")]
    ParseError(String, String),

    #[error("metrics error for skill '{0}': {1}")]
    MetricsError(String, String),

    #[error("cannot delete built-in or system-generated skill '{0}'")]
    Protected(String),
}

impl SkillError {
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

    pub fn already_registered(name: impl Into<String>) -> Self {
        Self::AlreadyRegistered(name.into())
    }

    pub fn parse_error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ParseError(name.into(), message.into())
    }

    pub fn metrics_error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::MetricsError(name.into(), message.into())
    }

    pub fn protected(name: impl Into<String>) -> Self {
        Self::Protected(name.into())
    }
}
