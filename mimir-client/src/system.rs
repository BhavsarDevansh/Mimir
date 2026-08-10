use mimir_api_types::{
    OptimizationRunNowResponse, SessionMessagesResponse, SessionSummary, StatusResponse,
};

use reqwest::StatusCode;

use crate::MimirClient;
use crate::error::ClientError;

impl MimirClient {
    /// Query the daemon status.
    pub async fn status(&self) -> Result<StatusResponse, ClientError> {
        self.get_json(&self.url("status"), &()).await
    }

    /// Return the current contents of the live memory block.
    pub async fn memory(&self) -> Result<String, ClientError> {
        let resp = Self::send_response(self.client.get(self.url("memory"))).await?;
        Ok(resp.text().await?)
    }

    /// Trigger memory condensation immediately.
    pub async fn memory_refresh(&self) -> Result<OptimizationRunNowResponse, ClientError> {
        Self::send_json(self.client.post(self.url("memory/refresh"))).await
    }

    /// Trigger a graceful shutdown of the daemon.
    ///
    /// A 503 response is treated as success because the server may already be
    /// shutting down; every other non-success status routes through
    /// `Self::check_status` for consistent `ClientError::Server` mapping.
    pub async fn stop(&self) -> Result<(), ClientError> {
        let resp = self.client.post(self.url("stop")).send().await?;
        let status = resp.status();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            // 503 may be returned if the server is already shutting down.
            Ok(())
        } else {
            Self::check_status(resp).await
        }
    }

    /// List all conversation sessions.
    pub async fn sessions(&self) -> Result<Vec<SessionSummary>, ClientError> {
        self.get_json(&self.url("sessions"), &()).await
    }

    /// Fetch messages for a single session from the last compaction point.
    pub async fn session_messages(
        &self,
        session_id: i64,
    ) -> Result<SessionMessagesResponse, ClientError> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|e| ClientError::Connection(format!("invalid base URL: {}", e)))?;
        url.path_segments_mut()
            .map_err(|_| ClientError::Connection("cannot be a base URL".to_string()))?
            .push("sessions")
            .push(&session_id.to_string())
            .push("messages");
        Self::send_json(self.client.get(url)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn test_status_parsing() {
        let server = MockServer::start().await;
        let status = StatusResponse {
            version: "0.13.0".to_string(),
            uptime_seconds: 1,
            queue_depth_user: 0,
            queue_depth_system: 0,
            worker_threads: 1,
            endpoint: "http://localhost:8080".to_string(),
            model: "gpt-4o".to_string(),
            config_path: None,
            config_exists: false,
            llm_reachable: true,
            context_window: None,
            memory_exists: false,
            memory_chars: 0,
            memory_limit: 0,
            memory_usage_pct: 0.0,
        };
        Mock::given(method("GET"))
            .and(path("/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&status))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.status().await.unwrap();
        assert_eq!(result.version, "0.13.0");
    }

    #[tokio::test]
    async fn test_memory_plain_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/memory"))
            .respond_with(ResponseTemplate::new(200).set_body_string("my memory"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.memory().await.unwrap();
        assert_eq!(result, "my memory");
    }

    #[tokio::test]
    async fn test_stop_acceptance() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/stop"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        client.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_server_error_on_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let req = ChatRequest {
            session_id: None,
            message: "hello".to_string(),
            model: None,
            personality_preset: None,
            incognito: None,
        };
        let err = client.chat(req).await.unwrap_err();
        assert!(matches!(err, ClientError::Server { status: 500, message } if message == "boom"));
    }

    #[tokio::test]
    async fn test_server_error_on_503() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(503).set_body_string("queue full"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let req = ChatRequest {
            session_id: None,
            message: "hello".to_string(),
            model: None,
            personality_preset: None,
            incognito: None,
        };
        let err = client.chat(req).await.unwrap_err();
        assert!(
            matches!(err, ClientError::Server { status: 503, message } if message == "queue full")
        );
    }

    #[tokio::test]
    async fn test_sessions_parsing() {
        let server = MockServer::start().await;
        let payload = vec![SessionSummary {
            session_id: 1,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-02T00:00:00Z".to_string(),
            preview: Some("hello".to_string()),
        }];
        Mock::given(method("GET"))
            .and(path("/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.sessions().await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].session_id, 1);
        assert_eq!(result[0].preview, Some("hello".to_string()));
    }

    #[tokio::test]
    async fn test_session_messages_parsing() {
        let server = MockServer::start().await;
        let payload = SessionMessagesResponse {
            session_id: 1,
            messages: vec![mimir_api_types::ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
            }],
        };
        Mock::given(method("GET"))
            .and(path("/sessions/1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.session_messages(1).await.unwrap();
        assert_eq!(result.session_id, 1);
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].role, "user");
    }

    #[tokio::test]
    async fn test_session_messages_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sessions/999999/messages"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let err = client.session_messages(999_999).await.unwrap_err();
        assert!(
            matches!(err, ClientError::Server { status: 404, message } if message == "not found")
        );
    }

    #[tokio::test]
    async fn test_memory_refresh_success() {
        let server = MockServer::start().await;
        let payload = OptimizationRunNowResponse {
            run_id: 42,
            status: "succeeded".to_string(),
            started_at: "2024-01-01T00:00:00Z".to_string(),
            finished_at: None,
            error: None,
        };
        Mock::given(method("POST"))
            .and(path("/memory/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.memory_refresh().await.unwrap();
        assert_eq!(result.run_id, 42);
        assert_eq!(result.status, "succeeded");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_memory_refresh_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/memory/refresh"))
            .respond_with(ResponseTemplate::new(409).set_body_string("already running"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let err = client.memory_refresh().await.unwrap_err();
        assert!(
            matches!(err, ClientError::Server { status: 409, message } if message == "already running")
        );
    }

    #[tokio::test]
    async fn test_kb_optimization_status() {
        let server = MockServer::start().await;
        let payload = OptimizationStatusResponse {
            job_id: "kg-optimization".to_string(),
            priority: "low".to_string(),
            schedule: Some("daily".to_string()),
            next_run_at: Some("2020-01-02T00:00:00Z".to_string()),
            last_run: None,
        };
        Mock::given(method("GET"))
            .and(path("/kb/optimization/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.kb_optimization_status().await.unwrap();
        assert_eq!(result.job_id, "kg-optimization");
        assert_eq!(result.priority, "low");
        assert!(result.last_run.is_none());
    }

    #[tokio::test]
    async fn test_kb_optimization_run_now() {
        let server = MockServer::start().await;
        let payload = OptimizationRunNowResponse {
            run_id: 9,
            status: "running".to_string(),
            started_at: "2020-01-01T00:00:00Z".to_string(),
            finished_at: None,
            error: None,
        };
        Mock::given(method("POST"))
            .and(path("/kb/optimization/run-now"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.kb_optimization_run_now().await.unwrap();
        assert_eq!(result.run_id, 9);
        assert_eq!(result.status, "running");
    }

    #[tokio::test]
    async fn test_stop_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/stop"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        client.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_stop_accepts_503() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/stop"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        client.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_stop_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/stop"))
            .respond_with(ResponseTemplate::new(500).set_body_string("bad"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let err = client.stop().await.unwrap_err();
        assert!(matches!(err, ClientError::Server { status: 500, message } if message == "bad"));
    }

    #[tokio::test]
    async fn test_sessions_list() {
        let server = MockServer::start().await;
        let payload = vec![SessionSummary {
            session_id: 1,
            created_at: "2020-01-01T00:00:00Z".to_string(),
            updated_at: "2020-01-01T00:00:00Z".to_string(),
            preview: Some("hello".to_string()),
        }];
        Mock::given(method("GET"))
            .and(path("/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.sessions().await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].session_id, 1);
    }

    #[tokio::test]
    async fn test_sessions_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sessions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("down"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let err = client.sessions().await.unwrap_err();
        assert!(matches!(err, ClientError::Server { status: 500, message } if message == "down"));
    }

    #[tokio::test]
    async fn test_session_messages_success() {
        let server = MockServer::start().await;
        let payload = SessionMessagesResponse {
            session_id: 5,
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                created_at: "2020-01-01T00:00:00Z".to_string(),
            }],
        };
        Mock::given(method("GET"))
            .and(path("/sessions/5/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.session_messages(5).await.unwrap();
        assert_eq!(result.session_id, 5);
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].role, "user");
    }
}
