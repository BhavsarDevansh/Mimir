use futures::Stream;

#[cfg(test)]
use futures::StreamExt;

use mimir_api_types::{ChatRequest, ChatResponse, StreamItem};

use crate::MimirClient;
use crate::error::ClientError;
use crate::sse::parse_sse_stream;

impl MimirClient {
    /// Send a non-streaming chat request.
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ClientError> {
        self.post_json(&self.url("chat"), &req).await
    }

    /// Send a streaming chat request and return an SSE item stream.
    pub async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<impl Stream<Item = Result<StreamItem, ClientError>>, ClientError> {
        let resp =
            Self::send_response(self.client.post(self.url("chat/stream")).json(&req)).await?;
        Ok(parse_sse_stream(resp.bytes_stream()))
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
}
