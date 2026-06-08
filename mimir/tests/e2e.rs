use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use mimir_core::config::{Config, ReloadableConfig};
use mimir_core::llm::MockLlmClient;

#[test]
fn e2e_ask_no_stream_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("config");
    let data_dir = temp.path().join("data");
    let home_dir = temp.path().join("home");
    std::fs::create_dir_all(config_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(data_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(&home_dir).unwrap();

    // Ensure the in-process server uses the temp directories.
    std::env::set_var("XDG_CONFIG_HOME", config_dir.to_str().unwrap());
    std::env::set_var("XDG_DATA_HOME", data_dir.to_str().unwrap());
    std::env::set_var("HOME", home_dir.to_str().unwrap());

    let db_path = data_dir.join("mimir").join("context.db");

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
db_path = "{}"
"#,
        db_path.display(),
    );

    std::fs::write(config_dir.join("mimir").join("config.toml"), config_toml).unwrap();

    // Pre-bind a listener to reserve a free port.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);

    // Configure mock LLM.
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
    config.server.bind_addr = format!("127.0.0.1:{}", port);
    config.context.db_path = Some(db_path.clone());

    // Start the daemon in-process.
    let config = Arc::new(ReloadableConfig::new(
        config,
        config_dir.join("mimir").join("config.toml"),
    ));
    let server_handle = rt.spawn(async move {
        mimir_server::start_server_with_llm_and_listener(config, mock, listener).await
    });

    // Poll /status until ready (up to 10 s).
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
        if let Ok(resp) = client.get(format!("{}/status", base_url)).send()
            && resp.status().is_success()
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "daemon did not become ready within 20 seconds");

    // Run `mimir ask --no-stream hello` against the daemon.
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["ask", "--no-stream", "hello"])
        .env("NO_COLOR", "1")
        .env("MIMIR_BASE_URL", &base_url)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("XDG_DATA_HOME", &data_dir)
        .env("HOME", &home_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn mimir ask");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mimir ask exited with failure.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        stdout.contains("Hello from mock LLM!"),
        "expected mock response in stdout.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // Stop the daemon.
    let stop = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .arg("stop")
        .env("NO_COLOR", "1")
        .env("MIMIR_BASE_URL", &base_url)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("XDG_DATA_HOME", &data_dir)
        .env("HOME", &home_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn mimir stop");

    assert!(
        stop.status.success(),
        "mimir stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );

    // Wait for the server task to exit (up to 5 s).
    let result =
        rt.block_on(async { tokio::time::timeout(Duration::from_secs(5), server_handle).await });
    assert!(result.is_ok(), "server did not exit within 5 seconds");
    assert!(
        result.unwrap().is_ok(),
        "server task panicked or returned error"
    );
}
