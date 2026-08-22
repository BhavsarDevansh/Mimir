//! Binary-level tests for `mimir personality list` (issue #387).
//!
//! Presets are local data (compiled-in built-ins plus files in the XDG
//! config directory), so the command runs entirely in the CLI process. The
//! tests spawn the real binary with an isolated HOME/XDG layout and never
//! start a daemon.

use std::process::Command;

/// Spawn `mimir personality list` with an isolated HOME/XDG layout and the
/// given files written into `config/mimir` (relative path → contents).
fn run_personality_list(
    config_files: &[(&str, &str)],
) -> (String, String, std::process::ExitStatus) {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("config");
    let data_dir = temp.path().join("data");
    let home_dir = temp.path().join("home");

    for (relative_path, contents) in config_files {
        let path = config_dir.join("mimir").join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["personality", "list"])
        .env("NO_COLOR", "1")
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("XDG_DATA_HOME", &data_dir)
        .env("HOME", &home_dir)
        .output()
        .unwrap();

    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status,
    )
}

#[test]
fn test_personality_list_lists_builtins_with_source() {
    let (stdout, stderr, status) = run_personality_list(&[]);
    assert!(
        status.success(),
        "personality list should succeed: {stderr}"
    );
    for name in ["transparent", "concise", "warm", "formal"] {
        assert!(
            stdout.contains(name),
            "built-in `{name}` missing from:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("Builtin"),
        "source column missing:\n{stdout}"
    );
    assert!(stderr.is_empty(), "no warnings expected, got: {stderr}");
}

#[test]
fn test_personality_list_shows_custom_preset_and_description() {
    let (stdout, stderr, status) = run_personality_list(&[(
        "personalities/cheerful.personality.md",
        "---\ndescription: Cheerful and upbeat\n---\nYou are cheerful!",
    )]);
    assert!(
        status.success(),
        "personality list should succeed: {stderr}"
    );
    assert!(
        stdout.contains("cheerful"),
        "custom preset missing:\n{stdout}"
    );
    assert!(
        stdout.contains("Custom"),
        "custom source missing:\n{stdout}"
    );
    assert!(
        stdout.contains("Cheerful and upbeat"),
        "description missing:\n{stdout}"
    );
    assert!(stderr.is_empty(), "no warnings expected, got: {stderr}");
}

#[test]
fn test_personality_list_uses_dash_for_missing_description() {
    let (stdout, stderr, status) =
        run_personality_list(&[("personalities/plain.personality.md", "You are plain.")]);
    assert!(
        status.success(),
        "personality list should succeed: {stderr}"
    );
    let row = stdout
        .lines()
        .find(|line| line.contains("plain"))
        .expect("custom preset row missing");
    assert!(
        row.contains('-'),
        "missing description should render as '-': {row}"
    );
}

#[test]
fn test_personality_list_warns_on_malformed_file_but_succeeds() {
    let (stdout, stderr, status) = run_personality_list(&[(
        "personalities/broken.personality.md",
        "---\ndescription: Broken\nnever closed",
    )]);
    assert!(
        status.success(),
        "malformed files must not fail the command: {stderr}"
    );
    assert!(
        stderr.contains("broken.personality.md"),
        "stderr should name the malformed file, got: {stderr}"
    );
    assert!(
        !stdout.contains("broken"),
        "malformed preset must be skipped, got:\n{stdout}"
    );
}

#[test]
fn test_personality_list_warns_when_configured_preset_unknown() {
    let (stdout, stderr, status) =
        run_personality_list(&[("config.toml", "[personality]\npreset = \"ghost\"\n")]);
    assert!(
        status.success(),
        "unknown preset must not fail the command: {stderr}"
    );
    assert!(
        stderr.contains("ghost"),
        "stderr should name the unknown configured preset, got: {stderr}"
    );
    assert!(
        stdout.contains("transparent"),
        "built-ins should still list:\n{stdout}"
    );
}

#[test]
fn test_personality_list_help_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(["personality", "list", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "personality list --help should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("List available personality presets"));
}
