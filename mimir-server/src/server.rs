//! Daemon lifecycle: state initialisation, background tasks, and server startup.
#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use mimir_core::config::ReloadableConfig;
use mimir_core::llm::{LlmBackend, LlmClient};
use tracing::info;

use crate::app::build_app;
use crate::shutdown::{GRACEFUL_DRAIN_TIMEOUT, serve_with_bounded_drain};
use crate::state::AppState;

///
/// Loads shared state from `config`, binds to `config.server.bind_addr`,
/// and runs until the process is terminated or a graceful shutdown is
/// triggered via the `/stop` endpoint, Ctrl-C, or SIGTERM.
///
/// If the server does not shut down gracefully within 30 seconds, it is
/// forcefully aborted so that resource cleanup can still run.
pub async fn start_server(config: Arc<ReloadableConfig>) -> anyhow::Result<()> {
    let api_token = Arc::from(mimir_core::auth::load_or_create_api_token()?.as_str());
    let llm_client: Arc<dyn LlmBackend> =
        Arc::new(LlmClient::new(config.snapshot().await.llm.clone()).await?);
    start_server_with_llm(config, llm_client, api_token).await
}

/// Start the Mimir HTTP server with an injected LLM backend and API token.
///
/// This is the same as [`start_server`], but allows tests (and future
/// embedders) to supply a custom [`LlmBackend`] implementation and a known
/// API token without relying on sentinel strings or config hacks.
pub async fn start_server_with_llm(
    config: Arc<ReloadableConfig>,
    llm_client: Arc<dyn LlmBackend>,
    api_token: Arc<str>,
) -> anyhow::Result<()> {
    let bind_addr = config.snapshot().await.server.bind_addr.clone();
    let addr: SocketAddr = bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    start_server_with_llm_and_listener(config, llm_client, listener, api_token).await
}

/// Start the Mimir HTTP server with an injected LLM backend and a pre-bound listener.
///
/// This is the same as [`start_server_with_llm`], but allows tests to supply
/// a pre-bound [`TcpListener`](tokio::net::TcpListener) so the bound port is known before the server
/// starts accepting connections.
pub async fn start_server_with_llm_and_listener(
    config: Arc<ReloadableConfig>,
    llm_client: Arc<dyn LlmBackend>,
    listener: tokio::net::TcpListener,
    api_token: Arc<str>,
) -> anyhow::Result<()> {
    let (app_state, scheduler_shutdown_rx) =
        AppState::from_config_with_llm(Arc::clone(&config), llm_client, api_token).await?;
    let state = Arc::new(app_state);

    // A non-loopback bind exposes the API to the network; the bearer token is
    // then the only access control (issue #281). Warn so the operator knows.
    if let Ok(local_addr) = listener.local_addr()
        && !local_addr.ip().is_loopback()
    {
        tracing::warn!(
            "bind_addr {local_addr} is not loopback: the API token is the only access control; see docs/api-authentication.md"
        );
    }

    // ---- Start background scheduler dispatch loop ----
    let sched = Arc::clone(&state.scheduler);
    let sched_shutdown_rx = scheduler_shutdown_rx;
    tokio::spawn(async move {
        sched.start(sched_shutdown_rx).await;
    });

    // ---- Listen for KG dirty signal and submit condensation ----
    if state.user_entity_id.is_some() {
        let notify = state.knowledge_graph.condensation_notify();
        let sched = Arc::clone(&state.scheduler);
        let mut shutdown_rx = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = notify.notified() => {
                        use mimir_core::scheduler::DaemonJob;
                        sched.submit(DaemonJob::MemoryCondensation).await;
                    }
                }
            }
        });
    } else {
        tracing::debug!("Skipping condensation notify listener: no user entity configured");
    }

    // ---- File watcher for config hot-reload ----
    let config_path = config.path().to_path_buf();
    if let Some(parent) = config_path.parent().map(|p| p.to_path_buf()) {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let config_clone = Arc::clone(&config);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let config_filename = config_path.file_name().map(|n| n.to_os_string());
        let meta_path = config_path.clone();

        tokio::task::spawn_blocking(move || {
            let (debounce_tx, debounce_rx) = std::sync::mpsc::channel();
            let mut debouncer = notify_debouncer_full::new_debouncer(
                std::time::Duration::from_secs(1),
                None,
                debounce_tx,
            )
            .expect("debouncer creation");
            if let Err(e) = debouncer.watch(&parent, notify::RecursiveMode::NonRecursive) {
                tracing::warn!("Failed to watch config directory: {}", e);
                return;
            }
            // Signature (mtime, size) of the last file content we asked the
            // async task to reload. Reading the config file generates
            // `Access`/close events; without dedupe these feed a self-reload
            // loop (~1 event per second) and flood the journal.
            let mut last_sig: Option<(std::time::SystemTime, u64)> = None;
            loop {
                match debounce_rx.recv_timeout(std::time::Duration::from_millis(250)) {
                    Ok(Ok(events)) => {
                        // Only react to real content changes on the config
                        // file; ignore pure `Access` events (open/read/close),
                        // which the OS emits when *we* read the file to reload.
                        let relevant = events.iter().any(|e| {
                            if e.event.kind.is_access() {
                                return false;
                            }
                            e.event.paths.iter().any(|p| {
                                config_filename
                                    .as_ref()
                                    .map(|cf| {
                                        p.file_name().map(|n| n == cf.as_os_str()).unwrap_or(false)
                                    })
                                    .unwrap_or(false)
                            })
                        });
                        if !relevant {
                            continue;
                        }
                        // Dedupe by metadata signature so identical content
                        // (repeated byte-identical saves, or repeated
                        // "sensitive field" rejections) is acted on at most once.
                        let Some(meta) = std::fs::metadata(&meta_path).ok() else {
                            continue;
                        };
                        let sig = (
                            meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                            meta.len(),
                        );
                        if last_sig == Some(sig) {
                            continue;
                        }
                        last_sig = Some(sig);
                        if let Err(e) = tx.try_send(()) {
                            tracing::warn!("Config reload event dropped: {}", e);
                        }
                    }
                    Ok(Err(errors)) => {
                        for error in errors {
                            tracing::warn!("File watcher error: {:?}", error);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            drop(debouncer);
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        let mut shutdown_rx_clone = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx_clone.changed() => {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    msg = rx.recv() => {
                        if msg.is_none() { break; }
                        match config_clone.reload().await {
                            Ok(()) => tracing::info!("Config reloaded from file watcher"),
                            Err(e) => tracing::warn!("Config reload failed: {}", e),
                        }
                    }
                }
            }
        });
    }

    // ---- SIGHUP handler for config hot-reload ----
    #[cfg(unix)]
    spawn_sighup_reload_handler(Arc::clone(&config), state.shutdown_tx.clone());

    let app = build_app(Arc::clone(&state));

    info!("Mimir daemon listening on {}", listener.local_addr()?);

    // Serve until a shutdown trigger fires (Ctrl-C, SIGTERM, or `/stop`),
    // then drain in-flight connections for at most `GRACEFUL_DRAIN_TIMEOUT`.
    // Only the drain phase is bounded — the serving lifetime is unbounded.
    let server_result = serve_with_bounded_drain(
        listener,
        app,
        state.shutdown_tx.clone(),
        GRACEFUL_DRAIN_TIMEOUT,
    )
    .await;

    // Broadcast the shutdown watch so every background task spawned from
    // `start_server_with_llm_and_listener` (file watcher, SIGHUP handler,
    // condensation listener) tears down before the runtime drops. Without an
    // explicit broadcast the SIGTERM/Ctrl-C path relied on `AppState` being
    // dropped during runtime teardown to resolve the file-watcher's
    // `shutdown_rx.changed()` (via sender-drop). That is a race: if tokio's
    // `BlockingPool::shutdown` runs before the watcher task is polled, the
    // `spawn_blocking` thread never exits and the process hangs until systemd
    // aborts it with SIGABRT after `TimeoutStopSec`. Sending here, while the
    // runtime is still fully alive, makes teardown deterministic.
    let _ = state.shutdown_tx.send(true);

    state.shutdown().await;
    server_result?;
    Ok(())
}

/// Spawn the SIGHUP config hot-reload handler.
///
/// The handler is registered synchronously, before the task is spawned:
/// `tokio::signal::unix::signal()` installs the libc handler in its
/// constructor, so a SIGHUP arriving before the spawned task is first polled
/// (e.g. during startup, once the listener is already bound) is caught here
/// instead of hitting the default disposition and killing the daemon (issue
/// #369). Mirrors `spawn_os_signal_shutdown` in `crate::shutdown` (issue
/// #329).
#[cfg(unix)]
fn spawn_sighup_reload_handler(
    config: Arc<ReloadableConfig>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
) {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP handler");
    // Subscribe before the task is spawned so a shutdown broadcast that
    // lands before the task's first poll is still observed: a receiver that
    // subscribes after the broadcast treats the current value as already
    // seen, and `changed()` would wait for a newer notification that never
    // arrives (the task retains `shutdown_tx`), leaving the task alive after
    // server shutdown.
    let mut shutdown_rx = shutdown_tx.subscribe();
    tokio::spawn(async move {
        // The sender is moved into the task so the watch channel stays open
        // for the task's lifetime: `changed()` returns immediately on a
        // closed channel, which would exit the loop before the SIGHUP
        // listener is ever polled.
        let _shutdown_tx = shutdown_tx;
        if *shutdown_rx.borrow_and_update() {
            // Shutdown was already broadcast before the task first polled
            // (e.g. the server is draining): exit immediately instead of
            // waiting for a newer notification.
            return;
        }
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                _ = sighup.recv() => {
                    match config.reload().await {
                        Ok(()) => tracing::info!("Config reloaded from SIGHUP"),
                        Err(e) => tracing::warn!("Config reload failed: {}", e),
                    }
                }
            }
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Regression (issue #369): the SIGHUP handler must be registered
    /// synchronously by `spawn_sighup_reload_handler`, before the spawned
    /// task is first polled. Previously the handler was installed inside the
    /// task, so a SIGHUP arriving in the window between the spawn and the
    /// task's first poll hit the default disposition and killed the daemon
    /// instead of triggering a config reload.
    ///
    /// The real SIGHUP is sent to an isolated child process (this test
    /// re-executed with a marker env var) rather than to this process:
    /// tokio's OS-signal listeners are process-global — every listener
    /// registered for a signal kind receives the notification — so a SIGHUP
    /// delivered here would also fire the SIGHUP listener that other tests
    /// running concurrently in this binary install via
    /// `start_server_with_llm_and_listener`, reloading their config
    /// mid-test. In the child there are no other listeners, and if the
    /// regression returns the child dies from the default disposition
    /// (signal 1) exactly as the original bug did.
    #[test]
    fn test_sighup_registered_before_spawn_reloads_config() {
        const CHILD_ENV: &str = "MIMIR_SIGHUP_REGRESSION_CHILD";
        const CHILD_OK: &str = "mimir-sighup-regression-child-ok";
        const TEST_NAME: &str = "test_sighup_registered_before_spawn_reloads_config";

        if std::env::var_os(CHILD_ENV).is_none() {
            // Parent: run the assertion in a fresh child process and verify
            // it passed (see `crate::test_utils::run_child_regression_test`).
            crate::test_utils::run_child_regression_test(
                module_path!(),
                TEST_NAME,
                CHILD_ENV,
                CHILD_OK,
            );
            return;
        }

        // Child: the actual regression assertion, in a process with no other
        // signal listeners.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build child test runtime");
        runtime.block_on(async {
            use std::time::Duration;

            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("config.toml");
            // Only `llm.temperature` is set; every other field falls back to
            // its serde default, which matches the in-memory
            // `Config::default()`, so the reload below is not rejected by the
            // sensitive-field gate. The file content (0.9) differs from the
            // in-memory default (0.2), so the assertion observes the change
            // actually applied by the SIGHUP-triggered reload.
            std::fs::write(&path, "[llm]\ntemperature = 0.9\n").expect("write config");
            let config = Arc::new(ReloadableConfig::new(
                mimir_core::config::Config::default(),
                path,
            ));

            let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
            spawn_sighup_reload_handler(Arc::clone(&config), shutdown_tx);

            // No await between the call and the signal: if the handler were
            // only installed when the spawned task is polled (the bug), this
            // SIGHUP kills the child via the default disposition (signal 1)
            // before the assertion below runs.
            nix::sys::signal::kill(nix::unistd::getpid(), nix::sys::signal::Signal::SIGHUP)
                .expect("kill(SIGHUP) failed");

            // The handler must catch the signal and reload the config from
            // disk. Poll the snapshot so the assertion observes the reload
            // even though the handler runs in a separate task.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if config.snapshot().await.llm.temperature == 0.9 {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "SIGHUP sent immediately after spawn_sighup_reload_handler did not trigger a config reload"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            println!("{CHILD_OK}");
        });
    }

    /// Regression (PR #421 CodeRabbit finding): the shutdown receiver must be
    /// subscribed before the handler task is spawned, and the task must check
    /// the current shutdown value before entering its loop. A shutdown
    /// broadcast that lands between the spawn and the task's first poll is
    /// otherwise missed: the task retains `shutdown_tx`, so `changed()` waits
    /// for a newer notification that never arrives and the handler stays
    /// alive after server shutdown.
    ///
    /// The broadcast is sent without yielding to the runtime, so the spawned
    /// task has definitely not been polled yet (current-thread runtime). The
    /// test observes the broadcast through its own receiver, then expects the
    /// channel to close once the task runs and exits, dropping the retained
    /// sender.
    #[tokio::test]
    async fn test_sighup_handler_exits_when_shutdown_before_first_poll() {
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm]\ntemperature = 0.9\n").expect("write config");
        let config = Arc::new(ReloadableConfig::new(
            mimir_core::config::Config::default(),
            path,
        ));

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        spawn_sighup_reload_handler(config, shutdown_tx.clone());

        // Broadcast before yielding, so the handler task has not been polled
        // yet.
        shutdown_tx.send(true).expect("send shutdown");
        drop(shutdown_tx);

        // The test's own receiver was subscribed before the broadcast, so it
        // observes the change...
        shutdown_rx
            .changed()
            .await
            .expect("observe shutdown broadcast");
        // ...and when the handler task runs it must see the current value,
        // exit, and drop its retained sender, closing the channel. A handler
        // that subscribed only after the broadcast would wait forever for a
        // newer notification and time out here.
        tokio::time::timeout(Duration::from_secs(5), shutdown_rx.changed())
            .await
            .expect("handler task must exit after shutdown")
            .expect_err("channel must close when the handler task exits");
    }
}
