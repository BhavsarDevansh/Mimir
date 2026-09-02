//! Shared helper to ensure the Mimir daemon is running before client commands.
//!
//! Provides [`ensure_daemon_running`] which probes the daemon via the lightweight
//! transport check (the `GET /health` endpoint over TCP, or a connection
//! attempt on the Unix socket — kept separate from the heavyweight
//! `/status`). If the daemon is unreachable, it prompts the user to auto-start
//! it, spawns `mimir start`, and polls with exponential backoff.

use std::future::Future;
use std::io::{BufRead, Write};
use std::path::Path;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::transport::DaemonTransport;

/// Errors that can occur during daemon guard checks.
#[derive(Debug, Error)]
pub enum DaemonGuardError {
    /// Failed to read user prompt response.
    #[error("prompt error: {0}")]
    Prompt(String),
    /// Failed to spawn the daemon process.
    #[error("spawn error: {0}")]
    Spawn(String),
    /// Daemon did not become ready within the timeout.
    #[error("daemon start timeout")]
    StartTimeout,
    /// Reserved for unexpected probe-level failures (currently unused).
    #[error("connection error: {0}")]
    #[allow(dead_code)] // intentionally reserved; not used by current probe impl
    Connection(String),
}

/// Ensure the Mimir daemon is running.
///
/// 1. Fast-probe the resolved transport with a 500 ms timeout (a connect
///    attempt on the Unix socket, or `GET /health` over TCP).
/// 2. If the daemon is reachable, return `Ok(())` immediately.
/// 3. If not, print an error and prompt the user to start it.
/// 4. On approval, spawn `mimir start` and poll the same transport probe with
///    exponential backoff until it comes up or a 10 s wall-clock timeout
///    expires.
/// 5. `already_tried` prevents more than one auto-start attempt per CLI
///    invocation.
pub async fn ensure_daemon_running(
    transport: &DaemonTransport,
    already_tried: &mut bool,
) -> Result<(), DaemonGuardError> {
    let guard = DaemonGuard::default();
    guard.ensure_running(transport, already_tried).await
}

/// Non-interactive reachability check for the daemon.
pub async fn check_daemon_reachable(transport: &DaemonTransport) -> bool {
    TransportProbe.check(transport).await
}

// ---------------------------------------------------------------------------
// Internal abstractions (trait-based so unit tests can inject mocks).
// ---------------------------------------------------------------------------

/// Trait for probing the daemon over the resolved transport (a connect
/// attempt on the Unix socket, or the TCP `GET /health` endpoint).
trait Probe: Send + Sync {
    fn check<'a>(
        &'a self,
        transport: &'a DaemonTransport,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
}

/// Trait for reading a line of user input.
trait PromptReader: Send + Sync {
    fn read_line(&self) -> Result<String, DaemonGuardError>;
}

/// Trait for spawning the daemon child process.
trait ProcessSpawner: Send + Sync {
    fn spawn(&self, exe: &Path) -> Result<(), DaemonGuardError>;
}

// ---------------------------------------------------------------------------
// Production implementations
// ---------------------------------------------------------------------------

static PROBE_CLIENT: LazyLock<Result<reqwest::Client, reqwest::Error>> =
    LazyLock::new(|| reqwest::Client::builder().build());

struct TransportProbe;

impl Probe for TransportProbe {
    fn check<'a>(
        &'a self,
        transport: &'a DaemonTransport,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            match transport {
                #[cfg(unix)]
                DaemonTransport::Unix(path) => {
                    // A connect attempt — not file existence — distinguishes a
                    // live daemon from a stale socket file left by a crashed
                    // one; the local syscall needs no HTTP round trip. Shares
                    // the bounded probe with the daemon's pre-bind
                    // stale-socket check (PR #503 review).
                    mimir_core::config::socket_is_live(path).await
                }
                DaemonTransport::Tcp(base_url) => {
                    let url = format!("{base_url}/health");
                    match PROBE_CLIENT.as_ref() {
                        Ok(client) => {
                            match client
                                .get(&url)
                                .timeout(Duration::from_millis(500))
                                .send()
                                .await
                            {
                                Ok(resp) => resp.status().is_success(),
                                Err(_) => false,
                            }
                        }
                        Err(_) => false,
                    }
                }
            }
        })
    }
}

struct RealPromptReader;

impl PromptReader for RealPromptReader {
    fn read_line(&self) -> Result<String, DaemonGuardError> {
        let mut line = String::new();
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        match lock.read_line(&mut line) {
            Ok(0) => Err(DaemonGuardError::Prompt("EOF".to_string())),
            Ok(_) => Ok(line),
            Err(e) => Err(DaemonGuardError::Prompt(e.to_string())),
        }
    }
}

struct RealProcessSpawner;

/// Build the daemon child command, stripping connector secrets from the
/// inherited environment so they never reach the daemon process.
fn daemon_command(exe: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("start")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .env_remove("MIMIR_CONNECTOR_PASSWORD")
        .env_remove("MIMIR_CONNECTOR_TOKEN");

    // Detach from the parent process group so Ctrl-C in the terminal
    // does not propagate to the background daemon.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd
}

impl ProcessSpawner for RealProcessSpawner {
    fn spawn(&self, exe: &Path) -> Result<(), DaemonGuardError> {
        let mut cmd = daemon_command(exe);
        cmd.spawn()
            .map_err(|e| DaemonGuardError::Spawn(e.to_string()))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DaemonGuard orchestrator
// ---------------------------------------------------------------------------

const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_START_POLL_DELAY: Duration = Duration::from_millis(20);

struct DaemonGuard {
    probe: Box<dyn Probe>,
    prompt_reader: Box<dyn PromptReader>,
    process_spawner: Box<dyn ProcessSpawner>,
    start_timeout: Duration,
}

impl Default for DaemonGuard {
    fn default() -> Self {
        Self {
            probe: Box::new(TransportProbe),
            prompt_reader: Box::new(RealPromptReader),
            process_spawner: Box::new(RealProcessSpawner),
            start_timeout: DEFAULT_START_TIMEOUT,
        }
    }
}

impl DaemonGuard {
    async fn ensure_running(
        &self,
        transport: &DaemonTransport,
        already_tried: &mut bool,
    ) -> Result<(), DaemonGuardError> {
        // 1. Fast probe.
        if self.probe.check(transport).await {
            return Ok(());
        }

        // 2. Prompt user.
        if *already_tried {
            return Err(DaemonGuardError::StartTimeout);
        }
        *already_tried = true;

        eprintln!("Error: Mimir is not running.");
        eprint!("Start the server now? [y/N]: ");
        let _ = std::io::stderr().flush();

        let response = self.prompt_reader.read_line()?;
        let trimmed = response.trim().to_lowercase();
        if trimmed != "y" && trimmed != "yes" {
            return Err(DaemonGuardError::Prompt("declined".to_string()));
        }

        // 3. Spawn daemon.
        let exe = std::env::current_exe().map_err(|e| DaemonGuardError::Spawn(e.to_string()))?;
        self.process_spawner.spawn(&exe)?;

        // 4. Poll with exponential backoff.
        let start = Instant::now();
        let mut delay = (self.start_timeout / 50).max(MIN_START_POLL_DELAY);

        while start.elapsed() < self.start_timeout {
            if self.probe.check(transport).await {
                return Ok(());
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(1));
        }

        Err(DaemonGuardError::StartTimeout)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_command_strips_connector_secrets_from_child_env() {
        let cmd = daemon_command(Path::new("mimir"));
        for var in ["MIMIR_CONNECTOR_PASSWORD", "MIMIR_CONNECTOR_TOKEN"] {
            let entry = cmd.get_envs().find(|(k, _)| *k == var);
            assert!(
                matches!(entry, Some((_, None))),
                "{var} must be removed from the daemon child environment"
            );
        }
    }

    struct MockProbe {
        results: std::sync::Mutex<Vec<bool>>,
    }

    impl Probe for MockProbe {
        fn check<'a>(
            &'a self,
            _transport: &'a DaemonTransport,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            let next = {
                let mut results = self.results.lock().unwrap();
                if results.is_empty() {
                    false
                } else {
                    results.remove(0)
                }
            };
            Box::pin(async move { next })
        }
    }

    struct MockPromptReader {
        response: String,
    }

    impl PromptReader for MockPromptReader {
        fn read_line(&self) -> Result<String, DaemonGuardError> {
            Ok(self.response.clone())
        }
    }

    struct MockProcessSpawner {
        should_succeed: bool,
    }

    impl ProcessSpawner for MockProcessSpawner {
        fn spawn(&self, _exe: &Path) -> Result<(), DaemonGuardError> {
            if self.should_succeed {
                Ok(())
            } else {
                Err(DaemonGuardError::Spawn("mock spawn failed".to_string()))
            }
        }
    }

    fn mock_guard(probe_results: Vec<bool>, prompt: &str, spawn_ok: bool) -> DaemonGuard {
        mock_guard_with_timeout(probe_results, prompt, spawn_ok, DEFAULT_START_TIMEOUT)
    }

    fn mock_guard_with_timeout(
        probe_results: Vec<bool>,
        prompt: &str,
        spawn_ok: bool,
        start_timeout: Duration,
    ) -> DaemonGuard {
        DaemonGuard {
            probe: Box::new(MockProbe {
                results: std::sync::Mutex::new(probe_results),
            }),
            prompt_reader: Box::new(MockPromptReader {
                response: prompt.to_string(),
            }),
            process_spawner: Box::new(MockProcessSpawner {
                should_succeed: spawn_ok,
            }),
            start_timeout,
        }
    }

    /// Spawn a tiny HTTP server that becomes ready after `ready_after`.
    /// Returns a join handle and the base URL.
    async fn spawn_test_server(ready_after: Duration) -> (tokio::task::JoinHandle<()>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{}", port);
        let start = Instant::now();

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                if start.elapsed() < ready_after {
                    // Drop connection immediately so the probe fails.
                    drop(socket);
                    continue;
                }
                let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
                let _ = tokio::io::AsyncWriteExt::shutdown(&mut socket).await;
            }
        });

        (handle, base_url)
    }

    #[tokio::test]
    async fn test_already_running() {
        let guard = mock_guard(vec![true], "", true);
        let mut tried = false;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let result = guard.ensure_running(&transport, &mut tried).await;
        assert!(result.is_ok());
        assert!(!tried);
    }

    #[tokio::test]
    async fn test_prompt_yes_spawns_and_polls() {
        // Daemon down, user approves, spawn succeeds, poll eventually succeeds.
        let guard = mock_guard(vec![false, false, true], "y\n", true);
        let mut tried = false;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let result = guard.ensure_running(&transport, &mut tried).await;
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        assert!(tried);
    }

    #[tokio::test]
    async fn test_http_probe_success() {
        let (handle, base_url) = spawn_test_server(Duration::ZERO).await;
        let probe = TransportProbe;
        let transport = DaemonTransport::Tcp(base_url);
        assert!(probe.check(&transport).await);
        handle.abort();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_http_probe_failure() {
        // No server running on this port.
        let probe = TransportProbe;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        assert!(!probe.check(&transport).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_unix_probe_connects_to_live_socket_and_rejects_stale() {
        let temp = tempfile::tempdir().unwrap();
        let socket_path = temp.path().join("mimir.sock");
        let probe = TransportProbe;

        // A missing socket file means the daemon is not running.
        let missing = DaemonTransport::Unix(socket_path.clone());
        assert!(!probe.check(&missing).await);

        // A live listener is reachable (instant detection — issue #25).
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        assert!(probe.check(&missing).await);

        // A stale socket file (crashed daemon, listener gone) must be
        // detected as down so the guard auto-starts the daemon instead of
        // letting commands fail with connection errors.
        drop(listener);
        assert!(socket_path.exists(), "stale socket file must remain");
        assert!(!probe.check(&missing).await);
    }

    #[tokio::test]
    async fn test_prompt_no() {
        let guard = mock_guard(vec![false], "n\n", true);
        let mut tried = false;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let result = guard.ensure_running(&transport, &mut tried).await;
        assert!(matches!(result, Err(DaemonGuardError::Prompt(_))));
        assert!(tried);
    }

    #[tokio::test]
    async fn test_prompt_eof() {
        let guard = mock_guard(vec![false], "", true);
        let mut tried = false;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let result = guard.ensure_running(&transport, &mut tried).await;
        assert!(matches!(result, Err(DaemonGuardError::Prompt(_))));
        assert!(tried);
    }

    #[tokio::test]
    async fn test_spawn_failure() {
        let guard = mock_guard(vec![false], "y\n", false);
        let mut tried = false;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let result = guard.ensure_running(&transport, &mut tried).await;
        assert!(matches!(result, Err(DaemonGuardError::Spawn(_))));
        assert!(tried);
    }

    #[tokio::test]
    async fn test_start_timeout() {
        let guard = mock_guard_with_timeout(vec![false], "y\n", true, Duration::from_millis(100));
        let mut tried = false;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());

        let started = Instant::now();
        let result = guard.ensure_running(&transport, &mut tried).await;

        assert!(matches!(result, Err(DaemonGuardError::StartTimeout)));
        assert!(tried);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the injectable start timeout should not use the 10 s production default"
        );
    }

    #[tokio::test]
    async fn test_already_tried_skips_prompt() {
        let guard = mock_guard(vec![false], "y\n", true);
        let mut tried = true;
        let transport = DaemonTransport::Tcp("http://127.0.0.1:1".to_string());
        let result = guard.ensure_running(&transport, &mut tried).await;
        assert!(matches!(result, Err(DaemonGuardError::StartTimeout)));
    }
}
