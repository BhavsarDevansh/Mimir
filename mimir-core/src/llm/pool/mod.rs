//! Priority worker pool for LLM requests.
//!
//! Types and the pool struct live here; enqueue + introspection methods in
//! `queue`, worker-task lifecycle in `worker`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::llm::types::Job;
use tokio::sync::{Mutex, Notify, watch};

mod queue;
#[cfg(test)]
mod tests;
mod worker;

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
    /// Test-only sequence of jobs claimed by workers.
    #[cfg(test)]
    job_starts: watch::Sender<u64>,
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
/// When both queues are full, enqueuing returns [`LlmError::QueueFull`](crate::llm::types::LlmError::QueueFull).
#[derive(Clone)]
pub struct LlmWorkerPool {
    inner: Arc<PoolInner>,
    config: WorkerPoolConfig,
}
