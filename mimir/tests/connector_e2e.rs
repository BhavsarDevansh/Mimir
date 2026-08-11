//! End-to-end `mimir connector` cycle against a real in-process daemon.
//!
//! Uses the `gmail/test` mock connector (registered in the daemon via the
//! `mock-connector` feature, enabled in this crate's dev-dependencies) so
//! the full add → status → resume → sync → pause → resume → remove cycle
//! runs without any external service — the acceptance criterion of
//! issue #204.

mod common;

use common::TestDaemon;

#[test]
fn connector_full_lifecycle_cycle() {
    let daemon = TestDaemon::start();

    // add → the instance starts in Setup/Unauthenticated.
    let (stdout, stderr, status) = daemon.run_cli(&[
        "connector",
        "add",
        "gmail",
        "--backend",
        "test",
        "--slug",
        "demo",
        "--name",
        "Demo",
        "--json",
    ]);
    assert!(
        status.success(),
        "connector add failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let created: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(created["slug"], "demo");
    assert_eq!(created["status"], "setup");
    assert_eq!(created["auth_state"], "unauthenticated");
    let id = created["id"].as_i64().unwrap();

    // status shows the instance.
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "status", "demo", "--json"]);
    assert!(
        status.success(),
        "connector status failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let shown: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(shown["id"], id);
    assert_eq!(shown["status"], "setup");

    // auth completes the credential ingest on the existing instance (no
    // remove + re-add), flipping auth_state to authenticated.
    let (stdout, stderr, status) = daemon.run_cli(&[
        "connector",
        "auth",
        "demo",
        "--token",
        "test-token",
        "--json",
    ]);
    assert!(
        status.success(),
        "connector auth failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let authed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(authed["id"], id);
    assert_eq!(authed["auth_state"], "authenticated");
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "status", "demo", "--json"]);
    assert!(
        status.success(),
        "connector status after auth failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let shown: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(shown["auth_state"], "authenticated");

    // list includes it.
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "list", "--json"]);
    assert!(
        status.success(),
        "connector list failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("\"demo\""),
        "list should include demo:\n{stdout}"
    );

    // resume activates the runner.
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "resume", "demo", "--json"]);
    assert!(
        status.success(),
        "connector resume failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let resumed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(resumed["status"], "active");

    // sync runs a manual cycle against the mock backend.
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "sync", "demo", "--json"]);
    assert!(
        status.success(),
        "connector sync failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let synced: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(synced["status"], "ok");
    assert_eq!(synced["fetched"], 0);

    // pause stops the runner; resume re-activates it.
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "pause", "demo", "--json"]);
    assert!(
        status.success(),
        "connector pause failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let paused: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(paused["status"], "paused");
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "resume", "demo"]);
    assert!(
        status.success(),
        "second resume failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("resumed"),
        "expected resume summary:\n{stdout}"
    );

    // remove tears the instance down (provenance detached; no facts existed).
    let (stdout, stderr, status) = daemon.run_cli(&["connector", "remove", "demo", "--yes"]);
    assert!(
        status.success(),
        "connector remove failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("removed"),
        "expected remove summary:\n{stdout}"
    );

    // forget is the cascade variant: add a second instance and forget it.
    let (stdout, stderr, status) = daemon.run_cli(&[
        "connector",
        "add",
        "gmail",
        "--backend",
        "test",
        "--slug",
        "demo2",
    ]);
    assert!(
        status.success(),
        "second connector add failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let (stdout, stderr, status) =
        daemon.run_cli(&["connector", "forget", "demo2", "--yes", "--json"]);
    assert!(
        status.success(),
        "connector forget failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let forgotten: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(forgotten["forgotten_count"], 0);

    daemon.stop();
}
