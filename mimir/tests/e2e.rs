mod common;

use common::TestDaemon;
use std::process::Command;
use std::time::Duration;

#[test]
fn e2e_ask_no_stream_round_trip() {
    let daemon = TestDaemon::start();

    // Run `mimir ask --no-stream hello` against the daemon.
    let (stdout, stderr, status) = daemon.run_cli(&["ask", "--no-stream", "hello"]);
    assert!(
        status.success(),
        "mimir ask exited with failure.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Hello from mock LLM!"),
        "expected mock response in stdout.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // Stop the daemon.
    daemon.stop();
}

/// Smoke test for the SIGTERM shutdown path.
///
/// systemd stops `mimir.service` with SIGTERM. The daemon must exit promptly
/// (well within `TimeoutStopSec`), otherwise systemd aborts it with SIGABRT —
/// the "it stops itself" symptom. Historically this path hung because the
/// config file-watcher `spawn_blocking` thread was only released by an
/// `AppState`-drop race during runtime teardown; under load that race lost
/// and `BlockingPool::shutdown` deadlocked. The fix broadcasts the shutdown
/// watch deterministically before the runtime drops. This test spawns the
/// real binary under an isolated environment, sends SIGTERM, and asserts it
/// exits within a few seconds.
#[test]
fn e2e_sigterm_exits_promptly() {
    use std::process::Stdio;

    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("config");
    let data_dir = temp.path().join("data");
    let home_dir = temp.path().join("home");
    std::fs::create_dir_all(config_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(data_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(&home_dir).unwrap();

    // Reserve a free port by pre-binding, then write it into the config so the
    // subprocess (which reads config from disk) listens on a known address.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let db_path = data_dir.join("mimir").join("context.db");
    let kg_db_path = data_dir.join("mimir").join("knowledge.db");
    let jobs_db_path = data_dir.join("mimir").join("jobs.db");
    let config_toml = format!(
        r#"
[llm]
endpoint = "http://127.0.0.1:1"
api_key = "test"
model = "gpt-4o"
temperature = 0.0

[server]
bind_addr = "127.0.0.1:{port}"

[context]
db_path = "{db}"

[knowledge]
db_path = "{kg_db}"

[scheduler]
db_path = "{jobs_db}"
"#,
        port = port,
        db = db_path.display(),
        kg_db = kg_db_path.display(),
        jobs_db = jobs_db_path.display(),
    );
    std::fs::write(config_dir.join("mimir").join("config.toml"), config_toml).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .arg("start")
        .env("NO_COLOR", "1")
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("XDG_DATA_HOME", &data_dir)
        .env("HOME", &home_dir)
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mimir start");

    let base_url = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    // Wait for the daemon to become ready via the cheap /health endpoint.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("mimir start exited early: {status}");
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

    // Send SIGTERM, exactly as systemd does on stop.
    // nix::sys::signal::kill is a safe wrapper around the unsafe libc call, so
    // the project's no-unsafe guarantee is preserved even in tests.
    let pid = nix::unistd::Pid::from_raw(child.id() as i32);
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("kill(SIGTERM) failed");

    // The process must exit promptly; before the fix it hung indefinitely.
    let exit_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        if std::time::Instant::now() >= exit_deadline {
            // Reap the child to avoid a zombie before failing.
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "mimir start did not exit within 10 seconds of SIGTERM — \
                 the graceful-shutdown path (SIGABRT on stop) has regressed"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        status.success(),
        "mimir start exited with failure after SIGTERM: {status}"
    );
}
