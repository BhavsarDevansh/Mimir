use std::sync::atomic::Ordering;

use notify::{Config, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventHandler, DebounceEventResult, Debouncer, RecommendedCache, new_debouncer_opt,
};
use tokio::sync::mpsc::unbounded_channel;

use crate::connector::ConnectorError;
use crate::photos::connector::PhotosConnector;

// ---------------------------------------------------------------------------
// Watcher plumbing
// ---------------------------------------------------------------------------

pub(super) type PhotosDebouncer = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

/// `DebounceEventHandler` that forwards debounced events onto a tokio
/// unbounded channel. `notify-debouncer-full`'s stable line has no tokio
/// feature, so the handler drives the channel with a synchronous `send` (the
/// debouncer runs the handler on its own thread); the connector awaits the
/// receiver inside `sync()`.
struct DebounceForwarder(tokio::sync::mpsc::UnboundedSender<DebounceEventResult>);

impl DebounceEventHandler for DebounceForwarder {
    fn handle_event(&mut self, event: DebounceEventResult) {
        // Best-effort: if the connector was dropped the receiver is gone, which
        // is a clean shutdown — swallow the send error.
        let _ = self.0.send(event);
    }
}

impl PhotosConnector {
    /// Start the debounced watcher (idempotent). Subsequent calls are no-ops
    /// once a watcher is installed.
    ///
    /// `started` is only flipped *after* the debouncer is created and the
    /// recursive watch is registered, so a failed init (e.g. inotify watch
    /// limits, or the watch dir vanishing before the first `sync`) leaves
    /// `started == false`. The supervisor's retry then re-runs setup instead
    /// of no-op'ing and busy-looping on the construction-time (closed) event
    /// channel. The connector is driven by a single runner task, so the
    /// load/store pair is race-free.
    pub(super) async fn start_watcher(&self) -> Result<(), ConnectorError> {
        if self.started.load(Ordering::SeqCst) {
            return Ok(());
        }
        let (tx, rx) = unbounded_channel::<DebounceEventResult>();
        let mut debouncer =
            new_debouncer_opt::<DebounceForwarder, notify::RecommendedWatcher, RecommendedCache>(
                self.debounce,
                None,
                DebounceForwarder(tx),
                RecommendedCache::new(),
                Config::default(),
            )
            .map_err(|error| ConnectorError::Other(format!("failed to create watcher: {error}")))?;
        debouncer
            .watch(&self.watch_dir, RecursiveMode::Recursive)
            .map_err(|error| {
                ConnectorError::Config(format!(
                    "failed to watch {}: {error}",
                    self.watch_dir.display()
                ))
            })?;
        // Fully installed before flipping the flag.
        *self.events.lock().await = rx;
        *self.watcher.lock().await = Some(debouncer);
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }
}
