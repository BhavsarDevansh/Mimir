use futures::StreamExt;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::Stream;
use tokio::sync::{Mutex, Notify, mpsc, oneshot, watch};
use tracing::debug;

use crate::config::LlmConfig;
use crate::llm::client::LlmClient;
use crate::llm::types::{Job, LlmError, Message, StreamItem, Usage};

/// Configuration for the LLM worker pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerPoolConfig {
    /// Number of worker tasks that process jobs concurrently.
    pub worker_threads: u8,
    /// Maximum number of user-facing jobs that can be queued.
    pub user_queue_size: u16,
    /// Maximum number of system jobs that can be queued.
    pub system_queue_size: u16,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            worker_threads: 1,
            user_queue_size: 100,
            system_queue_size: 100,
        }
    }
}

/// Internal shared state for the worker pool.
struct PoolInner {
    user_queue: Mutex<VecDeque<Job>>,
    system_queue: Mutex<VecDeque<Job>>,
    notify: Notify,
    shutdown_tx: watch::Sender<bool>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    in_flight: AtomicUsize,
}

/// Guard that increments `in_flight` on creation and decrements on drop.
struct InFlightGuard<'a>(&'a AtomicUsize);

impl<'a> InFlightGuard<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl<'a> Drop for InFlightGuard<'a> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A priority-based worker pool for LLM requests.
///
/// Maintains two bounded queues — user (highest priority) and system.
/// Workers always drain the user queue before servicing system jobs.
/// When both queues are full, enqueuing returns [`LlmError::QueueFull`].
#[derive(Clone)]
pub struct LlmWorkerPool {
    inner: Arc<PoolInner>,
    config: WorkerPoolConfig,
}

impl LlmWorkerPool {
    /// Create a new worker pool with the given LLM configuration and pool settings.
    ///
    /// Spawns `worker_threads` background tasks that consume jobs from the queues.
    ///
    /// # Async
    ///
    /// Must be called from within a Tokio runtime context because it spawns
    /// the background worker tasks via [`tokio::spawn`].
    pub async fn new(llm_config: LlmConfig, config: WorkerPoolConfig) -> Result<Self, String> {
        if config.worker_threads == 0 {
            return Err("WorkerPoolConfig.worker_threads must be > 0".to_string());
        }

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let inner = Arc::new(PoolInner {
            user_queue: Mutex::new(VecDeque::new()),
            system_queue: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            shutdown_tx,
            handles: Mutex::new(Vec::new()),
            in_flight: AtomicUsize::new(0),
        });

        // Build every worker's HTTP client up front so a construction failure
        // aborts pool creation *before* any worker task is spawned. Spawning a
        // worker and then failing on a later iteration would leave earlier
        // workers detached with no `LlmWorkerPool` handle to signal shutdown
        // (PR #177 review: avoid leaking partially-started workers on
        // constructor failure).
        let mut clients = Vec::with_capacity(config.worker_threads as usize);
        for i in 0..config.worker_threads {
            clients.push(
                LlmClient::new_direct(llm_config.clone())
                    .map_err(|e| format!("LLM worker {i} failed to build HTTP client: {e}"))?,
            );
        }

        for (i, client) in clients.into_iter().enumerate() {
            let inner_spawn = Arc::clone(&inner);
            let mut shutdown_rx = inner_spawn.shutdown_tx.subscribe();
            let handle = tokio::spawn(async move {
                debug!(worker_id = i, "LLM worker started");
                loop {
                    tokio::select! {
                        biased;
                        result = shutdown_rx.changed() => {
                            if result.is_err() || *shutdown_rx.borrow() {
                                debug!(worker_id = i, "LLM worker shutting down");
                                break;
                            }
                        }
                        job = Self::next_job(&inner_spawn) => {
                            if let Some(job) = job {
                                let _guard = InFlightGuard::new(&inner_spawn.in_flight);
                                Self::process_job(&client, job).await;
                            }
                        }
                    }
                }
            });
            inner.handles.lock().await.push(handle);
        }

        Ok(Self { inner, config })
    }

    /// Enqueue a non-streaming chat job to the **user** queue.
    ///
    /// Returns the assistant response and token usage when the job completes.
    /// Returns [`LlmError::QueueFull`] if the user queue is at capacity.
    /// Enqueue a non-streaming chat job to the **user** queue and return the full message.
    pub async fn enqueue_chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(Message, Usage), LlmError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut queue = self.inner.user_queue.lock().await;
            if queue.len() >= self.config.user_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::Chat {
                messages,
                tools,
                respond: tx,
            });
        }
        self.inner.notify.notify_one();
        rx.await
            .map_err(|_| LlmError::StreamError("worker pool closed".to_string()))?
    }

    /// Enqueue a non-streaming chat job to the **user** queue.
    ///
    /// Returns the assistant response text and token usage when the job completes.
    /// Returns [`LlmError::QueueFull`] if the user queue is at capacity.
    pub async fn enqueue_chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<(String, Usage), LlmError> {
        let (msg, usage) = self.enqueue_chat_message(messages, tools).await?;
        Ok((msg.content, usage))
    }

    /// Enqueue a streaming chat job to the **user** queue.
    ///
    /// Returns a stream that yields [`StreamItem`] chunks as the worker
    /// receives them from the LLM.
    /// Returns [`LlmError::QueueFull`] if the user queue is at capacity.
    pub async fn enqueue_chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        let (tx, rx) = mpsc::channel::<Result<StreamItem, LlmError>>(64);
        {
            let mut queue = self.inner.user_queue.lock().await;
            if queue.len() >= self.config.user_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::ChatStream {
                messages,
                tools,
                respond: tx,
            });
        }
        self.inner.notify.notify_one();

        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    /// Enqueue a non-streaming chat job to the **system** queue.
    /// Enqueue a non-streaming chat job to the **system** queue and return the full message.
    pub async fn enqueue_system_chat_message(
        &self,
        messages: Vec<Message>,
    ) -> Result<(Message, Usage), LlmError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut queue = self.inner.system_queue.lock().await;
            if queue.len() >= self.config.system_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::Chat {
                messages,
                tools: None,
                respond: tx,
            });
        }
        self.inner.notify.notify_one();
        rx.await
            .map_err(|_| LlmError::StreamError("worker pool closed".to_string()))?
    }

    /// Enqueue a non-streaming chat job to the **system** queue.
    pub async fn enqueue_system_chat(
        &self,
        messages: Vec<Message>,
    ) -> Result<(String, Usage), LlmError> {
        let (msg, usage) = self.enqueue_system_chat_message(messages).await?;
        Ok((msg.content, usage))
    }

    /// Enqueue a streaming chat job to the **system** queue.
    pub async fn enqueue_system_chat_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        let (tx, rx) = mpsc::channel::<Result<StreamItem, LlmError>>(64);
        {
            let mut queue = self.inner.system_queue.lock().await;
            if queue.len() >= self.config.system_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::ChatStream {
                messages,
                tools: None,
                respond: tx,
            });
        }
        self.inner.notify.notify_one();

        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(stream))
    }

    /// Current depth of the user queue.
    pub async fn user_queue_depth(&self) -> usize {
        self.inner.user_queue.lock().await.len()
    }

    /// Current depth of the system queue.
    pub async fn system_queue_depth(&self) -> usize {
        self.inner.system_queue.lock().await.len()
    }

    /// Number of jobs currently being processed by workers.
    pub fn in_flight_count(&self) -> usize {
        self.inner.in_flight.load(Ordering::Relaxed)
    }

    /// Wait for the next available job, prioritising user queue over system queue.
    async fn next_job(inner: &Arc<PoolInner>) -> Option<Job> {
        loop {
            // Prioritise user jobs
            {
                let mut user = inner.user_queue.lock().await;
                if let Some(job) = user.pop_front() {
                    return Some(job);
                }
            }
            // Fall back to system jobs
            {
                let mut system = inner.system_queue.lock().await;
                if let Some(job) = system.pop_front() {
                    return Some(job);
                }
            }
            // Both queues empty — wait for a notification.
            inner.notify.notified().await;
        }
    }

    /// Process a single job using the provided direct LLM client.
    async fn process_job(client: &LlmClient, job: Job) {
        match job {
            Job::Chat {
                messages,
                tools,
                respond,
            } => {
                let result = client.chat_message_direct(messages, tools).await;
                let _ = respond.send(result);
            }
            Job::ChatStream {
                messages,
                tools,
                respond,
            } => {
                match client.chat_stream_with_usage_direct(messages, tools).await {
                    Ok(mut stream) => {
                        while let Some(item) = stream.next().await {
                            if respond.send(item).await.is_err() {
                                // Receiver dropped — stop streaming.
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = respond.send(Err(e)).await;
                    }
                }
            }
        }
    }
}

// Accessor methods for runtime introspection
impl LlmWorkerPool {
    /// Number of worker threads configured for this pool.
    pub fn worker_threads(&self) -> u8 {
        self.config.worker_threads
    }

    /// Returns `true` if the user queue has capacity for at least one more job.
    ///
    /// This is a best-effort check — a concurrent enqueue can fill the gap before
    /// the caller actually enqueues.
    pub async fn user_queue_has_capacity(&self) -> bool {
        let len = self.inner.user_queue.lock().await.len();
        len < self.config.user_queue_size as usize
    }

    /// Signal all workers to stop and wait for them to finish.
    ///
    /// Each worker is given a 5-second timeout to complete its current job
    /// before the task is aborted.
    pub async fn shutdown(&self) {
        let _ = self.inner.shutdown_tx.send(true);
        let mut handles = self.inner.handles.lock().await;
        for handle in handles.drain(..) {
            let abort_handle = handle.abort_handle();
            match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => debug!("worker panicked: {}", e),
                Err(_) => {
                    debug!("worker shutdown timed out, aborting");
                    abort_handle.abort();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::Message;
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
        let system_job = pool.enqueue_system_chat(vec![Message::system("system-first")]);
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
}
