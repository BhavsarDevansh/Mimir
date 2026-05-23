use std::io::Write;
use std::process::Command;
use tempfile::tempdir;

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

/// Helper: create a temporary config file at the correct location.
fn write_temp_config(dir: &tempfile::TempDir, content: &str) -> std::path::PathBuf {
    let mimir_dir = dir.path().join("mimir");
    std::fs::create_dir_all(&mimir_dir).unwrap();
    let path = mimir_dir.join("config.toml");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    path
}

/// Helper: create a temporary memory.md file.
fn write_temp_memory(dir: &tempfile::TempDir, content: &str) -> std::path::PathBuf {
    let mimir_dir = dir.path().join("mimir");
    std::fs::create_dir_all(&mimir_dir).unwrap();
    let path = mimir_dir.join("memory.md");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    path
}

// ---------------------------------------------------------------------------
// status tests
// ---------------------------------------------------------------------------

#[test]
fn test_status_output_contains_expected_sections() {
    let (stdout, stderr, status) = run_mimir(&["status"]);
    assert!(status.success(), "mimir status exited with non-zero status");
    let combined = format!("{}{}", stdout, stderr);
    assert!(combined.contains("Config path"), "missing 'Config path'");
    assert!(combined.contains("LLM endpoint"), "missing 'LLM endpoint'");
    assert!(combined.contains("LLM model"), "missing 'LLM model'");
    assert!(combined.contains("Memory path"), "missing 'Memory path'");
    assert!(combined.contains("Memory usage"), "missing 'Memory usage'");
}

#[test]
fn test_status_with_temp_config() {
    let dir = tempdir().unwrap();
    let _config = write_temp_config(
        &dir,
        r#"
[llm]
endpoint = "https://api.example.com/v1"
api_key = "test"
model = "test-model"

[memory]
char_limit = 1000
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["status"])
        .env("NO_COLOR", "1")
        .env("XDG_CONFIG_HOME", dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "mimir status exited with non-zero status"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);
    assert!(combined.contains("https://api.example.com/v1"));
    assert!(combined.contains("test-model"));
}

// ---------------------------------------------------------------------------
// memory tests
// ---------------------------------------------------------------------------

#[test]
fn test_memory_command_output() {
    let (stdout, stderr, status) = run_mimir(&["memory"]);
    assert!(status.success(), "mimir memory exited with non-zero status");
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("Mimir Working Memory") || combined.contains("Failed to load memory"),
        "memory command should output memory content or an error message"
    );
}

#[test]
fn test_memory_with_temp_file() {
    let dir = tempdir().unwrap();
    let _mem_path = write_temp_memory(&dir, "Hello from memory!");

    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["memory"])
        .env("NO_COLOR", "1")
        .env("XDG_CONFIG_HOME", dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "mimir memory exited with non-zero status"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("Hello from memory!"),
        "memory command should show our custom content, got: {}",
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
    assert!(
        !stderr.contains("no query provided"),
        "piped ask should not complain about no query: {}",
        stderr
    );
}

#[test]
fn test_ask_empty_query_no_pipe() {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["ask", ""])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "mimir ask with empty query should exit with failure"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("no query provided"),
        "empty query should print error"
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
