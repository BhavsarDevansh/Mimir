//! Shutdown coordination: trigger sources, OS signals, and bounded graceful drain.
#![deny(unsafe_code)]

use std::time::Duration;

use axum::Router;
use tracing::{info, warn};

use crate::LocalPeer;

/// The origin of a daemon shutdown request.
///
/// Every code path that fires the shared `shutdown_tx` watch trigger
/// constructs a `ShutdownSource` and logs [`ShutdownSource::attribution`]
/// *before* sending, so the journal records *what* stopped the daemon — not
/// merely that it stopped. Previously all paths emitted the identical line
/// "Server shut down gracefully.", making the cause of unexpected exits
/// (e.g. 2026-06-30) impossible to determine from logs.
#[derive(Debug)]
pub enum ShutdownSource {
    /// The `/stop` HTTP endpoint, invoked by a loopback peer (e.g. `mimir stop`).
    StopEndpoint(LocalPeer),
    /// A `SIGTERM` delivered by the OS (e.g. `systemctl stop`, `kill`).
    Terminate,
    /// An interrupt signal (`Ctrl-C` / `SIGINT`).
    Interrupt,
}

impl ShutdownSource {
    /// Human-readable attribution line written to the log immediately before
    /// the shutdown trigger fires.
    pub fn attribution(&self) -> String {
        match self {
            ShutdownSource::StopEndpoint(peer) => {
                format!("Shutdown requested via /stop endpoint from {peer}.")
            }
            ShutdownSource::Terminate => "Shutdown triggered by SIGTERM (signal).".to_string(),
            ShutdownSource::Interrupt => "Shutdown triggered by interrupt (Ctrl-C).".to_string(),
        }
    }
}

/// Exit log line for [`serve_with_bounded_drain`], classifying whether the
/// server stopped because a shutdown trigger fired (graceful) or because the
/// server future resolved on its own without any trigger (unexpected).
///
/// Extracted as a pure function so the "do not mislabel an untriggered exit as
/// graceful" invariant is unit-testable without capturing log output.
pub fn server_exit_message(triggered: bool) -> &'static str {
    if triggered {
        "Server shut down gracefully."
    } else {
        "Server future resolved without a shutdown trigger; exiting."
    }
}

/// Capture Ctrl-C / SIGTERM **once** and fan the notification into the shared
/// `shutdown_tx` watch trigger.
///
/// Spawning a dedicated task (rather than building a fresh `ctrl_c()`/SIGTERM
/// future in every consumer) guarantees there is a single OS-signal listener
/// for the whole process. Both axum's graceful-shutdown future and the
/// phase-1 waiter observe the *same* `shutdown_tx` watch channel via
/// [`watch_shutdown`], so neither can observe a signal before the other has
/// registered interest — the original race that could leave axum accepting
/// connections until the drain timeout kicked in.
///
/// The signal handlers are registered synchronously, before the task is
/// spawned: `tokio::signal::unix::signal()` installs the libc handler in its
/// constructor, so a SIGTERM/SIGINT arriving before the spawned task is first
/// polled (e.g. during startup, once the health listener is already
/// accepting) is caught here instead of hitting the default disposition and
/// killing the process (issue #329).
fn spawn_os_signal_shutdown(shutdown_tx: tokio::sync::watch::Sender<bool>) {
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    #[cfg(unix)]
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("SIGINT handler");
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            // Distinguish which signal fired so the journal attributes the
            // shutdown to its real cause rather than a generic "graceful" line.
            tokio::select! {
                _ = sigint.recv() => {
                    info!("{}", ShutdownSource::Interrupt.attribution());
                }
                _ = sigterm.recv() => {
                    info!("{}", ShutdownSource::Terminate.attribution());
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            info!("{}", ShutdownSource::Interrupt.attribution());
        }
        let _ = shutdown_tx.send(true);
    });
}

/// Complete once the shared shutdown watch trigger fires.
///
/// Used by both axum's `with_graceful_shutdown` and the phase-1 serving loop
/// so they react to the same notification — whether it originated from
/// `/stop` writing to the watch channel or from `spawn_os_signal_shutdown`.
pub async fn watch_shutdown(mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
    // A freshly `subscribe()`d receiver's `changed()` only wakes on *future*
    // updates — a trigger fired before subscription (e.g. SIGTERM arriving in
    // the gap between `spawn_os_signal_shutdown` and `shutdown_tx.subscribe()`)
    // would otherwise be missed and leave the server waiting indefinitely.
    // Check the current value first so an already-fired trigger returns
    // immediately, then await further changes for triggers that fire later.
    if *shutdown_rx.borrow_and_update() {
        return;
    }
    let _ = shutdown_rx.changed().await;
}

/// Maximum time to wait for in-flight connections to finish after a shutdown
/// is requested. Bounds **only** the post-signal drain phase; the server runs
/// indefinitely while no shutdown is requested.
pub(crate) const GRACEFUL_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `app` on `listener` until a shutdown trigger fires, then bound the
/// graceful drain of in-flight connections to `drain_timeout`.
///
/// Extracted from `start_server_with_llm_and_listener` so the drain bound can
/// be unit-tested with a short timeout (see `test_serve_outlives_drain_timeout`).
pub async fn serve_with_bounded_drain(
    listener: tokio::net::TcpListener,
    app: Router,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    drain_timeout: Duration,
) -> anyhow::Result<()> {
    // Capture OS signals (Ctrl-C / SIGTERM) exactly once and fan them into the
    // shared `shutdown_tx` watch trigger. The `/stop` endpoint also writes to
    // this channel, so a single watch observation covers every trigger source
    // for both phases below — no duplicate `ctrl_c()`/SIGTERM listeners.
    spawn_os_signal_shutdown(shutdown_tx.clone());

    // One receiver drives axum's graceful-shutdown (stop accepting); the other
    // lets this function detect the trigger independently so it can bound only
    // the *drain* phase rather than the whole serving lifetime. Both observe
    // the same watch trigger, so they fire in lockstep.
    let graceful_rx = shutdown_tx.subscribe();
    let trigger_rx = shutdown_tx.subscribe();

    // Wrap axum's `IntoFuture` in an `async` block to obtain a concrete
    // `Future` that can be pinned and polled across two phases below.
    // (axum 0.8's `WithGracefulShutdown` implements `IntoFuture` but not
    // `Future`, so it cannot be `tokio::pin!`-ed or `&mut`-polled directly.)
    let server_fut = async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<LocalPeer>(),
        )
        .with_graceful_shutdown(watch_shutdown(graceful_rx))
        .await
    };

    // Pin the server future so it can be polled across two phases: first
    // serving (unbounded), then draining (bounded by `drain_timeout`).
    tokio::pin!(server_fut);

    // Phase 1 — serve until the shared shutdown trigger fires (Ctrl-C,
    // SIGTERM, or `/stop`). The server keeps accepting and handling
    // connections throughout; this wait is intentionally unbounded. If the
    // server future resolves first (e.g. a fatal listener error), propagate it.
    tokio::select! {
        biased;
        _ = watch_shutdown(trigger_rx) => {},
        result = &mut server_fut => {
            // The server future resolved without a shutdown trigger firing
            // first (e.g. a fatal listener error). Do NOT label this
            // "gracefully" — that masked the real cause of unexpected exits.
            warn!("{}", server_exit_message(false));
            return Ok(result?);
        }
    }

    // Phase 2 — trigger fired. axum's own graceful-shutdown signal (wired to
    // the same triggers via `graceful_rx`) has fired too, so it has stopped
    // accepting and is now draining in-flight connections. Bound that drain
    // so a wedged SSE stream cannot keep the process alive past systemd's
    // `TimeoutStopSec`. On timeout, dropping `server_fut` cuts the connections.
    let server_result = match tokio::time::timeout(drain_timeout, &mut server_fut).await {
        Ok(result) => {
            info!("{}", server_exit_message(true));
            result
        }
        Err(_) => {
            warn!(
                "Graceful drain timed out after {}s; forcing exit.",
                drain_timeout.as_secs()
            );
            Ok(())
        }
    };
    Ok(server_result?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use mimir_core::config::Config;

    /// Owns the spawned server task and aborts it on drop, so a panicking
    /// assertion cannot leave a live server holding the temp DBs and the
    /// reserved port for the rest of the suite (issue #384).
    struct ServerGuard(Option<tokio::task::JoinHandle<anyhow::Result<()>>>);

    impl ServerGuard {
        fn new(handle: tokio::task::JoinHandle<anyhow::Result<()>>) -> Self {
            Self(Some(handle))
        }

        fn is_finished(&self) -> bool {
            self.0.as_ref().is_some_and(|h| h.is_finished())
        }

        fn into_inner(mut self) -> tokio::task::JoinHandle<anyhow::Result<()>> {
            self.0.take().expect("server handle already taken")
        }
    }

    impl Drop for ServerGuard {
        fn drop(&mut self) {
            if let Some(handle) = self.0.take() {
                handle.abort();
            }
        }
    }

    /// Attribution strings are the whole point of this fix: the journal must
    /// record *what* stopped the daemon. Lock the exact wording so a future
    /// refactor cannot silently drop attribution.
    #[test]
    fn test_shutdown_source_attribution_messages() {
        let peer = crate::LocalPeer::Tcp(std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            8080,
        ));
        let stop = ShutdownSource::StopEndpoint(peer).attribution();
        assert!(stop.contains("/stop endpoint"), "got: {stop}");
        assert!(stop.contains("127.0.0.1:8080"), "got: {stop}");

        let term = ShutdownSource::Terminate.attribution();
        assert!(term.contains("SIGTERM"), "got: {term}");

        let interrupt = ShutdownSource::Interrupt.attribution();
        assert!(interrupt.contains("Ctrl-C"), "got: {interrupt}");
    }
    /// Regression: an exit where the server future resolved *without* a
    /// shutdown trigger must not be mislabeled "gracefully" (the original
    /// bug masked the real cause of unexpected daemon exits).
    #[test]
    fn test_server_exit_message_distinguishes_untriggered_exit() {
        let graceful = server_exit_message(true);
        let untriggered = server_exit_message(false);
        assert_eq!(graceful, "Server shut down gracefully.");
        assert_ne!(
            untriggered, graceful,
            "an exit without a trigger must not be reported as graceful"
        );
        assert!(
            untriggered.contains("without a shutdown trigger"),
            "got: {untriggered}"
        );
    }

    /// Build a server config pointing every database at `temp` (so tests
    /// never touch the real `~/.local/share/mimir`), with an optional Unix
    /// socket path. Returns the config and a free TCP address for the
    /// listener.
    async fn test_config(
        temp: &tempfile::TempDir,
        socket_path: Option<std::path::PathBuf>,
    ) -> (
        Arc<mimir_core::config::ReloadableConfig>,
        std::net::SocketAddr,
    ) {
        let db_path = temp.path().join("context.db");
        let kg_db_path = temp.path().join("knowledge.db");
        let jobs_db_path = temp.path().join("jobs.db");

        // Find an available port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let mut config = Config::default();
        config.llm.endpoint = "http://127.0.0.1:1".to_string();
        config.llm.api_key = "test".to_string();
        config.llm.model = "gpt-4o".to_string();
        config.llm.max_tokens = Some(10);
        config.llm.temperature = 0.0;
        config.server.bind_addr = addr.to_string();
        config.server.socket_path = socket_path.map(|p| p.display().to_string());
        config.memory.char_limit = 10_000;
        config.context.db_path = Some(db_path);
        config.knowledge.db_path = Some(kg_db_path);
        config.scheduler.db_path = Some(jobs_db_path);

        let config = Arc::new(mimir_core::config::ReloadableConfig::new(
            config,
            temp.path().join("config.toml"),
        ));
        (config, addr)
    }

    /// Spawn the daemon with `config`, a known token, and a mock LLM, wrapped
    /// in a [`ServerGuard`] so a panicking assertion aborts the server.
    fn spawn_test_server(
        config: Arc<mimir_core::config::ReloadableConfig>,
        api_token: &str,
    ) -> (ServerGuard, Arc<str>) {
        use mimir_core::llm::{LlmBackend, MockLlmClient};

        let api_token: Arc<str> = Arc::from(api_token);
        let llm: Arc<dyn LlmBackend> = Arc::new(MockLlmClient::builder().build());
        let token_for_server = api_token.clone();
        let handle = ServerGuard::new(tokio::spawn(async move {
            crate::server::start_server_with_llm(config, llm, token_for_server).await
        }));
        (handle, api_token)
    }

    #[tokio::test]
    async fn test_server_exits_after_stop() {
        let temp = tempfile::tempdir().unwrap();
        let (config, addr) = test_config(&temp, Some(temp.path().join("mimir.sock"))).await;

        // Inject a known token and mock LLM so the test never reads or writes
        // the real `~/.local/share/mimir` token file or DBs: the daemon API is
        // bearer-authenticated (issue #281), and `Config::default()` resolves
        // unset knowledge/scheduler db paths to the real data dir, which a
        // leftover test daemon could then lock and hang parallel suites with
        // (issue #384).
        let (handle, api_token) = spawn_test_server(config, "test-api-token");

        // Poll until the server accepts a TCP connection (up to 5 s).
        let poll_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut ready = false;
        while tokio::time::Instant::now() < poll_deadline {
            if handle.is_finished() {
                let result = handle.into_inner().await.unwrap();
                panic!("server exited early: {:?}", result);
            }
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ready, "server did not become reachable within 5 seconds");

        // Send the stop request.
        let client = reqwest::Client::builder().http1_only().build().unwrap();
        let res = client
            .post(format!("http://{}/stop", addr))
            .bearer_auth(api_token.as_ref())
            .send()
            .await;

        match res {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                assert_eq!(
                    status,
                    reqwest::StatusCode::OK,
                    "unexpected status: {} body: {}",
                    status,
                    body
                );
            }
            Err(e) => {
                // Server may shut down before the response is fully read.
                // As long as the server exits below, the stop signal was received.
                eprintln!(
                    "Stop request got connection error (server shutting down): {}",
                    e
                );
            }
        }

        // The server should exit within 5 seconds.
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), handle.into_inner()).await;
        assert!(result.is_ok(), "server did not exit within 5 seconds");
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_server_serves_over_unix_socket_and_removes_socket_on_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("mimir.sock");
        let (config, _addr) = test_config(&temp, Some(socket_path.clone())).await;
        let (handle, api_token) = spawn_test_server(config, "test-api-token");

        let client = reqwest::Client::builder()
            .unix_socket(socket_path.as_path())
            .build()
            .unwrap();

        // Poll until the socket file exists and serves /health (up to 5 s).
        let poll_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut ready = false;
        while tokio::time::Instant::now() < poll_deadline {
            if handle.is_finished() {
                let result = handle.into_inner().await.unwrap();
                panic!("server exited early: {:?}", result);
            }
            if socket_path.exists()
                && client
                    .get("http://localhost/health")
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
            {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            ready,
            "daemon did not serve over the Unix socket within 5 seconds"
        );

        // `/stop` over the socket must be accepted (loopback guard treats
        // Unix peers as local) and trigger a graceful shutdown.
        let res = client
            .post("http://localhost/stop")
            .bearer_auth(api_token.as_ref())
            .send()
            .await;
        match res {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                assert_eq!(
                    status,
                    reqwest::StatusCode::OK,
                    "unexpected status: {} body: {}",
                    status,
                    body
                );
            }
            Err(e) => {
                eprintln!(
                    "Stop request got connection error (server shutting down): {}",
                    e
                );
            }
        }

        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), handle.into_inner()).await;
        assert!(
            result.is_ok(),
            "server did not exit within 5 seconds after /stop over the Unix socket"
        );
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
        assert!(
            !socket_path.exists(),
            "socket file must be removed on graceful shutdown"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_server_removes_stale_socket_before_binding() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("stale.sock");
        // Leave a stale socket file behind, as a crashed daemon would: a bound
        // listener whose owner exited without removing the file.
        let stale = tokio::net::UnixListener::bind(&socket_path).unwrap();
        drop(stale);
        assert!(socket_path.exists(), "stale socket file must exist");

        let (config, _addr) = test_config(&temp, Some(socket_path.clone())).await;
        let (handle, api_token) = spawn_test_server(config, "test-api-token");

        let client = reqwest::Client::builder()
            .unix_socket(socket_path.as_path())
            .build()
            .unwrap();
        let poll_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut ready = false;
        while tokio::time::Instant::now() < poll_deadline {
            if handle.is_finished() {
                let result = handle.into_inner().await.unwrap();
                panic!("server exited early: {:?}", result);
            }
            if client
                .get("http://localhost/health")
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            ready,
            "daemon did not recover the stale socket and serve within 5 seconds"
        );

        // Tear the daemon down so the temp dir can be removed.
        let res = client
            .post("http://localhost/stop")
            .bearer_auth(api_token.as_ref())
            .send()
            .await;
        assert!(
            res.is_ok() || handle.is_finished(),
            "stop over the recovered socket should reach the daemon"
        );
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), handle.into_inner()).await;
        assert!(result.is_ok(), "server did not exit within 5 seconds");
    }

    /// A second daemon must not unlink and steal a live socket from a running
    /// daemon: startup fails with a descriptive "already in use" error and the
    /// original listener keeps the pathname (PR #503 review).
    #[cfg(unix)]
    #[tokio::test]
    async fn test_server_refuses_to_steal_live_socket() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("live.sock");
        let live = tokio::net::UnixListener::bind(&socket_path).unwrap();

        let (config, _addr) = test_config(&temp, Some(socket_path.clone())).await;
        let (handle, _api_token) = spawn_test_server(config, "test-api-token");

        // The daemon must fail startup instead of unlinking the live listener.
        let joined = tokio::time::timeout(std::time::Duration::from_secs(5), handle.into_inner())
            .await
            .expect("server must exit within 5 seconds")
            .expect("server task must not panic");
        let error = joined.expect_err("server must refuse to start over a live socket");
        assert!(
            error.to_string().contains("already in use"),
            "unexpected error: {error}"
        );
        assert!(
            tokio::net::UnixStream::connect(&socket_path).await.is_ok(),
            "live socket must still be connectable after the failed startup"
        );
        drop(live);
    }

    /// Regression: a shutdown trigger fired *before* a receiver subscribes
    /// (e.g. SIGTERM arriving in the gap between `spawn_os_signal_shutdown`
    /// and `shutdown_tx.subscribe()`) must not be missed. `changed()` only
    /// wakes on future updates, so `watch_shutdown` must check the current
    /// watch value first.
    #[tokio::test]
    async fn test_watch_shutdown_handles_already_fired_trigger() {
        use std::time::Duration;

        let (shutdown_tx, _rx) = tokio::sync::watch::channel(false);

        // Fire the trigger BEFORE subscribing — the race window.
        shutdown_tx.send(true).unwrap();

        // Subscribe after the send, as `serve_with_bounded_drain` does.
        let rx = shutdown_tx.subscribe();

        // `watch_shutdown` must return promptly despite the trigger having
        // already fired. If it only awaited `changed()`, it would hang until
        // the sender is dropped (which never happens during serving).
        let result = tokio::time::timeout(Duration::from_millis(500), watch_shutdown(rx)).await;
        assert!(
            result.is_ok(),
            "watch_shutdown hung on an already-fired trigger"
        );
    }
    /// Regression (issue #329): the SIGTERM handler must be registered
    /// synchronously by `spawn_os_signal_shutdown`, before the spawned task
    /// is first polled. Previously the handler was installed inside the
    /// task, so a SIGTERM arriving in the window between
    /// `spawn_os_signal_shutdown` and the task's first poll hit the default
    /// disposition and killed the process — the `e2e_sigterm_exits_promptly`
    /// flake under parallel load, where the health listener became ready
    /// before the signal task was scheduled.
    ///
    /// The real SIGTERM is sent to an isolated child process (this test
    /// re-executed with a marker env var) rather than to this process:
    /// tokio's OS-signal listeners are process-global — every listener
    /// registered for a signal kind receives the notification — so a SIGTERM
    /// delivered here would also fire the SIGTERM/SIGINT listeners that
    /// other tests running concurrently in this binary install via
    /// `serve_with_bounded_drain` (`test_serve_outlives_drain_timeout`,
    /// `test_server_exits_after_stop`), shutting their servers down
    /// mid-test. In the child there are no other listeners, and if the
    /// regression returns the child dies from the default disposition
    /// (signal 15) exactly as the original flake did.
    #[cfg(unix)]
    #[test]
    fn test_sigterm_registered_before_spawn_returns() {
        const CHILD_ENV: &str = "MIMIR_SIGTERM_REGRESSION_CHILD";
        const CHILD_OK: &str = "mimir-sigterm-regression-child-ok";
        const TEST_NAME: &str = "test_sigterm_registered_before_spawn_returns";

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

            let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
            spawn_os_signal_shutdown(shutdown_tx);

            // No await between the call and the signal: if the handler were
            // only installed when the spawned task is polled (the bug), this
            // SIGTERM kills the child via the default disposition (signal
            // 15) before the assertion below runs.
            nix::sys::signal::kill(nix::unistd::getpid(), nix::sys::signal::Signal::SIGTERM)
                .expect("kill(SIGTERM) failed");

            let result = tokio::time::timeout(Duration::from_secs(5), shutdown_rx.changed()).await;
            assert!(
                result.is_ok(),
                "SIGTERM sent immediately after spawn_os_signal_shutdown was not caught by the handler"
            );
            assert!(*shutdown_rx.borrow(), "shutdown trigger did not fire");
            println!("{CHILD_OK}");
        });
    }
    /// Regression: the drain timeout must bound only the post-signal drain
    /// phase, NOT the serving lifetime. Previously `tokio::time::timeout`
    /// wrapped the entire server future, so the daemon self-terminated
    /// `drain_timeout` after start even with no shutdown requested.
    #[tokio::test]
    async fn test_serve_outlives_drain_timeout() {
        use axum::routing::get;
        use std::time::Duration;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let app = axum::Router::new().route("/health", get(|| async { "ok" }));
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let trigger = shutdown_tx.clone();

        // 100 ms drain bound — far shorter than the 300 ms alive check below.
        let drain_timeout = Duration::from_millis(100);
        let handle = tokio::spawn(async move {
            serve_with_bounded_drain(listener, app, shutdown_tx, drain_timeout).await
        });

        // Wait until the server accepts connections (up to 5 s).
        let poll_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while tokio::time::Instant::now() < poll_deadline {
            if handle.is_finished() {
                let result = handle.await.unwrap();
                panic!("server exited early (before alive check): {:?}", result);
            }
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(ready, "server did not become reachable within 5 seconds");

        // Sleep well past the drain timeout. If the timeout bounded the
        // serving lifetime (the bug), the server would already be dead.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !handle.is_finished(),
            "server self-terminated after the drain timeout with no shutdown requested (the bug)"
        );

        // Request shutdown; the server should exit within the drain timeout.
        trigger.send(true).expect("send shutdown trigger");
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "server did not exit within 2s of shutdown");
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
    }
}
