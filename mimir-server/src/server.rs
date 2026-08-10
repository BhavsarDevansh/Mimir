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
    let llm_client: Arc<dyn LlmBackend> =
        Arc::new(LlmClient::new(config.snapshot().await.llm.clone()).await?);
    start_server_with_llm(config, llm_client).await
}

/// Start the Mimir HTTP server with an injected LLM backend.
///
/// This is the same as [`start_server`], but allows tests (and future
/// embedders) to supply a custom [`LlmBackend`] implementation without
/// relying on sentinel strings or config hacks.
pub async fn start_server_with_llm(
    config: Arc<ReloadableConfig>,
    llm_client: Arc<dyn LlmBackend>,
) -> anyhow::Result<()> {
    let bind_addr = config.snapshot().await.server.bind_addr.clone();
    let addr: SocketAddr = bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    start_server_with_llm_and_listener(config, llm_client, listener).await
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
) -> anyhow::Result<()> {
    let (app_state, scheduler_shutdown_rx) =
        AppState::from_config_with_llm(Arc::clone(&config), llm_client).await?;
    let state = Arc::new(app_state);

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
    {
        let config_clone = Arc::clone(&config);
        let mut shutdown_rx_clone = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP handler");
            loop {
                tokio::select! {
                    _ = shutdown_rx_clone.changed() => break,
                    _ = sighup.recv() => {
                        match config_clone.reload().await {
                            Ok(()) => tracing::info!("Config reloaded from SIGHUP"),
                            Err(e) => tracing::warn!("Config reload failed: {}", e),
                        }
                    }
                }
            }
        });
    }

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
