//! Worker pool tests.

use super::*;
use crate::config::LlmConfig;
use crate::llm::client::RetryConfig;
use crate::llm::types::{LlmError, LlmRequestOverrides, Message, StreamItem};
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Duration, timeout};

async fn wait_for_job_started(pool: &LlmWorkerPool) {
    timeout(Duration::from_secs(5), pool.inner.job_started.notified())
        .await
        .expect("worker must start a job");
}

/// Read a complete HTTP request (headers + `Content-Length` body) from a mock
/// server socket so JSON parsing never sees a partially delivered body (PR #477 review).
async fn read_complete_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk).await.expect("read request headers");
        assert!(n > 0, "connection closed before request headers arrived");
        buf.extend_from_slice(&chunk[..n]);
    };
    let content_length = String::from_utf8_lossy(&buf[..header_end])
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk).await.expect("read request body");
        assert!(n > 0, "connection closed before the request body arrived");
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn test_config() -> LlmConfig {
    LlmConfig {
        endpoint: "http://127.0.0.1:1".to_string(),
        api_key: "test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.0,
    }
}

fn tiny_pool_config() -> WorkerPoolConfig {
    WorkerPoolConfig {
        worker_threads: 1,
        user_queue_size: 2,
        system_queue_size: 2,
    }
}

#[tokio::test]
async fn test_pool_enqueues_chat_job() {
    let pool = LlmWorkerPool::new(test_config(), tiny_pool_config(), RetryConfig::default())
        .await
        .unwrap();

    // This will fail with a network error, but it proves the job was
    // dequeued and processed by the worker.
    let result = pool.enqueue_chat(vec![Message::user("hello")], None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_pool_user_priority_over_system() {
    // Use a mock server so jobs actually complete and we can observe order.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        let mut order = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_complete_request(&mut stream).await;
            if req.contains("system-first") {
                order.push("system");
            } else if req.contains("user-second") {
                order.push("user");
            }

            // Write a minimal HTTP JSON response
            let body = r#"{"id":"1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
        order
    });

    let config = LlmConfig {
        endpoint: format!("http://{}/v1", addr),
        api_key: "test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.0,
    };

    let pool = LlmWorkerPool::new(config, tiny_pool_config(), RetryConfig::default())
        .await
        .unwrap();

    // Enqueue system first — it should sit in the system queue.
    let system_job = pool.enqueue_system_chat(vec![Message::system("system-first")], None);
    // Enqueue user second — it should jump ahead.
    let user_job = pool.enqueue_chat(vec![Message::user("user-second")], None);

    let (sys_res, usr_res) = tokio::join!(system_job, user_job);
    assert!(sys_res.is_ok());
    assert!(usr_res.is_ok());

    let order = server_handle.await.unwrap();
    // Because the worker drains user queue first, the user job (enqueued second)
    // should complete before the system job.
    assert_eq!(order, vec!["user", "system"]);
}

#[tokio::test]
async fn test_pool_queue_full_returns_error() {
    let mut config = tiny_pool_config();
    config.user_queue_size = 0;
    config.system_queue_size = 0;

    let pool = LlmWorkerPool::new(test_config(), config, RetryConfig::default())
        .await
        .unwrap();

    let result = pool
        .enqueue_chat(vec![Message::user("overflow")], None)
        .await;
    assert!(matches!(result, Err(LlmError::QueueFull)));
}
#[tokio::test]
async fn test_pool_stream_yields_text_and_usage() {
    // Build a minimal HTTP server that returns SSE chunks.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_complete_request(&mut stream).await;
        assert!(req.contains("/chat/completions"));

        let sse_body = format!(
            "data: {}\n\ndata: {}\n\n",
            r#"{"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
            r#"{"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
            sse_body.len(),
            sse_body
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });

    let config = LlmConfig {
        endpoint: format!("http://{}/v1", addr),
        api_key: "test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.0,
    };

    let pool = LlmWorkerPool::new(config, tiny_pool_config(), RetryConfig::default())
        .await
        .unwrap();

    let mut stream = pool
        .enqueue_chat_stream(vec![Message::user("hello")], None)
        .await
        .unwrap();

    let mut items = Vec::new();
    while let Some(item) = stream.next().await {
        items.push(item.unwrap());
    }

    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], StreamItem::Text(t) if t == "Hello"));
    assert!(matches!(&items[1], StreamItem::Usage(u) if u.total_tokens == 4));
}

#[tokio::test]
async fn test_pool_job_applies_request_overrides() {
    // Issue #465: per-request overrides carried by the job must reach the
    // upstream request, so override clones can stay on the shared pool.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_complete_request(&mut stream).await;
        let body = req.split("\r\n\r\n").nth(1).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["model"], "override-model");
        assert_eq!(json["temperature"], 0.9);
        assert_eq!(json["max_tokens"], 77);

        let body = r#"{"id":"1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });

    let config = LlmConfig {
        endpoint: format!("http://{}/v1", addr),
        api_key: "test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.2,
    };

    let pool = LlmWorkerPool::new(config, tiny_pool_config(), RetryConfig::default())
        .await
        .unwrap();

    let overrides = LlmRequestOverrides {
        model: Some("override-model".to_string()),
        temperature: Some(0.9),
        max_tokens: Some(77),
    };
    let result = pool
        .enqueue_chat_with_overrides(vec![Message::user("hello")], None, overrides)
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_worker_pool_shutdown() {
    let pool = LlmWorkerPool::new(test_config(), tiny_pool_config(), RetryConfig::default())
        .await
        .unwrap();

    pool.shutdown().await;

    // After shutdown, enqueuing should still work (queues are not cleared),
    // but the workers have exited. We verify by checking that a second
    // shutdown is a no-op (no handles left to await).
    pool.shutdown().await;
}

#[tokio::test]
async fn test_pool_spawns_exactly_configured_workers() {
    // PR #177 review: a successful `LlmWorkerPool::new` must spawn exactly
    // `worker_threads` worker tasks and register one handle per worker, so
    // a construction failure can never leave spawned workers detached.
    // All worker clients are built up front before any task is spawned.
    let config = WorkerPoolConfig {
        worker_threads: 3,
        user_queue_size: 4,
        system_queue_size: 4,
    };
    let pool = LlmWorkerPool::new(test_config(), config, RetryConfig::default())
        .await
        .expect("pool must build with a valid config");

    assert_eq!(pool.worker_threads(), 3);
    let handle_count = pool.inner.handles.lock().await.len();
    assert_eq!(handle_count, 3, "expected exactly 3 worker handles");

    pool.shutdown().await;
    let after = pool.inner.handles.lock().await.len();
    assert_eq!(after, 0, "shutdown must drain all worker handles");
}

#[tokio::test]
async fn test_in_flight_counter_tracks_active_jobs() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_complete_request(&mut stream).await;
        assert!(req.contains("/chat/completions"));

        let body = r#"{"id":"1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });

    let config = LlmConfig {
        endpoint: format!("http://{}/v1", addr),
        api_key: "test".to_string(),
        model: "gpt-4o".to_string(),
        max_tokens: Some(10),
        temperature: 0.0,
    };

    let pool = LlmWorkerPool::new(config, tiny_pool_config(), RetryConfig::default())
        .await
        .unwrap();

    // Spawn the enqueue so it actually enters the queue while we observe.
    let pool_clone = pool.clone();
    let job = tokio::spawn(async move {
        pool_clone
            .enqueue_chat(vec![Message::user("hello")], None)
            .await
    });

    wait_for_job_started(&pool).await;
    assert_eq!(pool.in_flight_count(), 1);

    // Wait for the job to complete.
    let _ = job.await.unwrap();

    assert_eq!(
        pool.in_flight_count(),
        0,
        "expected in_flight_count to be 0 after completion"
    );
}
