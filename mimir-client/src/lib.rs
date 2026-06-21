#![deny(unsafe_code)]
use bytes::Bytes;
use futures::{Stream, StreamExt};
use mimir_api_types::{
    AuditQueryRequest, AuditQueryResponse, BrowseRequest, BrowseResponse, CategoryDetailResponse,
    CategoryResponse, ChatRequest, ChatResponse, ConfirmFactResponse, FactDetailResponse,
    FactEditRequest, FactEditResponse, FactQueryParams, FactQueryResponse, ForgetRequest,
    ForgetResponse, OptimizationRunNowResponse, OptimizationStatusResponse, PendingListResponse,
    ProfileRequest, ProfileResponse, RejectFactRequest, RestoreRequest, RestoreResponse,
    SessionMessagesResponse, SessionSummary, StatusResponse, StreamItem, ToolCallInfo,
    TrashListResponse, Usage,
};
use reqwest::StatusCode;
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
#[derive(Debug, Clone)]
pub struct MimirClient {
    base_url: String,
    client: reqwest::Client,
}

impl MimirClient {
    /// Create a new client pointing at the given base URL.
    ///
    /// The base URL should include the scheme and host/port, e.g.
    /// `http://127.0.0.1:8080`.
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");
        Self {
            base_url: base_url.into(),
            client,
        }
    }

    /// Validate the HTTP response status, returning the response on success or a
    /// [`ClientError::Server`] on failure.
    async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, ClientError> {
        let status = resp.status();
        if status.is_success() {
            Ok(resp)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }

    /// Build a URL by appending `path` to the configured base URL.
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path)
    }

    /// Send a non-streaming chat request.
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ClientError> {
        let url = self.url("chat");
        let resp = self.client.post(&url).json(&req).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<ChatResponse>().await?;
        Ok(body)
    }

    /// Send a streaming chat request and return an SSE item stream.
    pub async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<impl Stream<Item = Result<StreamItem, ClientError>>, ClientError> {
        let url = self.url("chat/stream");
        let resp = self.client.post(&url).json(&req).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(parse_sse_stream(resp.bytes_stream()))
    }

    /// Query the daemon status.
    pub async fn status(&self) -> Result<StatusResponse, ClientError> {
        let url = self.url("status");
        let resp = self.client.get(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<StatusResponse>().await?;
        Ok(body)
    }

    /// Return the current contents of the live memory block.
    pub async fn memory(&self) -> Result<String, ClientError> {
        let url = self.url("memory");
        let resp = self.client.get(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.text().await?;
        Ok(body)
    }

    /// Trigger memory condensation immediately.
    pub async fn memory_refresh(&self) -> Result<OptimizationRunNowResponse, ClientError> {
        let url = self.url("memory/refresh");
        let resp = self.client.post(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<OptimizationRunNowResponse>().await?;
        Ok(body)
    }

    /// Query the knowledge graph optimization job status.
    pub async fn kb_optimization_status(&self) -> Result<OptimizationStatusResponse, ClientError> {
        let url = self.url("kb/optimization/status");
        let resp = self.client.get(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<OptimizationStatusResponse>().await?;
        Ok(body)
    }

    /// Trigger the knowledge graph optimization job immediately.
    pub async fn kb_optimization_run_now(&self) -> Result<OptimizationRunNowResponse, ClientError> {
        let url = self.url("kb/optimization/run-now");
        let resp = self.client.post(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<OptimizationRunNowResponse>().await?;
        Ok(body)
    }

    // Trigger a graceful shutdown of the daemon.
    // ------------------------------------------------------------------
    // Knowledge Graph (kb) commands
    // ------------------------------------------------------------------

    /// Query facts for an entity.
    pub async fn kb_query(&self, req: FactQueryParams) -> Result<FactQueryResponse, ClientError> {
        let url = self.url("kb/query");
        let mut params = vec![("entity", req.entity)];
        if let Some(p) = req.predicate {
            params.push(("predicate", p));
        }
        if let Some(c) = req.min_confidence {
            params.push(("min_confidence", c.to_string()));
        }
        if let Some(o) = req.offset {
            params.push(("offset", o.to_string()));
        }
        if let Some(l) = req.limit {
            params.push(("limit", l.to_string()));
        }
        let resp = self.client.get(&url).query(&params).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<FactQueryResponse>().await?;
        Ok(body)
    }

    /// Show a single fact by ID.
    pub async fn kb_show(&self, fact_id: i32) -> Result<FactDetailResponse, ClientError> {
        let url = self.url(&format!("kb/facts/{fact_id}"));
        let resp = self.client.get(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<FactDetailResponse>().await?;
        Ok(body)
    }

    /// Edit a single fact.
    pub async fn kb_edit(
        &self,
        fact_id: i32,
        req: FactEditRequest,
    ) -> Result<FactEditResponse, ClientError> {
        let url = self.url(&format!("kb/facts/{fact_id}"));
        let resp = self.client.patch(&url).json(&req).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<FactEditResponse>().await?;
        Ok(body)
    }

    /// Browse the knowledge graph from an entity.
    pub async fn kb_browse(&self, req: BrowseRequest) -> Result<BrowseResponse, ClientError> {
        let url = self.url("kb/browse");
        let mut params: Vec<(&str, String)> =
            vec![("entity", req.entity), ("depth", req.depth.to_string())];
        if let Some(o) = req.offset {
            params.push(("offset", o.to_string()));
        }
        if let Some(l) = req.limit {
            params.push(("limit", l.to_string()));
        }
        let resp = self.client.get(&url).query(&params).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<BrowseResponse>().await?;
        Ok(body)
    }

    /// Get a profile for an entity.
    pub async fn kb_profile(&self, req: ProfileRequest) -> Result<ProfileResponse, ClientError> {
        let url = self.url("kb/profile");
        let mut params: Vec<(&str, String)> = vec![];
        if let Some(e) = req.entity {
            params.push(("entity", e));
        }
        let resp = self.client.get(&url).query(&params).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<ProfileResponse>().await?;
        Ok(body)
    }

    /// Query the audit log.
    pub async fn kb_audit(
        &self,
        req: AuditQueryRequest,
    ) -> Result<AuditQueryResponse, ClientError> {
        let url = self.url("kb/audit");
        let mut params: Vec<(&str, String)> = vec![];
        if let Some(e) = req.entity {
            params.push(("entity", e));
        }
        if let Some(p) = req.predicate {
            params.push(("predicate", p));
        }
        if let Some(f) = req.from {
            params.push(("from", f));
        }
        if let Some(t) = req.to {
            params.push(("to", t));
        }
        if let Some(c) = req.change_type {
            params.push(("change_type", c));
        }
        if let Some(o) = req.offset {
            params.push(("offset", o.to_string()));
        }
        if let Some(l) = req.limit {
            params.push(("limit", l.to_string()));
        }
        let resp = self.client.get(&url).query(&params).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<AuditQueryResponse>().await?;
        Ok(body)
    }

    /// Forget facts (single or bulk).
    pub async fn kb_forget(&self, req: ForgetRequest) -> Result<ForgetResponse, ClientError> {
        let url = self.url("kb/facts/forget");
        let resp = self.client.post(&url).json(&req).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<ForgetResponse>().await?;
        Ok(body)
    }

    /// Restore facts from trash.
    pub async fn kb_restore(&self, req: RestoreRequest) -> Result<RestoreResponse, ClientError> {
        let url = self.url("kb/trash/restore");
        let resp = self.client.post(&url).json(&req).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<RestoreResponse>().await?;
        Ok(body)
    }

    /// List trash contents.
    pub async fn kb_trash(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<TrashListResponse, ClientError> {
        let url = self.url("kb/trash");
        let params = [("offset", offset.to_string()), ("limit", limit.to_string())];
        let resp = self.client.get(&url).query(&params).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<TrashListResponse>().await?;
        Ok(body)
    }

    /// Empty the trash.
    pub async fn kb_trash_empty(&self) -> Result<(), ClientError> {
        let url = self.url("kb/trash");
        let resp = self.client.delete(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }
    /// List pending sensitive facts awaiting confirmation.
    pub async fn kb_pending(&self) -> Result<PendingListResponse, ClientError> {
        let url = self.url("kb/pending");
        let resp = self.client.get(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<PendingListResponse>().await?;
        Ok(body)
    }

    /// Confirm a pending sensitive fact.
    pub async fn kb_confirm(&self, fact_id: i32) -> Result<ConfirmFactResponse, ClientError> {
        let url = self.url(&format!("kb/facts/{fact_id}/confirm"));
        let resp = self.client.post(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<ConfirmFactResponse>().await?;
        Ok(body)
    }

    /// Reject a pending sensitive fact. An optional reason is written to the
    /// audit log. Returns `Ok(())` on a 204 No Content.
    pub async fn kb_reject(&self, fact_id: i32, reason: Option<&str>) -> Result<(), ClientError> {
        let url = self.url(&format!("kb/facts/{fact_id}/reject"));
        let req = RejectFactRequest {
            reason: reason.map(|s| s.to_string()),
        };
        let resp = self.client.post(&url).json(&req).send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }

    pub async fn stop(&self) -> Result<(), ClientError> {
        let url = self.url("stop");
        let resp = self.client.post(&url).send().await?;
        let status = resp.status();
        if status.is_success() || status == StatusCode::SERVICE_UNAVAILABLE {
            // 503 may be returned if the server is already shutting down.
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }

    /// List all conversation sessions.
    pub async fn sessions(&self) -> Result<Vec<SessionSummary>, ClientError> {
        let url = self.url("sessions");
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            let body = resp.json::<Vec<SessionSummary>>().await?;
            Ok(body)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
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
        let resp = self.client.get(url).send().await?;
        let resp = Self::check_response(resp).await?;
        let body = resp.json::<SessionMessagesResponse>().await?;
        Ok(body)
    }
    /// List knowledge graph categories.
    pub async fn kb_categories(
        &self,
        parent: Option<i32>,
    ) -> Result<Vec<CategoryResponse>, ClientError> {
        let url = self.url("kb/categories");
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(p) = parent {
            params.push(("parent", p.to_string()));
        }
        let resp = self.client.get(&url).query(&params).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json::<Vec<CategoryResponse>>().await?)
    }

    /// Show a single category with its children.
    pub async fn kb_category_show(&self, id: i32) -> Result<CategoryDetailResponse, ClientError> {
        let url = self.url(&format!("kb/categories/{id}"));
        let resp = self.client.get(&url).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json::<CategoryDetailResponse>().await?)
    }

    /// Create a new knowledge graph category.
    pub async fn kb_category_create(
        &self,
        id: i32,
        name: String,
        parent_id: Option<i32>,
        description: Option<String>,
        memory_weight: Option<f32>,
    ) -> Result<CategoryResponse, ClientError> {
        let url = self.url("kb/categories");
        let body = serde_json::json!({
            "id": id,
            "name": name,
            "parent_id": parent_id,
            "description": description,
            "memory_weight": memory_weight,
        });
        let resp = self.client.post(&url).json(&body).send().await?;
        let resp = Self::check_response(resp).await?;
        Ok(resp.json::<CategoryResponse>().await?)
    }

    /// Delete a knowledge graph category.
    pub async fn kb_category_delete(&self, id: i32) -> Result<(), ClientError> {
        let url = self.url(&format!("kb/categories/{id}"));
        let resp = self.client.delete(&url).send().await?;
        Self::check_response(resp).await?;
        Ok(())
    }
}

/// Parse a byte stream into SSE events.
///
/// Buffers raw bytes and only decodes complete events (delimited by `\n\n`)
/// so that multi-byte UTF-8 sequences split across TCP/HTTP chunk boundaries
/// are preserved.
fn parse_sse_stream(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>>,
) -> impl Stream<Item = Result<StreamItem, ClientError>> {
    let mut buf = Vec::new();
    stream
        .filter_map(move |result| match result {
            Ok(bytes) => {
                buf.extend_from_slice(&bytes);
                let mut items = Vec::new();
                while let Some((pos, delim_len)) = find_double_newline(&buf) {
                    let event_bytes: Vec<u8> = buf.drain(..pos + delim_len).collect();
                    match String::from_utf8(event_bytes) {
                        Ok(event) => {
                            if let Some(item) = parse_sse_event(&event) {
                                items.push(item);
                            }
                        }
                        Err(_) => {
                            items.push(Err(ClientError::Connection(
                                "invalid UTF-8 in SSE event".to_string(),
                            )));
                        }
                    }
                }
                futures::future::ready(Some(items))
            }
            Err(e) => futures::future::ready(Some(vec![Err(ClientError::Http(e))])),
        })
        .flat_map(futures::stream::iter)
}

/// Return the index and delimiter length of the first `\n\n` or `\r\n\r\n` in `buf`.
fn find_double_newline(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        if buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        if buf[i..].starts_with(b"\n\n") {
            return Some((i, 2));
        }
    }
    None
}

/// Parse a single SSE event block into a [`StreamItem`] or an error.
fn parse_sse_event(event: &str) -> Option<Result<StreamItem, ClientError>> {
    let mut event_type = "";
    let mut data = String::new();
    for line in event.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = rest.trim();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            // Per SSE spec: strip exactly one leading space after "data:", not all whitespace.
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            data.push_str(value);
        }
    }
    match event_type {
        "usage" => match serde_json::from_str::<Usage>(&data) {
            Ok(u) => Some(Ok(StreamItem::Usage(u))),
            Err(e) => Some(Err(ClientError::Serialization(e))),
        },
        "tool_call" => match serde_json::from_str::<ToolCallInfo>(&data) {
            Ok(info) => Some(Ok(StreamItem::ToolCall(info))),
            Err(e) => Some(Err(ClientError::Serialization(e))),
        },
        "session_id" => match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(v) => v
                .get("session_id")
                .and_then(|s| s.as_i64())
                .map(|id| Ok(StreamItem::SessionId(id.to_string()))),
            Err(_) => None,
        },
        "error" => Some(Err(ClientError::Server {
            status: 500,
            message: data,
        })),
        // default / no event type → text
        _ => {
            if data.is_empty() {
                None
            } else {
                Some(Ok(StreamItem::Text(data)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

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

    #[tokio::test]
    async fn test_chat_roundtrip() {
        let server = MockServer::start().await;
        let resp = ChatResponse {
            session_id: 1,
            response: "hi".to_string(),
            usage: Usage::default(),
            tool_calls: vec![],
        };
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&resp))
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
        let result = client.chat(req).await.unwrap();
        assert_eq!(result.session_id, 1);
        assert_eq!(result.response, "hi");
    }

    #[tokio::test]
    async fn test_chat_stream_parses_text_and_usage() {
        let server = MockServer::start().await;
        let body = "data: hello\n\nevent: usage\ndata: {\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
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
        let mut stream = client.chat_stream(req).await.unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, StreamItem::Text("hello".to_string()));
        let second = stream.next().await.unwrap().unwrap();
        assert_eq!(
            second,
            StreamItem::Usage(Usage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
            })
        );
    }

    #[tokio::test]
    async fn test_chat_stream_error_event() {
        let server = MockServer::start().await;
        let body = "event: error\ndata: something went wrong\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
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
        let mut stream = client.chat_stream(req).await.unwrap();
        let item = stream.next().await.unwrap();
        assert!(
            matches!(item, Err(ClientError::Server { status: 500, message }) if message == "something went wrong")
        );
    }

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

    #[test]
    fn test_parse_sse_tool_call_event() {
        let event = "event: tool_call\ndata: {\"name\":\"get_current_time\",\"display_name\":\"Get Current Time\",\"result\":\"2025-05-30T12:00:00Z\"}\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::ToolCall(info)) => {
                assert_eq!(info.name, "get_current_time");
                assert_eq!(info.display_name, "Get Current Time");
                assert_eq!(info.result, "2025-05-30T12:00:00Z");
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_default_event_is_text() {
        let event = "data: Hello world\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::Text(t)) => assert_eq!(t, "Hello world"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_with_leading_space_preserved() {
        // When the LLM streams a token like " on", the SSE data line becomes
        // "data:  on" (two spaces after colon). Per SSE spec only the first
        // space after "data:" is stripped; the second is part of the content.
        let event = "data:  on\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::Text(t)) => assert_eq!(t, " on"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_session_id_event() {
        let event = "event: session_id\ndata: {\"session_id\":123}\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::SessionId(s)) => assert_eq!(s, "123"),
            other => panic!("expected SessionId, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_no_space_after_colon() {
        // "data:" with no space is valid SSE; content starts immediately.
        let event = "data:hello\n\n";
        let result = parse_sse_event(event);
        assert!(result.is_some());
        let item = result.unwrap();
        match item {
            Ok(StreamItem::Text(t)) => assert_eq!(t, "hello"),
            other => panic!("expected Text, got {:?}", other),
        }
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

    #[test]
    fn test_url_helper_builds_correct_urls() {
        let client = MimirClient::new("http://127.0.0.1:8080");
        assert_eq!(client.url("chat"), "http://127.0.0.1:8080/chat");
        assert_eq!(
            client.url("kb/categories"),
            "http://127.0.0.1:8080/kb/categories"
        );
        assert_eq!(
            client.url("chat/stream"),
            "http://127.0.0.1:8080/chat/stream"
        );
    }

    #[tokio::test]
    async fn test_kb_categories_parsing() {
        let server = MockServer::start().await;
        let payload = vec![CategoryResponse {
            id: 1,
            name: "People".to_string(),
            description: None,
            parent_id: None,
            memory_weight: Some(1.0),
        }];
        Mock::given(method("GET"))
            .and(path("/kb/categories"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let cats = client.kb_categories(None).await.unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].id, 1);
    }

    #[tokio::test]
    async fn test_kb_category_show_parsing() {
        let server = MockServer::start().await;
        let payload = CategoryDetailResponse {
            id: 1,
            name: "People".to_string(),
            description: None,
            parent_id: None,
            memory_weight: Some(1.0),
            fact_count: 5,
            children: vec![],
        };
        Mock::given(method("GET"))
            .and(path("/kb/categories/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let cat = client.kb_category_show(1).await.unwrap();
        assert_eq!(cat.id, 1);
        assert_eq!(cat.fact_count, 5);
    }

    #[tokio::test]
    async fn test_kb_category_create_and_delete() {
        let server = MockServer::start().await;
        let payload = CategoryResponse {
            id: 42,
            name: "Places".to_string(),
            description: None,
            parent_id: None,
            memory_weight: Some(1.0),
        };
        Mock::given(method("POST"))
            .and(path("/kb/categories"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/kb/categories/42"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let cat = client
            .kb_category_create(42, "Places".to_string(), None, None, None)
            .await
            .unwrap();
        assert_eq!(cat.id, 42);
        client.kb_category_delete(42).await.unwrap();
    }
}
