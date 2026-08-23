//! Job enqueue + runtime introspection for the worker pool.

use std::pin::Pin;
use std::sync::atomic::Ordering;

use futures::Stream;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

use crate::llm::pool::LlmWorkerPool;
use crate::llm::types::{Job, LlmError, LlmRequestOverrides, Message, StreamItem, Usage};

impl LlmWorkerPool {
    /// Enqueue a non-streaming chat job to the **user** queue.
    ///
    /// Returns the assistant response and token usage when the job completes.
    /// Returns [`LlmError::QueueFull`] if the user queue is at capacity.
    pub async fn enqueue_chat_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
        overrides: LlmRequestOverrides,
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
                overrides,
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
        overrides: LlmRequestOverrides,
    ) -> Result<(String, Usage), LlmError> {
        let (msg, usage) = self
            .enqueue_chat_message(messages, tools, overrides)
            .await?;
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
        overrides: LlmRequestOverrides,
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
                overrides,
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
        tools: Option<Vec<serde_json::Value>>,
        overrides: LlmRequestOverrides,
    ) -> Result<(Message, Usage), LlmError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut queue = self.inner.system_queue.lock().await;
            if queue.len() >= self.config.system_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::Chat {
                messages,
                tools,
                overrides,
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
        tools: Option<Vec<serde_json::Value>>,
        overrides: LlmRequestOverrides,
    ) -> Result<(String, Usage), LlmError> {
        let (msg, usage) = self
            .enqueue_system_chat_message(messages, tools, overrides)
            .await?;
        Ok((msg.content, usage))
    }

    /// Enqueue a streaming chat job to the **system** queue.
    pub async fn enqueue_system_chat_stream(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
        overrides: LlmRequestOverrides,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamItem, LlmError>> + Send>>, LlmError> {
        let (tx, rx) = mpsc::channel::<Result<StreamItem, LlmError>>(64);
        {
            let mut queue = self.inner.system_queue.lock().await;
            if queue.len() >= self.config.system_queue_size as usize {
                return Err(LlmError::QueueFull);
            }
            queue.push_back(Job::ChatStream {
                messages,
                tools,
                overrides,
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
}

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
