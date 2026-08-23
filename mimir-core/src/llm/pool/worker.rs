//! Worker task lifecycle: pool construction and job dispatch.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use futures::StreamExt;
use tokio::sync::{Mutex, Notify, watch};
use tracing::debug;

use crate::config::LlmConfig;
use crate::llm::client::LlmClient;
use crate::llm::pool::{InFlightGuard, LlmWorkerPool, PoolInner, WorkerPoolConfig};
use crate::llm::types::Job;

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
}

impl LlmWorkerPool {
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
                overrides,
                respond,
            } => {
                let result = client
                    .chat_message_direct(messages, tools, &overrides)
                    .await;
                let _ = respond.send(result);
            }
            Job::ChatStream {
                messages,
                tools,
                overrides,
                respond,
            } => {
                match client
                    .chat_stream_with_usage_direct(messages, tools, &overrides)
                    .await
                {
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
