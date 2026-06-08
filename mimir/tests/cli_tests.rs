use std::io::Write;
use std::process::Command;

/// Helper: run the mimir binary with given args and return stdout + stderr.
fn run_mimir(args: &[&str]) -> (String, String, std::process::ExitStatus) {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run mimir: {}", e));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status)
}

// ---------------------------------------------------------------------------
// status tests
// ---------------------------------------------------------------------------

#[test]
fn test_status_fails_when_server_down() {
    let (stdout, stderr, status) = run_mimir(&["status"]);
    assert!(
        !status.success(),
        "mimir status should fail when daemon is not running"
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("error") || combined.contains("Error"),
        "should report an error when daemon is unreachable, got: {}",
        combined
    );
}

// ---------------------------------------------------------------------------
// stop tests
// ---------------------------------------------------------------------------

#[test]
fn test_stop_when_server_down() {
    let (stdout, stderr, status) = run_mimir(&["stop"]);
    assert!(
        !status.success(),
        "mimir stop should fail when daemon is not running"
    );
    assert_eq!(status.code(), Some(1), "exit code should be 1");
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("Mimir is not running."),
        "should report daemon not running, got: {}",
        combined
    );
}

// ---------------------------------------------------------------------------
// memory tests
// ---------------------------------------------------------------------------

#[test]
fn test_memory_fails_when_server_down() {
    let (stdout, stderr, status) = run_mimir(&["memory"]);
    assert!(
        !status.success(),
        "mimir memory should fail when daemon is not running"
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("error") || combined.contains("Error"),
        "should report an error when daemon is unreachable, got: {}",
        combined
    );
}

// ---------------------------------------------------------------------------
// ask piping tests
// ---------------------------------------------------------------------------

#[test]
fn test_ask_piped_input_detection() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["ask"])
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    {
        let stdin = child.stdin.take().expect("stdin not available");
        let mut stdin = std::io::BufWriter::new(stdin);
        stdin.write_all(b"hello\n").unwrap();
    }

    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    // When the daemon is not running, the piped input "hello" is consumed by
    // the daemon guard's prompt reader, which interprets it as a declined
    // prompt. Therefore the error originates from the daemon guard (prompt
    // handling) rather than from query validation.
    assert!(
        !stderr.contains("no query provided"),
        "piped ask should not complain about no query: {}",
        stderr
    );
}

#[test]
fn test_ask_empty_query_no_pipe() {
    // Without a running daemon, the command fails at the daemon guard before
    // it ever reaches the empty-query validation.
    let (stdout, stderr, status) = run_mimir(&["ask", ""]);
    assert!(
        !status.success(),
        "mimir ask with empty query should exit with failure"
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("error") || combined.contains("Error"),
        "should report an error when daemon is unreachable, got: {}",
        combined
    );
}

// ---------------------------------------------------------------------------
// start tests
// ---------------------------------------------------------------------------

#[test]
fn test_start_starts_server() {
    // In the mono-binary architecture, `mimir start` runs the server
    // in-process. We can't fully test this without a running LLM endpoint,
    // but we can verify the command is recognised by checking --help.
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["start", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "mimir start --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("Start the Mimir"),
        "start --help should describe the command"
    );
}

// ---------------------------------------------------------------------------
// flag parsing tests (no LLM call)
// ---------------------------------------------------------------------------

#[test]
fn test_ask_incognito_flag_accepted() {
    // Validate via --help that the flag is recognised by clap.
    // This avoids making a real LLM API call that would break in CI.
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["ask", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mimir ask --help exited with non-zero status"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("--incognito"),
        "ask --help should list --incognito flag"
    );
}

#[test]
fn test_ask_no_stream_flag_accepted() {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["ask", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mimir ask --help exited with non-zero status"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("--no-stream"),
        "ask --help should list --no-stream flag"
    );
}

// ---------------------------------------------------------------------------
// chat help
// ---------------------------------------------------------------------------

#[test]
fn test_chat_help_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["chat", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mimir chat --help exited with non-zero status"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("interactive"),
        "chat help should describe the command"
    );
}

// ---------------------------------------------------------------------------
// stop help
// ---------------------------------------------------------------------------

#[test]
fn test_stop_help_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["stop", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mimir stop --help exited with non-zero status"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("Stop the Mimir"),
        "stop --help should describe the command"
    );
}

// ---------------------------------------------------------------------------
// memory flag parsing tests (no daemon required)
// ---------------------------------------------------------------------------

#[test]
fn test_memory_refresh_flag_accepted() {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["memory", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mimir memory --help exited with non-zero status"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        stdout.contains("--refresh"),
        "memory --help should list --refresh flag"
    );
}
