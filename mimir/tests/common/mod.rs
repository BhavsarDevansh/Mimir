//! Shared fixtures for the `mimir` binary integration tests.
//!
//! [`TestDaemon`] starts the real daemon in-process (mock LLM + the
//! `mock-connector` feature so the `email/test` harness backend is
//! registered) on a reserved loopback port, isolated in a temp
//! HOME/XDG layout, so tests can drive the `mimir` CLI binary against it
//! via `MIMIR_BASE_URL`.

use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use mimir_core::config::{Config, ReloadableConfig};
use mimir_core::llm::MockLlmClient;

/// Render a filesystem path for interpolation into a TOML basic string.
///
/// Windows paths contain backslashes, which TOML treats as escape
/// introducers (e.g. `\U` is an invalid escape), so they must be doubled
/// before the path is embedded in the quoted `config.toml` template
/// (PR #503 review).
fn toml_escape_path(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

/// Base URL of a deterministically unreachable daemon endpoint.
///
/// TCP port 0 can never be a listening port: binding port 0 asks the kernel
/// for an ephemeral port, so no process can ever listen on `127.0.0.1:0`
/// and every connection attempt fails deterministically.
///
/// Used by the daemon-down CLI tests: pointing `MIMIR_BASE_URL` here makes
/// "daemon unreachable" assertions independent of any real or leftover
/// daemon on the default base URL (issue #384).
#[allow(dead_code)]
pub fn unreachable_daemon_base_url() -> &'static str {
    "http://127.0.0.1:0"
}

/// Spawn the real `mimir` binary with an isolated environment: temp
/// HOME/XDG dirs, the given `MIMIR_BASE_URL`, optional piped stdin and extra
/// env vars, and connector-secret env vars stripped. The temp dir lives for
/// the child's lifetime and the captured output is returned once it exits.
/// Shared by the wiremock-backed CLI tests and the daemon-down tests so the
/// isolation environment never drifts (issue #384).
#[allow(dead_code)]
pub fn spawn_mimir_cli(
    args: &[&str],
    base_url: &str,
    stdin_bytes: Option<&[u8]>,
    envs: &[(&str, &str)],
) -> std::process::Output {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_dir = temp.path().join("config");
    let data_dir = temp.path().join("data");
    let home_dir = temp.path().join("home");
    std::fs::create_dir_all(config_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(data_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(&home_dir).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mimir"));
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env("MIMIR_BASE_URL", base_url)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("XDG_DATA_HOME", &data_dir)
        .env("HOME", &home_dir)
        .env_remove("MIMIR_CONNECTOR_PASSWORD")
        .env_remove("MIMIR_CONNECTOR_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in envs {
        command.env(key, value);
    }
    match stdin_bytes {
        Some(bytes) => {
            use std::io::Write;
            let mut child = command.stdin(Stdio::piped()).spawn().expect("spawn mimir");
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(bytes)
                .expect("write stdin");
            child.wait_with_output().expect("wait for mimir")
        }
        None => command.stdin(Stdio::null()).output().expect("spawn mimir"),
    }
}

/// A running in-process daemon plus the environment a CLI subprocess needs
/// to talk to it.
pub struct TestDaemon {
    pub rt: tokio::runtime::Runtime,
    pub server_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    pub base_url: String,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub home_dir: PathBuf,
    pub _temp: tempfile::TempDir,
}

impl Drop for TestDaemon {
    /// Kill-on-drop: a test that panics before [`stop`](Self::stop) must not
    /// leak a live daemon holding the reserved port and temp DB handles into
    /// parallel suites (issue #384).
    fn drop(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }
}

impl TestDaemon {
    /// Start the daemon and wait until `GET /health` responds (up to 20 s).
    /// Shared fixture: not every test binary uses every helper, so
    /// dead-code analysis is relaxed.
    #[allow(dead_code)]
    pub fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("config");
        let data_dir = temp.path().join("data");
        let home_dir = temp.path().join("home");
        std::fs::create_dir_all(config_dir.join("mimir")).unwrap();
        std::fs::create_dir_all(data_dir.join("mimir")).unwrap();
        std::fs::create_dir_all(&home_dir).unwrap();

        let context_db = data_dir.join("mimir").join("context.db");
        let kg_db = data_dir.join("mimir").join("knowledge.db");
        let jobs_db = data_dir.join("mimir").join("jobs.db");
        let socket_path = data_dir.join("mimir").join("mimir.sock");
        let config_toml = format!(
            r#"
[llm]
endpoint = "http://127.0.0.1:1"
api_key = "test"
model = "gpt-4o"
max_tokens = 10
temperature = 0.0

[server]
bind_addr = "127.0.0.1:0"
socket_path = "{socket_path}"

[memory]
char_limit = 10000

[context]
db_path = "{context_db}"

[knowledge]
db_path = "{kg_db}"

[scheduler]
db_path = "{jobs_db}"
"#,
            context_db = toml_escape_path(&context_db),
            kg_db = toml_escape_path(&kg_db),
            jobs_db = toml_escape_path(&jobs_db),
            socket_path = toml_escape_path(&socket_path),
        );
        std::fs::write(config_dir.join("mimir").join("config.toml"), config_toml).unwrap();

        // Pre-bind a listener to reserve a free port.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}");

        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat("Hello from mock LLM!", mimir_core::llm::Usage::default())
                .build(),
        );

        let mut config = Config::default();
        config.llm.endpoint = "http://127.0.0.1:1".to_string();
        config.llm.api_key = "test".to_string();
        config.llm.model = "gpt-4o".to_string();
        config.llm.max_tokens = Some(10);
        config.llm.temperature = 0.0;
        config.server.bind_addr = format!("127.0.0.1:{port}");
        config.server.socket_path = Some(socket_path.display().to_string());
        config.context.db_path = Some(context_db.clone());
        config.knowledge.db_path = Some(kg_db.clone());
        config.scheduler.db_path = Some(jobs_db.clone());

        let config = Arc::new(ReloadableConfig::new(
            config,
            config_dir.join("mimir").join("config.toml"),
        ));
        // The CLI subprocesses resolve the token from `$XDG_DATA_HOME/mimir`,
        // so create it at the same path the daemon would use in production.
        let api_token = mimir_core::auth::load_or_create_api_token_at(
            &data_dir.join("mimir").join("api_token"),
        )
        .expect("test API token must be creatable");
        let server_handle = rt.spawn(async move {
            mimir_server::start_server_with_llm_and_listener(
                config,
                mock,
                listener,
                Arc::from(api_token.as_str()),
            )
            .await
        });

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut ready = false;
        while std::time::Instant::now() < deadline {
            if server_handle.is_finished() {
                let result = rt.block_on(server_handle);
                panic!("server exited early: {:?}", result);
            }
            if let Ok(resp) = client.get(format!("{base_url}/health")).send()
                && resp.status().is_success()
            {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ready, "daemon did not become ready within 20 seconds");

        Self {
            rt,
            server_handle: Some(server_handle),
            base_url,
            config_dir,
            data_dir,
            home_dir,
            _temp: temp,
        }
    }

    /// Run the `mimir` CLI binary against this daemon with the isolated
    /// environment, returning (stdout, stderr, status).
    pub fn run_cli(&self, args: &[&str]) -> (String, String, ExitStatus) {
        self.run_cli_with_env(args, &[])
    }

    /// Run the CLI without `MIMIR_BASE_URL` so transport resolution follows
    /// the daemon's socket: `MIMIR_SERVER_SOCKET_PATH` → `server.socket_path`
    /// → default `<data_dir>/mimir.sock` (the UDS case, issue #25). Every
    /// other fixture call pins `MIMIR_BASE_URL`, so this is the only way the
    /// integration tests exercise the Unix-socket path end to end (PR #503
    /// review). Shared fixture: not every test binary uses this helper, so
    /// dead-code analysis is relaxed.
    #[allow(dead_code)]
    pub fn run_cli_uds(&self, args: &[&str]) -> (String, String, ExitStatus) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mimir"));
        command
            .args(args)
            .env("NO_COLOR", "1")
            .env_remove("MIMIR_BASE_URL")
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .env("XDG_DATA_HOME", &self.data_dir)
            .env("HOME", &self.home_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().expect("spawn mimir");
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status,
        )
    }

    /// Like [`run_cli`](Self::run_cli) but with extra environment variables
    /// (e.g. `BROWSER` for the OAuth PKCE E2E, which drives the flow through
    /// a fake browser).
    pub fn run_cli_with_env(
        &self,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> (String, String, ExitStatus) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mimir"));
        command
            .args(args)
            .env("NO_COLOR", "1")
            .env("MIMIR_BASE_URL", &self.base_url)
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .env("XDG_DATA_HOME", &self.data_dir)
            .env("HOME", &self.home_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let output = command.output().expect("spawn mimir");
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status,
        )
    }

    /// Run the CLI and parse stdout as JSON, panicking with the full output on
    /// failure. Commands must be invoked with `--json` (or otherwise print a
    /// single JSON document to stdout). Shared fixture: not every test binary
    /// uses every helper, so dead-code analysis is relaxed for this one.
    #[allow(dead_code)]
    pub fn run_cli_json(&self, args: &[&str]) -> serde_json::Value {
        let (stdout, stderr, status) = self.run_cli(args);
        assert!(
            status.success(),
            "`mimir {}` failed.\nstdout: {stdout}\nstderr: {stderr}",
            args.join(" ")
        );
        serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
            panic!(
                "`mimir {}` did not print JSON.\nstdout: {stdout}\nstderr: {stderr}\nerror: {error}",
                args.join(" ")
            )
        })
    }

    /// Stop the daemon via `mimir stop` and await server exit (up to 5 s).
    /// Shared fixture: not every test binary uses every helper, so
    /// dead-code analysis is relaxed.
    #[allow(dead_code)]
    pub fn stop(mut self) {
        let (_, stderr, status) = self.run_cli(&["stop"]);
        assert!(status.success(), "mimir stop failed: {stderr}");
        let server_handle = self.server_handle.take().expect("server handle");
        let result = self
            .rt
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), server_handle).await });
        assert!(result.is_ok(), "server did not exit within 5 seconds");
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::toml_escape_path;
    use std::path::Path;

    #[test]
    fn toml_escape_path_doubles_windows_backslashes() {
        assert_eq!(
            toml_escape_path(Path::new(r"C:\Users\dev\Mimir\mimir.sock")),
            r"C:\\Users\\dev\\Mimir\\mimir.sock"
        );
    }

    #[test]
    fn toml_escape_path_leaves_unix_paths_unchanged() {
        assert_eq!(
            toml_escape_path(Path::new("/home/dev/.local/share/mimir/mimir.sock")),
            "/home/dev/.local/share/mimir/mimir.sock"
        );
    }
}
