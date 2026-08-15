//! Binary-level tests for the `mimir connector` CLI against a wiremock
//! daemon (full clap → daemon-guard → HTTP-client path).

use std::process::{Command, Stdio};

use mimir_api_types::{
    ConnectorCatalogEntry, ConnectorCatalogResponse, ConnectorListResponse, ConnectorResponse,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn run_mimir(args: &[&str], base_url: &str) -> (String, String, std::process::ExitStatus) {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_dir = temp.path().join("config");
    let data_dir = temp.path().join("data");
    let home_dir = temp.path().join("home");
    std::fs::create_dir_all(config_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(data_dir.join("mimir")).unwrap();
    std::fs::create_dir_all(&home_dir).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(args)
        .env("NO_COLOR", "1")
        .env("MIMIR_BASE_URL", base_url)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("XDG_DATA_HOME", &data_dir)
        .env("HOME", &home_dir)
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

async fn mount_health(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
}

fn connector_fixture(id: i32, slug: &str) -> ConnectorResponse {
    ConnectorResponse {
        id,
        connector_type: "gmail".to_string(),
        slug: slug.to_string(),
        backend: "test".to_string(),
        display_name: slug.to_string(),
        status: "setup".to_string(),
        auth_state: "unauthenticated".to_string(),
        sync_cursor: None,
        last_sync_at: None,
        last_error: None,
        created_at: "2026-08-11T00:00:00Z".to_string(),
        updated_at: "2026-08-11T00:00:00Z".to_string(),
        item_count: 0,
    }
}

#[tokio::test]
async fn connector_list_json_against_daemon() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    let payload = ConnectorListResponse {
        connectors: vec![connector_fixture(1, "demo")],
    };
    Mock::given(method("GET"))
        .and(path("/connectors"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let (stdout, stderr, status) = run_mimir(&["connector", "list", "--json"], &server.uri());
    assert!(
        status.success(),
        "connector list failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let parsed: ConnectorListResponse = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed.connectors.len(), 1);
    assert_eq!(parsed.connectors[0].slug, "demo");
}

#[tokio::test]
async fn connector_catalog_lists_supported_backends() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    let payload = ConnectorCatalogResponse {
        entries: vec![
            ConnectorCatalogEntry {
                connector_type: "calendar".to_string(),
                backend: "caldav".to_string(),
            },
            ConnectorCatalogEntry {
                connector_type: "gmail".to_string(),
                backend: "imap".to_string(),
            },
            ConnectorCatalogEntry {
                connector_type: "photos".to_string(),
                backend: "local".to_string(),
            },
        ],
    };
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    // --json round-trips the wire shape.
    let (stdout, stderr, status) = run_mimir(&["connector", "catalog", "--json"], &server.uri());
    assert!(
        status.success(),
        "connector catalog --json failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let parsed: ConnectorCatalogResponse = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed.entries.len(), 3);
    assert_eq!(parsed.entries[0].backend, "caldav");

    // Plain output renders every pair in a table.
    let (stdout, stderr, status) = run_mimir(&["connector", "catalog"], &server.uri());
    assert!(
        status.success(),
        "connector catalog failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    for needle in ["calendar", "caldav", "gmail", "imap", "photos", "local"] {
        assert!(
            stdout.contains(needle),
            "expected '{needle}' in catalog table, got:\n{stdout}"
        );
    }
}

#[tokio::test]
async fn connector_sync_not_running_hints_at_resume() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    Mock::given(method("GET"))
        .and(path("/connectors"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorListResponse {
                connectors: vec![connector_fixture(1, "demo")],
            }),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/connectors/1/sync"))
        .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
            "error": "connector 1 is not running (status: setup)",
            "code": "CONNECTOR_NOT_RUNNING"
        })))
        .mount(&server)
        .await;

    let (stdout, stderr, status) = run_mimir(&["connector", "sync", "demo"], &server.uri());
    assert!(!status.success(), "sync must fail for a Setup connector");
    assert!(
        stderr.contains("resume demo"),
        "expected the activation hint in stderr, got: {stderr}"
    );
    assert!(stdout.is_empty(), "expected no stdout, got: {stdout}");
}

#[test]
fn connector_list_fails_when_daemon_down() {
    // Port 1 is never bound; the daemon guard must report failure without
    // hanging (stdin is null so the start prompt hits EOF immediately).
    let (stdout, stderr, status) = run_mimir(&["connector", "list"], "http://127.0.0.1:1");
    assert!(
        !status.success(),
        "connector list should fail when the daemon is not running"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("Error") || combined.contains("error"),
        "should report an error when the daemon is unreachable, got: {combined}"
    );
}

#[test]
fn connector_add_rejects_both_flags() {
    let (stdout, stderr, status) = run_mimir(
        &[
            "connector",
            "add",
            "gmail",
            "--backend",
            "test",
            "--password",
            "p",
            "--token",
            "t",
        ],
        "http://127.0.0.1:1",
    );
    assert!(
        !status.success(),
        "add must reject passing both --password and --token"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("cannot be used with '--token <TOKEN>'"),
        "expected the clap both-flags conflict error, got: {combined}"
    );
}

#[tokio::test]
async fn connector_add_ingest_failure_hints_at_auth() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorCatalogResponse {
                entries: vec![ConnectorCatalogEntry {
                    connector_type: "gmail".to_string(),
                    backend: "test".to_string(),
                }],
            }),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .respond_with(ResponseTemplate::new(201).set_body_json(connector_fixture(1, "demo")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/connectors/1/tokens"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": "secret store unavailable",
            "code": "SECRET_STORE_ERROR"
        })))
        .mount(&server)
        .await;

    let (stdout, stderr, status) = run_mimir(
        &[
            "connector",
            "add",
            "gmail",
            "--backend",
            "test",
            "--slug",
            "demo",
            "auth.kind=app_password",
            "--password",
            "hunter2",
            "--json",
        ],
        &server.uri(),
    );
    assert!(
        !status.success(),
        "add must fail when credential ingest fails.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("mimir connector auth demo"),
        "expected the recovery hint in stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("secret store unavailable"),
        "expected the server error detail in stderr, got: {stderr}"
    );
}

#[tokio::test]
async fn connector_add_rejects_unregistered_backend_before_credential_prompt() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    // The daemon supports gmail/imap and photos/local; the user asks for a
    // photos backend that does not exist.
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorCatalogResponse {
                entries: vec![
                    ConnectorCatalogEntry {
                        connector_type: "gmail".to_string(),
                        backend: "imap".to_string(),
                    },
                    ConnectorCatalogEntry {
                        connector_type: "photos".to_string(),
                        backend: "local".to_string(),
                    },
                ],
            }),
        )
        .mount(&server)
        .await;
    // Fail loud if the CLI still POSTs after the pre-flight rejection.
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let (stdout, stderr, status) = run_mimir(
        &[
            "connector",
            "add",
            "photos",
            "--backend",
            "cloud",
            "--password",
            "hunter2",
        ],
        &server.uri(),
    );
    assert!(
        !status.success(),
        "add must reject an unregistered backend.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("does not support backend 'cloud'"),
        "expected a backend-specific rejection, got: {stderr}"
    );
    assert!(
        stderr.contains("local"),
        "expected the supported-backend hint, got: {stderr}"
    );
    assert!(stdout.is_empty(), "expected no stdout, got: {stdout}");

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests.iter().all(|r| r.url.path() != "/connectors"),
        "POST /connectors must not run after a pre-flight rejection"
    );
}

#[tokio::test]
async fn connector_add_rejects_unknown_type_with_supported_pairs_hint() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorCatalogResponse {
                entries: vec![ConnectorCatalogEntry {
                    connector_type: "gmail".to_string(),
                    backend: "imap".to_string(),
                }],
            }),
        )
        .mount(&server)
        .await;
    // Fail loud if the CLI still POSTs after the pre-flight rejection.
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let (stdout, stderr, status) = run_mimir(
        &[
            "connector",
            "add",
            "dropbox",
            "--backend",
            "api",
            "--password",
            "hunter2",
        ],
        &server.uri(),
    );
    assert!(
        !status.success(),
        "add must reject an unknown connector type.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("unknown connector type 'dropbox'"),
        "expected an unknown-type rejection, got: {stderr}"
    );
    assert!(
        stderr.contains("gmail/imap"),
        "expected the supported-pairs hint, got: {stderr}"
    );
    assert!(stdout.is_empty(), "expected no stdout, got: {stdout}");

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests.iter().all(|r| r.url.path() != "/connectors"),
        "POST /connectors must not run after a pre-flight rejection"
    );
}

#[tokio::test]
async fn connector_add_rejects_when_daemon_has_no_backends() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorCatalogResponse { entries: vec![] }),
        )
        .mount(&server)
        .await;
    // Fail loud if the CLI still POSTs after the pre-flight rejection.
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let (stdout, stderr, status) = run_mimir(
        &[
            "connector",
            "add",
            "gmail",
            "--backend",
            "imap",
            "--password",
            "hunter2",
        ],
        &server.uri(),
    );
    assert!(
        !status.success(),
        "add must reject when the daemon has no backends.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("no connector backends registered"),
        "expected an empty-catalog rejection, got: {stderr}"
    );
    assert!(stdout.is_empty(), "expected no stdout, got: {stdout}");

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests.iter().all(|r| r.url.path() != "/connectors"),
        "POST /connectors must not run after a pre-flight rejection"
    );
}

#[test]
fn connector_auth_rejects_both_flags() {
    let (stdout, stderr, status) = run_mimir(
        &[
            "connector",
            "auth",
            "demo",
            "--password",
            "p",
            "--token",
            "t",
        ],
        "http://127.0.0.1:1",
    );
    assert!(
        !status.success(),
        "auth must reject passing both --password and --token"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("cannot be used with '--token <TOKEN>'"),
        "expected the clap both-flags conflict error, got: {combined}"
    );
}
