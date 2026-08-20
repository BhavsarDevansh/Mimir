//! Test-only helpers for `mimir-server` unit tests.
//!
//! Compiled only under `cfg(test)`; never part of production builds.

/// Re-execute the current test binary as an isolated child process running
/// `module::test_name` with `child_env` set, and assert the child passed and
/// printed `child_ok`.
///
/// Used by signal-registration regression tests, which must deliver real OS
/// signals to a process with no other signal listeners: tokio's OS-signal
/// listeners are process-global, so every listener registered for a signal
/// kind receives the notification, and a signal sent to the test process
/// itself would fire listeners that other tests running concurrently in this
/// binary install. The child prints `child_ok` only when it actually ran the
/// assertion, so a future rename that makes the `--exact` filter match
/// nothing fails loudly instead of passing silently. libtest names tests
/// crate-relative (no crate-name prefix), but `module_path!()` includes the
/// crate — the caller passes its own `module_path!()` and the first segment
/// is stripped here.
pub(crate) fn run_child_regression_test(
    module: &str,
    test_name: &str,
    child_env: &str,
    child_ok: &str,
) {
    let module = module.split_once("::").map_or(module, |(_, rest)| rest);
    let filter = format!("{module}::{test_name}");
    let output = std::process::Command::new(std::env::current_exe().expect("current_exe"))
        .arg("--exact")
        .arg(&filter)
        .arg("--nocapture")
        .env(child_env, "1")
        .output()
        .expect("failed to spawn the child regression process");
    assert!(
        output.status.success(),
        "{test_name} child regression failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(child_ok),
        "{test_name} child regression did not run the assertion"
    );
}
