use thiserror::Error;

/// Errors that can occur when interacting with the Mimir daemon.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Connection failure or DNS error.
    #[error("connection error: {0}")]
    Connection(String),
    /// HTTP-level error from reqwest.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// JSON serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The server returned an explicit error event or non-2xx status.
    #[error("server error {status}: {message}")]
    Server { status: u16, message: String },
}

/// A thin HTTP client for the Mimir daemon.
#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use mimir_api_types::{
        AuditRow, BrowseEdge, ChatMessage, ChatRequest, FactRow, OptimizationStatusResponse,
        PendingFactRow, ProfileGroup, TrashRow, Usage,
    };
    #[allow(unused_imports)]
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    use crate::ClientError;

    #[test]
    fn test_client_error_display() {
        let err = ClientError::Connection("dns fail".to_string());
        assert_eq!(err.to_string(), "connection error: dns fail");

        let err2 = ClientError::Server {
            status: 503,
            message: "busy".to_string(),
        };
        assert_eq!(err2.to_string(), "server error 503: busy");
    }
}
