use futures::StreamExt;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
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
    pub async fn new(
        llm_config: LlmConfig,
        config: WorkerPoolConfig,
    ) -> Result<Self, &'static str> {
        if config.worker_threads == 0 {
            return Err("WorkerPoolConfig.worker_threads must be > 0");
        }

        let inner = Arc::new(PoolInner {
            user_queue: Mutex::new(VecDeque::new()),
            system_queue: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
        });

        for i in 0..config.worker_threads {
            let inner = Arc::clone(&inner);
            let llm_config = llm_config.clone();
            tokio::spawn(async move {
                let client = LlmClient::new_direct(llm_config);
                debug!(worker_id = i, "LLM worker started");
                loop {
                    if let Some(job) = Self::next_job(&inner).await {
                        Self::process_job(&client, job).await;
                    }
                }
            });
        }

        Ok(Self { inner, config })
    }

    /// Enqueue a non-streaming chat job to the **user** queue.
    ///
    /// Returns the assistant response and token usage when the job completes.
    /// Returns [`LlmError::QueueFull`] if the user queue is at capacity.
    pub async fn enqueue_chat(&self, messages: Vec<Message>) -> Result<(String, Usage), LlmError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut queue = self.inner.user_queue.lock().await;
            if queue.len() >= self.config.user_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::Chat {
                messages,
                respond: tx,
            });
        }
        self.inner.notify.notify_one();
        rx.await
            .map_err(|_| LlmError::StreamError("worker pool closed".to_string()))?
    }

    /// Enqueue a streaming chat job to the **user** queue.
    ///
    /// Returns a stream that yields [`StreamItem`] chunks as the worker
    /// receives them from the LLM.
    /// Returns [`LlmError::QueueFull`] if the user queue is at capacity.
    pub async fn enqueue_chat_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        let (tx, rx) = mpsc::channel::<Result<StreamItem, LlmError>>(64);
        {
            let mut queue = self.inner.user_queue.lock().await;
            if queue.len() >= self.config.user_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::ChatStream {
                messages,
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
    pub async fn enqueue_system_chat(
        &self,
        messages: Vec<Message>,
    ) -> Result<(String, Usage), LlmError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut queue = self.inner.system_queue.lock().await;
            if queue.len() >= self.config.system_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::Chat {
                messages,
                respond: tx,
            });
        }
        self.inner.notify.notify_one();
        rx.await
            .map_err(|_| LlmError::StreamError("worker pool closed".to_string()))?
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
            Job::Chat { messages, respond } => {
                let result = client.chat_direct(messages).await;
                let _ = respond.send(result);
            }
            Job::ChatStream { messages, respond } => {
                match client.chat_stream_with_usage_direct(messages).await {
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
        let result = pool.enqueue_chat(vec![Message::user("hello")]).await;
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
        let user_job = pool.enqueue_chat(vec![Message::user("user-second")]);

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

        let result = pool.enqueue_chat(vec![Message::user("overflow")]).await;
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
            .enqueue_chat_stream(vec![Message::user("hello")])
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
}
