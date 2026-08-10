//! Worker pool tests.

use super::*;
use crate::config::LlmConfig;
use crate::llm::types::{LlmError, Message, StreamItem};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::time::Duration;

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
    let pool = LlmWorkerPool::new(test_config(), tiny_pool_config())
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
            let mut buf = [0u8; 1024];
            let n = stream.peek(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
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

    let pool = LlmWorkerPool::new(config, tiny_pool_config())
        .await
        .unwrap();

    // Enqueue system first — it should sit in the system queue.
    let system_job = pool.enqueue_system_chat(vec![Message::system("system-first")], None);
    // Give the worker a moment to pick up the system job if it were to.
    tokio::time::sleep(Duration::from_millis(50)).await;

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

    let pool = LlmWorkerPool::new(test_config(), config).await.unwrap();

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
        let mut buf = [0u8; 2048];
        let n = stream.peek(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]);
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

    let pool = LlmWorkerPool::new(config, tiny_pool_config())
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
async fn test_worker_pool_shutdown() {
    let pool = LlmWorkerPool::new(test_config(), tiny_pool_config())
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
    let pool = LlmWorkerPool::new(test_config(), config)
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
        let mut buf = [0u8; 1024];
        let n = stream.peek(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]);
        assert!(req.contains("/chat/completions"));

        // Sleep while "processing" so the counter stays elevated.
        tokio::time::sleep(Duration::from_millis(200)).await;

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

    let pool = LlmWorkerPool::new(config, tiny_pool_config())
        .await
        .unwrap();

    // Spawn the enqueue so it actually enters the queue while we observe.
    let pool_clone = pool.clone();
    let job = tokio::spawn(async move {
        pool_clone
            .enqueue_chat(vec![Message::user("hello")], None)
            .await
    });

    // Poll until in_flight becomes 1 (job picked up by worker).
    let mut found_in_flight = false;
    for _ in 0..100 {
        let count = pool.in_flight_count();
        if count == 1 {
            found_in_flight = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(found_in_flight, "expected in_flight_count to reach 1");

    // Wait for the job to complete.
    let _ = job.await.unwrap();

    assert_eq!(
        pool.in_flight_count(),
        0,
        "expected in_flight_count to be 0 after completion"
    );
}
