//! Daemon lifecycle: state initialisation, background tasks, and server startup.
#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use mimir_core::config::ReloadableConfig;
use mimir_core::llm::{LlmBackend, LlmClient};
use tracing::info;

use crate::LocalPeer;
use crate::app::build_app;
use crate::shutdown::{GRACEFUL_DRAIN_TIMEOUT, serve_with_bounded_drain};
use crate::state::AppState;

///
/// Loads shared state from `config`, binds to `config.server.bind_addr`,
/// and runs until the process is terminated or a graceful shutdown is
/// triggered via the `/stop` endpoint, Ctrl-C, or SIGTERM. On Unix the same
/// router is also served on a Unix domain socket (issue #25) alongside the
/// TCP listener, so the local CLI can prefer the socket.
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
    let (app_state, scheduler_shutdown_rx, hook_shutdown_rx) =
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

    // ---- Start hooks engine dispatch loop ----
    let hook_engine = Arc::clone(&state.hook_engine);
    tokio::spawn(async move {
        hook_engine.start(hook_shutdown_rx).await;
    });

    // ---- Listen for KG dirty signal and trigger condensation hook ----
    if state.user_entity_id.is_some() {
        let notify = state.knowledge_graph.condensation_notify();
        let hook_engine = Arc::clone(&state.hook_engine);
        let mut shutdown_rx = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = notify.notified() => {
                        use mimir_core::hooks::Trigger;
                        hook_engine.trigger(Trigger::FactInserted).await;
                    }
                }
            }
        });
    } else {
        tracing::debug!("Skipping condensation notify listener: no user entity configured");
    }

    // ---- File watcher for config hot-reload ----
    spawn_config_watcher(Arc::clone(&config), state.shutdown_tx.clone());

    // ---- SIGHUP handler for config hot-reload ----
    #[cfg(unix)]
    spawn_sighup_reload_handler(Arc::clone(&config), state.shutdown_tx.clone());

    let app = build_app(Arc::clone(&state));

    info!("Mimir daemon listening on {}", listener.local_addr()?);

    // ---- Unix domain socket transport (issue #25) ----
    // Bind the local CLI socket (configured path, or the platform default
    // `<data_dir>/mimir.sock` on Unix) and serve the same router on it. The
    // CLI prefers the socket for local commands; TCP remains for remote or
    // non-Unix clients.
    let unix_task = bind_and_serve_unix_socket(&config, app.clone(), &state).await?;

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

    // Join the Unix-socket server. It observes the same shutdown trigger, so
    // it stops accepting at the same time as the TCP listener; bound its
    // drain so a wedged local stream cannot extend shutdown past the TCP
    // bound. On a fatal TCP serve error no trigger ever fires, so abort the
    // task rather than waiting for the timeout.
    if let Some((socket_path, task)) = unix_task {
        if server_result.is_err() {
            task.abort();
        }
        match tokio::time::timeout(GRACEFUL_DRAIN_TIMEOUT, task).await {
            // The Unix server joined cleanly alongside the TCP server.
            Ok(Ok(Ok(()))) => {}
            // The Unix listener failed on its own while TCP is healthy;
            // report the socket loss instead of dropping it silently.
            Ok(Ok(Err(error))) => {
                tracing::warn!("Unix socket server exited with an error: {error:#}");
            }
            // Aborted because the TCP server failed; `serve_with_bounded_drain`
            // already reported that failure.
            Ok(Err(error)) if error.is_cancelled() => {}
            // Panic inside the Unix server task.
            Ok(Err(error)) => {
                tracing::warn!("Unix socket server task failed: {error}");
            }
            Err(_) => {
                tracing::warn!(
                    "Unix socket drain timed out after {}s; forcing exit.",
                    GRACEFUL_DRAIN_TIMEOUT.as_secs()
                );
            }
        }
        // The Unix task removes the socket file itself while it still owns the
        // listener (graceful path). After an abort the file may remain, but a
        // replacement daemon may also have bound the pathname in the meantime;
        // only remove it when no live listener holds it (PR #503 review).
        #[cfg(unix)]
        if !mimir_core::config::socket_is_live(&socket_path).await {
            let _ = tokio::fs::remove_file(&socket_path).await;
        }
        #[cfg(not(unix))]
        let _ = socket_path;
    }

    // Broadcast the shutdown watch so every background task spawned from
    // `start_server_with_llm_and_listener` (file watcher, SIGHUP handler,
    // condensation listener) tears down before the runtime drops. Without an
    // explicit broadcast the SIGTERM/Ctrl-C path relied on `AppState` being
    // dropped during runtime teardown to resolve the file-watcher's
    // `shutdown_rx.changed()` (via sender-drop). That is a race: if tokio's
    // `BlockingPool::shutdown` runs before the watcher task is polled, the
    // `spawn_blocking` thread never exits and the process hangs until systemd
    // aborts it with SIGABRT after `TimeoutStopSec`. Sending here, while the
    // runtime is still fully alive, makes teardown deterministic. Error paths
    // that never reach this broadcast (panic, early return) are covered by
    // the watcher's own lifetime channel: dropping the async task disconnects
    // the blocking loop, so runtime teardown no longer hangs (issue #415).
    let _ = state.shutdown_tx.send(true);

    state.shutdown().await;
    server_result?;
    Ok(())
}

/// Bind the Unix domain socket listener, after probing that any existing
/// socket file is stale (a live daemon holding the path aborts startup with
/// an "already in use" error), and spawn the task that serves the shared
/// router on it.
///
/// Returns `Ok(None)` when no Unix socket applies (non-Unix platforms);
/// otherwise the socket path plus the spawned task so the caller can join it
/// and remove the socket file on shutdown.
#[cfg(unix)]
async fn bind_and_serve_unix_socket(
    config: &ReloadableConfig,
    app: axum::Router,
    state: &Arc<AppState>,
) -> anyhow::Result<Option<(PathBuf, tokio::task::JoinHandle<anyhow::Result<()>>)>> {
    let configured = config.snapshot().await.server.socket_path.clone();
    let Some(socket_path) = mimir_core::config::effective_socket_path(configured.as_deref()) else {
        return Ok(None);
    };
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "creating parent directory for Unix socket {}",
                socket_path.display()
            )
        })?;
    }
    if socket_path.exists() {
        // A pathname socket may belong to a *live* daemon rather than a
        // crashed one. Unlinking it would steal the local transport from the
        // running process, so prove staleness with a bounded connect attempt
        // before removing anything (PR #503 review).
        if mimir_core::config::socket_is_live(&socket_path).await {
            anyhow::bail!(
                "Unix socket {} is already in use by a running daemon",
                socket_path.display()
            );
        }
        tracing::warn!(
            "removing stale Unix socket {} before binding",
            socket_path.display()
        );
        tokio::fs::remove_file(&socket_path)
            .await
            .with_context(|| format!("removing stale Unix socket {}", socket_path.display()))?;
    }
    let listener = tokio::net::UnixListener::bind(&socket_path)
        .with_context(|| format!("binding Unix socket {}", socket_path.display()))?;
    restrict_socket_permissions(&socket_path).with_context(|| {
        format!(
            "setting permissions on Unix socket {}",
            socket_path.display()
        )
    })?;

    let shutdown_rx = state.shutdown_tx.subscribe();
    let cleanup_path = socket_path.clone();
    let task = tokio::spawn(async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<LocalPeer>(),
        )
        .with_graceful_shutdown(crate::shutdown::watch_shutdown(shutdown_rx))
        .await
        .map_err(anyhow::Error::from);
        // Remove the socket file while this task still owns the listener, so
        // shutdown cleanup can never unlink a replacement daemon's socket
        // (PR #503 review).
        let _ = tokio::fs::remove_file(&cleanup_path).await;
        result
    });
    info!(
        "Mimir daemon listening on Unix socket {}",
        socket_path.display()
    );
    Ok(Some((socket_path, task)))
}

/// Restrict the socket file to the owning user (mode 0600) so filesystem
/// permissions, not just the API token, gate local access.
#[cfg(unix)]
fn restrict_socket_permissions(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// No Unix socket on non-Unix platforms (Windows falls back to TCP).
#[cfg(not(unix))]
async fn bind_and_serve_unix_socket(
    _config: &ReloadableConfig,
    _app: axum::Router,
    _state: &Arc<AppState>,
) -> anyhow::Result<Option<(PathBuf, tokio::task::JoinHandle<anyhow::Result<()>>)>> {
    Ok(None)
}

/// Spawn the config hot-reload file watcher.
///
/// A blocking thread (`notify` + `notify_debouncer_full`) watches the config
/// directory for content changes and forwards reload events to an async task
/// that performs the reload. The blocking thread's lifetime is tied to the
/// async task through a `std::sync::mpsc` lifetime channel: the sender is
/// owned by the async task, so the blocking loop observes `Disconnected` and
/// exits whenever the task exits — including when the runtime is dropped on
/// an error path where the shutdown watch never fires. Without this, the
/// `spawn_blocking` thread would loop on `recv_timeout` forever and tokio's
/// runtime drop would hang joining the blocking pool (issue #415). Returns
/// `None` when the config path has no parent directory to watch.
fn spawn_config_watcher(
    config: Arc<ReloadableConfig>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    spawn_config_watcher_inner(config, shutdown_tx, None)
}

/// Test-only variant of [`spawn_config_watcher`] that signals successful
/// watcher registration on `ready_tx`. Tokio does not guarantee that a
/// `spawn_blocking` closure has started by the time the spawning call (or
/// `Runtime::block_on`) returns, so tests that drop the runtime or rewrite
/// the watched file immediately after spawning could run before
/// `debouncer.watch` registers the directory and pass without exercising the
/// path under test. Tests wait on the signal before proceeding (PR #437
/// review).
#[cfg(test)]
fn spawn_config_watcher_with_readiness(
    config: Arc<ReloadableConfig>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    ready_tx: std::sync::mpsc::Sender<()>,
) -> Option<tokio::task::JoinHandle<()>> {
    spawn_config_watcher_inner(config, shutdown_tx, Some(ready_tx))
}

/// Shared implementation of [`spawn_config_watcher`]. When `ready_tx` is
/// present (tests only), the blocking thread signals it once `debouncer.watch`
/// has registered the directory, giving tests a deterministic point after
/// which the watch is active.
fn spawn_config_watcher_inner(
    config: Arc<ReloadableConfig>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    ready_tx: Option<std::sync::mpsc::Sender<()>>,
) -> Option<tokio::task::JoinHandle<()>> {
    let config_path = config.path().to_path_buf();
    let parent = config_path.parent()?.to_path_buf();

    // Reload-event channel: produced by the blocking thread, consumed by the
    // async task. The sender lives inside the blocking closure, so this
    // receiver only disconnects once the blocking thread has exited.
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    // Lifetime channel: the sender is owned by the async task below, so the
    // blocking loop observes `Disconnected` whenever the task exits — on
    // every exit branch, and when runtime teardown drops the task without
    // the shutdown watch firing (issue #415).
    let (lifetime_tx, lifetime_rx) = std::sync::mpsc::channel::<()>();
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
        if let Some(ready_tx) = ready_tx {
            // Tests wait on this before dropping the runtime or rewriting the
            // watched file (PR #437 review).
            let _ = ready_tx.send(());
        }
        // Signature (mtime, size) of the last file content we asked the
        // async task to reload. Reading the config file generates
        // `Access`/close events; without dedupe these feed a self-reload
        // loop (~1 event per second) and flood the journal.
        let mut last_sig: Option<(std::time::SystemTime, u64)> = None;
        loop {
            // Exit when the async task is gone: its lifetime sender is
            // dropped on every exit path, including a runtime teardown that
            // drops the task without firing the shutdown watch (issue #415).
            // Polled on every iteration (not only on timeout) so a burst of
            // watcher events cannot delay shutdown.
            if matches!(
                lifetime_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ) {
                break;
            }
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
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    // Subscribe before the task is spawned and check the current value
    // before entering the loop, so a shutdown broadcast that lands before
    // the task's first poll is still observed (same pattern as
    // `spawn_sighup_reload_handler`, issue #421).
    let mut shutdown_rx = shutdown_tx.subscribe();
    Some(tokio::spawn(async move {
        // Held for the task's lifetime: dropping it on any exit path (break,
        // early return, or runtime teardown) disconnects the blocking loop's
        // receiver so its thread exits too (issue #415).
        let _lifetime_tx = lifetime_tx;
        // Keep the watch channel open for the task's lifetime: `changed()`
        // returns immediately on a closed channel, which would exit the loop
        // before the watcher is ever polled (same pattern as
        // `spawn_sighup_reload_handler`).
        let _shutdown_tx = shutdown_tx;
        if *shutdown_rx.borrow_and_update() {
            // Shutdown was already broadcast: exit immediately. Dropping
            // `_lifetime_tx` lets the blocking thread exit too.
            return;
        }
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                msg = rx.recv() => {
                    if msg.is_none() { break; }
                    match config.reload().await {
                        Ok(()) => tracing::info!("Config reloaded from file watcher"),
                        Err(e) => tracing::warn!("Config reload failed: {}", e),
                    }
                }
            }
        }
    }))
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

/// Regression (issue #415): the config watcher's `spawn_blocking` thread
/// must exit when the async watcher task is dropped, even when the
/// shutdown watch never fires (e.g. a panic or early-return error path
/// drops the runtime before the shutdown broadcast). Previously the
/// blocking loop only exited via the `stop` flag set by the async task's
/// shutdown branch, so dropping the runtime without a shutdown broadcast
/// leaked the thread and tokio's runtime drop hung indefinitely joining
/// the blocking pool.
///
/// The watcher itself is cross-platform (`notify`), so these tests live in a
/// non-Unix-gated module and run on every platform.
#[cfg(test)]
mod watcher_tests {
    use super::*;

    #[test]
    fn test_config_watcher_thread_exits_when_runtime_dropped_without_shutdown() {
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm]\ntemperature = 0.9\n").expect("write config");
        let config = Arc::new(ReloadableConfig::new(
            mimir_core::config::Config::default(),
            path,
        ));

        // The runtime is built and dropped on a helper thread: with the
        // regression the hang happens inside `Runtime::drop` (blocking-pool
        // join), so the drop must not run on this test thread or a failure
        // would hang the whole test binary. The helper signals completion;
        // if no signal arrives within the timeout the watcher thread leaked.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime");
            runtime.block_on(async {
                let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
                let _watcher = spawn_config_watcher_with_readiness(config, shutdown_tx, ready_tx);
            });
            // Wait for the blocking thread to register the directory watch
            // before dropping the runtime: tokio does not guarantee that a
            // `spawn_blocking` closure has started by the time `block_on`
            // returns, so an immediate drop could leave the closure never
            // started and the test passing without exercising the
            // leaked-thread path (PR #437 review).
            ready_rx.recv_timeout(Duration::from_secs(10)).expect(
                "watcher thread must register the directory watch before the runtime is dropped",
            );
            // Dropping the runtime must not hang; with the regression it
            // joins the leaked blocking thread forever.
            drop(runtime);
            let _ = done_tx.send(());
        });

        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("runtime drop must complete: config watcher thread leaked (issue #415)");
    }

    /// Happy path for the watcher loop refactored in issue #415: a content
    /// change on the config file is forwarded by the blocking thread through
    /// the debouncer and reloaded by the async task.
    #[tokio::test]
    async fn test_config_watcher_reloads_on_file_change() {
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm]\ntemperature = 0.9\n").expect("write config");
        let config = Arc::new(ReloadableConfig::new(
            mimir_core::config::Config::default(),
            path.clone(),
        ));

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let _watcher =
            spawn_config_watcher_with_readiness(Arc::clone(&config), shutdown_tx, ready_tx);

        // Wait for the blocking thread to register the file watch before
        // writing different content: the old fixed delay did not guarantee
        // registration, so the write could land before the watch and the
        // reload assertion could pass vacuously (PR #437 review). The length
        // differs from the first write so the metadata-signature dedupe
        // cannot mistake it for a duplicate.
        ready_rx.recv_timeout(Duration::from_secs(10)).expect(
            "watcher thread must register the directory watch before the config is rewritten",
        );
        std::fs::write(&path, "[llm]\ntemperature = 1.25\n").expect("write config");

        // The debouncer waits 1 s before forwarding, so poll the snapshot
        // until the reload lands.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if config.snapshot().await.llm.temperature == 1.25 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "config change was not reloaded by the file watcher"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
