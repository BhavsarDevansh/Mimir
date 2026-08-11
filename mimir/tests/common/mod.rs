//! Shared fixtures for the `mimir` binary integration tests.
//!
//! [`TestDaemon`] starts the real daemon in-process (mock LLM + the
//! `mock-connector` feature so the `gmail/test` harness backend is
//! registered) on a reserved loopback port, isolated in a temp
//! HOME/XDG layout, so tests can drive the `mimir` CLI binary against it
//! via `MIMIR_BASE_URL`.

use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use mimir_core::config::{Config, ReloadableConfig};
use mimir_core::llm::MockLlmClient;

/// A running in-process daemon plus the environment a CLI subprocess needs
/// to talk to it.
pub struct TestDaemon {
    pub rt: tokio::runtime::Runtime,
    pub server_handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    pub base_url: String,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub home_dir: PathBuf,
    pub _temp: tempfile::TempDir,
}

impl TestDaemon {
    /// Start the daemon and wait until `GET /health` responds (up to 20 s).
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

[memory]
char_limit = 10000

[context]
db_path = "{context_db}"

[knowledge]
db_path = "{kg_db}"

[scheduler]
db_path = "{jobs_db}"
"#,
            context_db = context_db.display(),
            kg_db = kg_db.display(),
            jobs_db = jobs_db.display(),
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
        config.context.db_path = Some(context_db.clone());
        config.knowledge.db_path = Some(kg_db.clone());
        config.scheduler.db_path = Some(jobs_db.clone());

        let config = Arc::new(ReloadableConfig::new(
            config,
            config_dir.join("mimir").join("config.toml"),
        ));
        let server_handle = rt.spawn(async move {
            mimir_server::start_server_with_llm_and_listener(config, mock, listener).await
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
            server_handle,
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
        let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
            .args(args)
            .env("NO_COLOR", "1")
            .env("MIMIR_BASE_URL", &self.base_url)
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .env("XDG_DATA_HOME", &self.data_dir)
            .env("HOME", &self.home_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn mimir");
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status,
        )
    }

    /// Stop the daemon via `mimir stop` and await server exit (up to 5 s).
    pub fn stop(self) {
        let (_, stderr, status) = self.run_cli(&["stop"]);
        assert!(status.success(), "mimir stop failed: {stderr}");
        let result = self.rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), self.server_handle).await
        });
        assert!(result.is_ok(), "server did not exit within 5 seconds");
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
    }
}
