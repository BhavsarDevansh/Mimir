use std::time::Duration;

use futures::{Stream, StreamExt};

use mimir_api_types::{ChatRequest, ChatResponse, StreamItem};

use crate::MimirClient;
use crate::error::ClientError;
use crate::sse::parse_sse_stream;

impl MimirClient {
    /// Total timeout for a non-streaming chat request.
    ///
    /// The agentic loop can run many tool rounds — each with its own LLM call
    /// and possibly a multi-minute retrieval agent — so the client's default
    /// 120s total timeout is too short (issue #487). Overridden per request.
    pub const CHAT_TOTAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

    /// Total timeout backstop for a streaming chat request.
    ///
    /// The stream is bounded by [`Self::CHAT_STREAM_READ_TIMEOUT`]; this only
    /// guards against a server that never stops sending (issue #487).
    pub const CHAT_STREAM_TOTAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

    /// Maximum silence between SSE chunks before a streaming chat response is
    /// considered wedged.
    ///
    /// The daemon emits keep-alive comments every 10s, so 60s means six missed
    /// keep-alives. A wall-clock total timeout is the wrong tool for a stream
    /// that can legitimately run for minutes (issue #487).
    pub const CHAT_STREAM_READ_TIMEOUT: Duration = Duration::from_secs(60);

    /// Send a non-streaming chat request.
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ClientError> {
        Self::send_json(
            self.client
                .post(self.url("chat"))
                .json(&req)
                .timeout(Self::CHAT_TOTAL_TIMEOUT),
        )
        .await
    }

    /// Send a streaming chat request and return an SSE item stream.
    pub async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<impl Stream<Item = Result<StreamItem, ClientError>>, ClientError> {
        let resp = Self::send_response(
            self.client
                .post(self.url("chat/stream"))
                .json(&req)
                .timeout(Self::CHAT_STREAM_TOTAL_TIMEOUT),
        )
        .await?;
        let byte_stream = resp
            .bytes_stream()
            .map(|item| item.map_err(ClientError::Http));
        Ok(parse_sse_stream(with_read_timeout(
            byte_stream,
            Self::CHAT_STREAM_READ_TIMEOUT,
        )))
    }
}

/// Wrap a byte stream with a per-chunk read timeout.
///
/// Each chunk resets the deadline, so a slow-but-alive stream (e.g. the
/// daemon's keep-alive comments) is never cut off; only a stream that goes
/// completely silent for `timeout` is reported as wedged.
fn with_read_timeout<S>(
    stream: S,
    timeout: Duration,
) -> impl Stream<Item = Result<bytes::Bytes, ClientError>>
where
    S: Stream<Item = Result<bytes::Bytes, ClientError>>,
{
    let timed = tokio_stream::StreamExt::timeout(stream, timeout);
    let mapped = tokio_stream::StreamExt::map(timed, |item| match item {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(ClientError::Connection(
            "timed out waiting for stream data".to_string(),
        )),
    });
    // Box so the returned stream stays `Unpin` (the timeout wrapper is not),
    // keeping `chat_stream`'s public stream usable with `StreamExt::next`.
    Box::pin(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use mimir_api_types::{
        AuditRow, BrowseEdge, ChatMessage, ChatRequest, FactRow, OptimizationStatusResponse,
        PendingFactRow, ProfileGroup, TrashRow, Usage,
    };
    use std::time::Duration;
    #[allow(unused_imports)]
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

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
    async fn test_chat_server_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(503).set_body_string("overloaded"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let err = client
            .chat(ChatRequest {
                session_id: None,
                message: "hi".to_string(),
                model: None,
                personality_preset: None,
                incognito: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ClientError::Server { status: 503, message } if message == "overloaded")
        );
    }

    #[tokio::test]
    async fn test_chat_stream_session_id_and_tool_call() {
        let server = MockServer::start().await;
        let body = "event: session_id\ndata: {\"session_id\":99}\n\nevent: tool_call\ndata: {\"name\":\"echo\",\"display_name\":\"Echo\",\"result\":\"hi\"}\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let mut stream = client
            .chat_stream(ChatRequest {
                session_id: None,
                message: "hi".to_string(),
                model: None,
                personality_preset: None,
                incognito: None,
            })
            .await
            .unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, StreamItem::SessionId("99".to_string()));
        let second = stream.next().await.unwrap().unwrap();
        assert!(matches!(second, StreamItem::ToolCall(_)));
    }

    #[tokio::test]
    async fn test_chat_stream_invalid_utf8_returns_connection_error() {
        let server = MockServer::start().await;
        // A raw invalid-UTF-8 byte sequence (0xFF is never a valid leading byte).
        let body: Vec<u8> = b"data: ".to_vec();
        let body = [body, vec![0xFF, 0xFE], b"\n\n".to_vec()].concat();
        Mock::given(method("POST"))
            .and(path("/chat/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&server)
            .await;

        let client = MimirClient::new(server.uri());
        let mut stream = client
            .chat_stream(ChatRequest {
                session_id: None,
                message: "hi".to_string(),
                model: None,
                personality_preset: None,
                incognito: None,
            })
            .await
            .unwrap();
        let item = stream.next().await.unwrap();
        assert!(matches!(item, Err(ClientError::Connection(_))));
    }

    #[tokio::test]
    async fn test_chat_stream_survives_response_slower_than_default_total_timeout() {
        // Issue #487: the client's default 120s total request timeout killed
        // long streaming responses (e.g. a query that runs the retrieval agent
        // for a minute and then streams a long answer). The chat endpoints
        // must override the total timeout per request.
        let server = MockServer::start().await;
        let body = "data: hello\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .set_delay(Duration::from_secs(2)),
            )
            .mount(&server)
            .await;

        // A client whose default total timeout (1s) is far shorter than the
        // server's response delay.
        let client =
            MimirClient::try_new(server.uri(), Duration::from_secs(1), Duration::from_secs(1))
                .unwrap();
        let mut stream = client
            .chat_stream(ChatRequest {
                session_id: None,
                message: "hello".to_string(),
                model: None,
                personality_preset: None,
                incognito: None,
            })
            .await
            .unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, StreamItem::Text("hello".to_string()));
    }

    #[tokio::test]
    async fn test_chat_survives_response_slower_than_default_total_timeout() {
        // Issue #487: the non-streaming chat endpoint must also override the
        // default total timeout — the agentic loop can run the retrieval
        // agent for a minute or more before the final response is ready.
        let server = MockServer::start().await;
        let resp = ChatResponse {
            session_id: 1,
            response: "hi".to_string(),
            usage: Usage::default(),
            tool_calls: vec![],
        };
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&resp)
                    .set_delay(Duration::from_secs(2)),
            )
            .mount(&server)
            .await;

        let client =
            MimirClient::try_new(server.uri(), Duration::from_secs(1), Duration::from_secs(1))
                .unwrap();
        let result = client
            .chat(ChatRequest {
                session_id: None,
                message: "hello".to_string(),
                model: None,
                personality_preset: None,
                incognito: None,
            })
            .await
            .unwrap();
        assert_eq!(result.response, "hi");
    }

    #[tokio::test]
    async fn test_chat_stream_read_timeout_errors_on_silent_stream() {
        // One chunk arrives, then the stream goes silent: the per-chunk read
        // timeout must surface a Connection error instead of hanging forever.
        let silent = futures::stream::iter(vec![Ok(bytes::Bytes::from_static(b"data: hello\n\n"))])
            .chain(futures::stream::pending::<Result<bytes::Bytes, ClientError>>());
        let mut timed = with_read_timeout(silent, Duration::from_millis(50));
        let first = timed.next().await.unwrap().unwrap();
        assert_eq!(first, bytes::Bytes::from_static(b"data: hello\n\n"));
        let second = timed.next().await.unwrap();
        assert!(
            matches!(second, Err(ClientError::Connection(ref m)) if m.contains("timed out")),
            "unexpected item: {second:?}"
        );
    }

    #[tokio::test]
    async fn test_chat_stream_read_timeout_resets_on_each_chunk() {
        // Chunks arriving faster than the read timeout (like the daemon's
        // keep-alive comments) must not trip it.
        let chunks = tokio_stream::StreamExt::throttle(
            futures::stream::iter(vec![
                Ok(bytes::Bytes::from_static(b"data: a\n\n")),
                Ok(bytes::Bytes::from_static(b"data: b\n\n")),
                Ok(bytes::Bytes::from_static(b"data: c\n\n")),
            ]),
            Duration::from_millis(20),
        );
        let mut timed = with_read_timeout(chunks, Duration::from_millis(50));
        let mut count = 0;
        while let Some(Ok(_)) = timed.next().await {
            count += 1;
        }
        assert_eq!(count, 3);
    }
}
