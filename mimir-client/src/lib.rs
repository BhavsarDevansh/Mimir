use bytes::Bytes;
use futures::{Stream, StreamExt};
use mimir_api_types::{
    ChatRequest, ChatResponse, SessionMessagesResponse, SessionSummary, StatusResponse, StreamItem,
    ToolCallInfo, Usage,
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

    /// Send a non-streaming chat request.
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ClientError> {
        let url = format!("{}/chat", self.base_url);
        let resp = self.client.post(&url).json(&req).send().await?;
        let status = resp.status();
        if status.is_success() {
            let body = resp.json::<ChatResponse>().await?;
            Ok(body)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }

    /// Send a streaming chat request and return an SSE item stream.
    pub async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<impl Stream<Item = Result<StreamItem, ClientError>>, ClientError> {
        let url = format!("{}/chat/stream", self.base_url);
        let resp = self.client.post(&url).json(&req).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            });
        }
        Ok(parse_sse_stream(resp.bytes_stream()))
    }

    /// Query the daemon status.
    pub async fn status(&self) -> Result<StatusResponse, ClientError> {
        let url = format!("{}/status", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            let body = resp.json::<StatusResponse>().await?;
            Ok(body)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }

    /// Return the current contents of `memory.md`.
    pub async fn memory(&self) -> Result<String, ClientError> {
        let url = format!("{}/memory", self.base_url);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            let body = resp.text().await?;
            Ok(body)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
    }

    /// Trigger a graceful shutdown of the daemon.
    pub async fn stop(&self) -> Result<(), ClientError> {
        let url = format!("{}/stop", self.base_url);
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
        let url = format!("{}/sessions", self.base_url);
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
        session_id: &str,
    ) -> Result<SessionMessagesResponse, ClientError> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|e| ClientError::Connection(format!("invalid base URL: {}", e)))?;
        url.path_segments_mut()
            .map_err(|_| ClientError::Connection("cannot be a base URL".to_string()))?
            .push("sessions")
            .push(session_id)
            .push("messages");
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        if status.is_success() {
            let body = resp.json::<SessionMessagesResponse>().await?;
            Ok(body)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(ClientError::Server {
                status: status.as_u16(),
                message: text,
            })
        }
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
            data.push_str(rest.trim_start());
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
            session_id: "s1".to_string(),
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
        assert_eq!(result.session_id, "s1");
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
            memory_path: "/tmp".to_string(),
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
            session_id: "sess-1".to_string(),
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
        assert_eq!(result[0].session_id, "sess-1");
        assert_eq!(result[0].preview, Some("hello".to_string()));
    }

    #[tokio::test]
    async fn test_session_messages_parsing() {
        let server = MockServer::start().await;
        let payload = SessionMessagesResponse {
            session_id: "sess-1".to_string(),
            messages: vec![mimir_api_types::ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
            }],
        };
        Mock::given(method("GET"))
            .and(path("/sessions/sess-1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let result = client.session_messages("sess-1").await.unwrap();
        assert_eq!(result.session_id, "sess-1");
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

    #[tokio::test]
    async fn test_session_messages_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sessions/bad-id/messages"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let err = client.session_messages("bad-id").await.unwrap_err();
        assert!(
            matches!(err, ClientError::Server { status: 404, message } if message == "not found")
        );
    }
}
