//! Binary-level tests for the `mimir connector` CLI against a wiremock
//! daemon (full clap → daemon-guard → HTTP-client path).

use std::process::{Command, Stdio};

use mimir_api_types::{ConnectorListResponse, ConnectorResponse};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn run_mimir(args: &[&str], base_url: &str) -> (String, String, std::process::ExitStatus) {
    let output = Command::new(env!("CARGO_BIN_EXE_mimir"))
        .args(args)
        .env("NO_COLOR", "1")
        .env("MIMIR_BASE_URL", base_url)
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
