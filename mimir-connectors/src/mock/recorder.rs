//! Sync recorder + guard used by supervisor lifecycle tests.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::connector::SyncOptions;

#[derive(Debug, Default)]
pub struct MockSyncRecorder {
    recorded: std::sync::Mutex<Vec<SyncOptions>>,
    in_flight: AtomicU32,
    max_concurrent: AtomicU32,
    completion: tokio::sync::Notify,
}

impl MockSyncRecorder {
    /// Number of `sync()` calls recorded.
    pub fn len(&self) -> usize {
        self.recorded.lock().expect("recorder lock poisoned").len()
    }

    /// Whether no `sync()` calls have been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The [`SyncOptions`] received by the most recent `sync()`, if any.
    pub fn last(&self) -> Option<SyncOptions> {
        self.recorded
            .lock()
            .expect("recorder lock poisoned")
            .last()
            .copied()
    }

    /// Peak number of concurrently in-flight `sync()` calls observed.
    pub fn max_concurrent(&self) -> u32 {
        self.max_concurrent.load(Ordering::SeqCst)
    }

    /// Wait until `completed_calls` `sync()` calls have returned.
    pub async fn wait_for_completed(&self, completed_calls: usize) {
        let notified = self.completion.notified();
        tokio::pin!(notified);
        while self.len() < completed_calls {
            notified.as_mut().await;
            notified.set(self.completion.notified());
        }
    }

    /// Enter a `sync()` call, returning an RAII guard that records the
    /// [`SyncOptions`] and decrements the in-flight counter on [`Drop`].
    ///
    /// The guard is cancellation-, panic-, and failure-safe: it must be
    /// created *before* the first `.await` of `sync()` and is dropped when
    /// the call ends — whether by returning, unwinding on a panic, or having
    /// its task aborted by the supervisor. This guarantees `in_flight` is
    /// always balanced and that *every* call (including injected failures and
    /// panics) is recorded, rather than only successful post-delay calls.
    pub(super) fn enter(&self, options: SyncOptions) -> MockSyncGuard<'_> {
        let prev = self.in_flight.fetch_add(1, Ordering::SeqCst);
        self.max_concurrent.fetch_max(prev + 1, Ordering::SeqCst);
        MockSyncGuard {
            recorder: self,
            options,
        }
    }
}

/// RAII guard returned by `MockSyncRecorder::enter`.
///
/// [`Drop`] records the captured [`SyncOptions`] and decrements the
/// recorder's in-flight counter, so `sync()` tracking stays balanced across
pub struct MockSyncGuard<'a> {
    recorder: &'a MockSyncRecorder,
    options: SyncOptions,
}

impl Drop for MockSyncGuard<'_> {
    fn drop(&mut self) {
        self.recorder.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.recorder
            .recorded
            .lock()
            .expect("recorder lock poisoned")
            .push(self.options);
        self.recorder.completion.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completion_wait_observes_guard_drop() {
        let recorder = MockSyncRecorder::default();
        let wait = recorder.wait_for_completed(1);
        tokio::pin!(wait);

        let guard = recorder.enter(SyncOptions::default());
        drop(guard);

        wait.await;
        assert_eq!(recorder.len(), 1);
        assert_eq!(recorder.max_concurrent(), 1);
    }
}

// ---------------------------------------------------------------------------
// MockConnector
// ---------------------------------------------------------------------------
