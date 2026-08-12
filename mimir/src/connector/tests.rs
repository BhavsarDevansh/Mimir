//! Unit + wiremock-backed tests for the connector CLI module.

use mimir_api_types::{
    ActionResultResponse, ConnectorListResponse, ConnectorResponse, ForgetConnectorResponse,
    SyncConnectorResponse,
};
use mimir_client::MimirClient;
use mimir_connectors::SecretBundle;
use mimir_connectors::test_utils::self_callback_opener;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_partial_json, method, path},
};

use super::add::handle_connector_add_with_opener;
use super::auth::handle_connector_auth_with_opener;
use super::oauth::{oauth_flow_config, oauth_ingest_request};
use super::*;

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

async fn mount_list(server: &MockServer, connectors: Vec<ConnectorResponse>) {
    Mock::given(method("GET"))
        .and(path("/connectors"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorListResponse { connectors }),
        )
        .mount(server)
        .await;
}

/// Mount a wiremock token endpoint that answers the PKCE code exchange.
async fn mount_token_endpoint(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "ya29.access",
            "token_type": "Bearer",
            "refresh_token": "rt",
            "expires_in": 3600,
        })))
        .expect(1)
        .mount(server)
        .await;
}

/// The OAuth config fields the PKCE flow needs, as `key=value` pairs.
fn oauth_config_pairs(token_endpoint: &str) -> Vec<String> {
    vec![
        "auth.kind=oauth".to_string(),
        "auth.auth_uri=https://oauth.example.com/authorize".to_string(),
        format!("auth.token_endpoint={token_endpoint}"),
        "auth.client_id=test-client".to_string(),
        "calendar_url=https://dav.example.com/cal".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// parse_duration
// ---------------------------------------------------------------------------

#[test]
fn parse_duration_bare_seconds() {
    assert_eq!(parse_duration("90").unwrap(), 90);
    assert_eq!(parse_duration("0").unwrap(), 0);
}

#[test]
fn parse_duration_units() {
    assert_eq!(parse_duration("30s").unwrap(), 30);
    assert_eq!(parse_duration("5m").unwrap(), 300);
    assert_eq!(parse_duration("12h").unwrap(), 43_200);
    assert_eq!(parse_duration("7d").unwrap(), 604_800);
}

#[test]
fn parse_duration_accepts_case_and_whitespace() {
    assert_eq!(parse_duration(" 7D ").unwrap(), 604_800);
}

#[test]
fn parse_duration_rejects_garbage() {
    assert!(parse_duration("").is_err());
    assert!(parse_duration("s").is_err());
    assert!(parse_duration("7x").is_err());
    assert!(parse_duration("abc").is_err());
    assert!(parse_duration("7d5h").is_err());
    assert!(parse_duration("7д").is_err());
}

#[test]
fn parse_duration_rejects_overflow() {
    assert!(parse_duration("99999999999999999999d").is_err());
}

// ---------------------------------------------------------------------------
// merge_config
// ---------------------------------------------------------------------------

#[test]
fn merge_config_nests_dotted_keys_and_parses_scalars() {
    let merged = merge_config(
        &[
            "auth.kind=app_password".to_string(),
            "auth.username=me@example.com".to_string(),
            "port=993".to_string(),
            "enabled=true".to_string(),
            "host=imap.example.com".to_string(),
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        merged,
        serde_json::json!({
            "auth": {"kind": "app_password", "username": "me@example.com"},
            "port": 993,
            "enabled": true,
            "host": "imap.example.com"
        })
    );
}

#[test]
fn merge_config_config_json_base_with_pair_overrides() {
    let merged = merge_config(
        &["auth.kind=api_token".to_string()],
        Some(r#"{"auth": {"kind": "app_password"}, "host": "imap.example.com"}"#),
    )
    .unwrap();
    assert_eq!(merged["auth"]["kind"], serde_json::json!("api_token"));
    assert_eq!(merged["host"], serde_json::json!("imap.example.com"));
}

#[test]
fn merge_config_rejects_malformed_input() {
    assert!(merge_config(&["no-equals".to_string()], None).is_err());
    assert!(merge_config(&["=value".to_string()], None).is_err());
    assert!(merge_config(&["auth..kind=oauth".to_string()], None).is_err());
    assert!(merge_config(&["auth.=oauth".to_string()], None).is_err());
    assert!(merge_config(&[], Some("not json")).is_err());
    assert!(merge_config(&[], Some(r#"[1, 2]"#)).is_err());
}

#[test]
fn merge_config_quoted_values_stay_strings() {
    let merged = merge_config(
        &[
            "account=\"0755\"".to_string(),
            "version=\"1.0\"".to_string(),
            "flag=\"true\"".to_string(),
            "note=\"hello world\"".to_string(),
        ],
        None,
    )
    .unwrap();
    assert_eq!(merged["account"], serde_json::json!("0755"));
    assert_eq!(merged["version"], serde_json::json!("1.0"));
    assert_eq!(merged["flag"], serde_json::json!("true"));
    assert_eq!(merged["note"], serde_json::json!("hello world"));
}

#[test]
fn parse_config_scalar_honors_double_quotes() {
    assert_eq!(parse_config_scalar("\"0755\""), serde_json::json!("0755"));
    assert_eq!(parse_config_scalar("\"1.0\""), serde_json::json!("1.0"));
    assert_eq!(parse_config_scalar("\"true\""), serde_json::json!("true"));
    assert_eq!(
        parse_config_scalar("\"hello world\""),
        serde_json::json!("hello world")
    );
    assert_eq!(parse_config_scalar("\"\""), serde_json::json!(""));
    assert_eq!(parse_config_scalar("0755"), serde_json::json!(755));
    assert_eq!(parse_config_scalar("true"), serde_json::json!(true));
    assert_eq!(parse_config_scalar("\"0755"), serde_json::json!("\"0755"));
}

#[test]
fn merge_config_dotted_path_overwrites_scalar() {
    let merged = merge_config(&["auth.kind=oauth".to_string()], Some(r#"{"auth": 5}"#)).unwrap();
    assert_eq!(merged["auth"]["kind"], serde_json::json!("oauth"));
}

// ---------------------------------------------------------------------------
// credential_kind_for / title_case / error rendering
// ---------------------------------------------------------------------------

#[test]
fn credential_kind_detected_from_auth_tag() {
    let app_password = serde_json::json!({"auth": {"kind": "app_password"}});
    assert!(matches!(
        credential_kind_for(&app_password),
        CredentialKind::AppPassword
    ));
    let api_token = serde_json::json!({"auth": {"kind": "api_token"}});
    assert!(matches!(
        credential_kind_for(&api_token),
        CredentialKind::ApiToken
    ));
    let oauth = serde_json::json!({"auth": {"kind": "oauth"}});
    assert!(matches!(credential_kind_for(&oauth), CredentialKind::OAuth));
    for config in [
        serde_json::json!({"host": "imap.example.com"}),
        serde_json::Value::Null,
    ] {
        assert!(matches!(credential_kind_for(&config), CredentialKind::None));
    }
}

#[test]
fn title_case_uppercases_first_letter() {
    assert_eq!(title_case("gmail"), "Gmail");
    assert_eq!(title_case("calendar"), "Calendar");
    assert_eq!(title_case("photos"), "Photos");
    assert_eq!(title_case(""), "");
}

#[test]
fn server_error_detail_unwraps_api_error_body() {
    assert_eq!(
        server_error_detail(
            r#"{"error": "connector 1 is not running (status: setup)", "code": "CONNECTOR_NOT_RUNNING"}"#
        ),
        "connector 1 is not running (status: setup)"
    );
    assert_eq!(server_error_detail("plain text"), "plain text");
}

#[test]
fn is_connector_not_running_matches_code() {
    assert!(is_connector_not_running(
        r#"{"error": "x", "code": "CONNECTOR_NOT_RUNNING"}"#
    ));
    assert!(!is_connector_not_running(
        r#"{"error": "x", "code": "CONNECTOR_PUSH_UNSUPPORTED"}"#
    ));
    assert!(!is_connector_not_running("not json"));
}

// ---------------------------------------------------------------------------
// resolve_connector
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_connector_matches_slug_from_list() {
    let server = MockServer::start().await;
    mount_list(
        &server,
        vec![connector_fixture(1, "a"), connector_fixture(2, "b")],
    )
    .await;
    let client = MimirClient::new(server.uri());
    let conn = resolve_connector(&client, "b").await;
    assert_eq!(conn.id, 2);
    assert_eq!(conn.slug, "b");
}

// ---------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_with_app_password_ingests_credentials() {
    let server = MockServer::start().await;
    let created = connector_fixture(1, "gmail");
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .and(body_json(serde_json::json!({
            "connector_type": "gmail",
            "backend": "imap",
            "slug": "gmail",
            "display_name": "Gmail",
            "config_json": {
                "auth": {"kind": "app_password", "username": "me@example.com"},
                "host": "imap.example.com"
            }
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(&created))
        .mount(&server)
        .await;
    let mut authenticated = created;
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/1/tokens"))
        .and(body_json(serde_json::json!({
            "kind": "app_password",
            "password": "hunter2"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .mount(&server)
        .await;

    handle_connector_add(
        "gmail".to_string(),
        "imap".to_string(),
        vec![
            "auth.kind=app_password".to_string(),
            "auth.username=me@example.com".to_string(),
            "host=imap.example.com".to_string(),
        ],
        None,
        None,
        None,
        Some("hunter2".to_string()),
        None,
        true,
        &server.uri(),
    )
    .await;

    // The credential prompt is skipped in non-interactive tests; the flag
    // supplies the secret, so exactly one token ingest must have occurred.
    let token_requests = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/connectors/1/tokens")
        .count();
    assert_eq!(token_requests, 1);
}

#[tokio::test]
async fn add_with_oauth_config_runs_pkce_flow_and_ingests_tokens() {
    let daemon = MockServer::start().await;
    let token_server = MockServer::start().await;
    let created = connector_fixture(1, "calendar");
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .respond_with(ResponseTemplate::new(201).set_body_json(&created))
        .mount(&daemon)
        .await;
    let mut authenticated = created;
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/1/tokens"))
        .and(body_partial_json(serde_json::json!({
            "kind": "oauth",
            "access_token": "ya29.access",
            "refresh_token": "rt",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .mount(&daemon)
        .await;
    mount_token_endpoint(&token_server).await;

    handle_connector_add_with_opener(
        "calendar".to_string(),
        "caldav".to_string(),
        oauth_config_pairs(&format!("{}/token", token_server.uri())),
        None,
        None,
        None,
        None,
        None,
        true,
        &daemon.uri(),
        &self_callback_opener("auth-code"),
    )
    .await;

    // The PKCE flow must have exchanged the code and POSTed the OAuth
    // bundle to the daemon's token route.
    let token_requests = daemon
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().contains("/tokens"))
        .count();
    assert_eq!(token_requests, 1);
}

#[test]
fn oauth_flow_config_extracts_fields() {
    let config = serde_json::json!({
        "auth": {
            "kind": "oauth",
            "auth_uri": "https://oauth.example.com/authorize",
            "token_endpoint": "https://oauth.example.com/token",
            "client_id": "cid",
            "client_secret": "secret",
            "scopes": ["a", "b"],
        }
    });
    let flow = oauth_flow_config(&config).expect("config");
    assert_eq!(flow.auth_uri, "https://oauth.example.com/authorize");
    assert_eq!(flow.token_endpoint, "https://oauth.example.com/token");
    assert_eq!(flow.client_id, "cid");
    assert_eq!(flow.client_secret.as_deref(), Some("secret"));
    assert_eq!(
        flow.scopes.as_deref(),
        Some(&["a".to_string(), "b".to_string()][..])
    );
}

#[test]
fn oauth_flow_config_rejects_missing_fields() {
    let missing_uri = serde_json::json!({
        "auth": {"kind": "oauth", "token_endpoint": "https://oauth.example.com/token", "client_id": "cid"}
    });
    let err = oauth_flow_config(&missing_uri).expect_err("auth_uri is required");
    assert!(err.contains("auth_uri"), "got: {err}");

    let missing_auth = serde_json::json!({"calendar_url": "https://dav.example.com/cal"});
    let err = oauth_flow_config(&missing_auth).expect_err("auth object is required");
    assert!(err.contains("auth"), "got: {err}");
}

#[test]
fn oauth_ingest_request_converts_bundle_to_wire() {
    let bundle = SecretBundle::OAuth {
        access_token: "ya29.access".to_string(),
        refresh_token: Some("rt".to_string()),
        expires_at: Some(chrono::Utc::now()),
    };
    let req = oauth_ingest_request(&bundle);
    match req {
        mimir_api_types::IngestTokenRequest::OAuth {
            access_token,
            refresh_token,
            expires_at,
        } => {
            assert_eq!(access_token, "ya29.access");
            assert_eq!(refresh_token.as_deref(), Some("rt"));
            assert!(
                expires_at
                    .as_ref()
                    .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()),
                "expiry must be RFC-3339, got {expires_at:?}"
            );
        }
        other => panic!("expected OAuth ingest request, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_with_oauth_config_runs_pkce_flow_and_ingests_tokens() {
    let daemon = MockServer::start().await;
    let token_server = MockServer::start().await;
    let conn = connector_fixture(1, "calendar");
    mount_list(&daemon, vec![conn.clone()]).await;
    let mut authenticated = conn;
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/1/tokens"))
        .and(body_partial_json(serde_json::json!({
            "kind": "oauth",
            "access_token": "ya29.access",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .mount(&daemon)
        .await;
    mount_token_endpoint(&token_server).await;

    handle_connector_auth_with_opener(
        "calendar".to_string(),
        oauth_config_pairs(&format!("{}/token", token_server.uri())),
        None,
        None,
        None,
        true,
        &daemon.uri(),
        &self_callback_opener("auth-code"),
    )
    .await;

    let token_requests = daemon
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/connectors/1/tokens")
        .count();
    assert_eq!(token_requests, 1);
}

#[tokio::test]
async fn auth_with_password_flag_ingests_app_password() {
    let server = MockServer::start().await;
    let conn = connector_fixture(1, "gmail");
    mount_list(&server, vec![conn.clone()]).await;
    let mut authenticated = conn;
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/1/tokens"))
        .and(body_json(serde_json::json!({
            "kind": "app_password",
            "password": "hunter2"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .mount(&server)
        .await;

    handle_connector_auth_with_opener(
        "gmail".to_string(),
        vec![],
        None,
        Some("hunter2".to_string()),
        None,
        true,
        &server.uri(),
        &|_url: &str| {},
    )
    .await;

    let token_requests = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/connectors/1/tokens")
        .count();
    assert_eq!(token_requests, 1);
}

// ---------------------------------------------------------------------------
// sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_sends_human_duration_as_seconds() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    Mock::given(method("POST"))
        .and(path("/connectors/7/sync"))
        .and(body_json(
            serde_json::json!({"full": false, "since": 604_800}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&SyncConnectorResponse::Ok {
                fetched: 3,
                new_cursor: Some("c1".to_string()),
            }),
        )
        .expect(1)
        .mount(&server)
        .await;

    handle_connector_sync(
        "demo".to_string(),
        false,
        Some("7d".to_string()),
        true,
        &server.uri(),
    )
    .await;
}

#[tokio::test]
async fn sync_full_omits_since() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    Mock::given(method("POST"))
        .and(path("/connectors/7/sync"))
        .and(body_json(serde_json::json!({"full": true})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&SyncConnectorResponse::Ok {
                fetched: 0,
                new_cursor: None,
            }),
        )
        .expect(1)
        .mount(&server)
        .await;

    handle_connector_sync("demo".to_string(), true, None, true, &server.uri()).await;
}

// ---------------------------------------------------------------------------
// lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_and_resume_round_trip() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    let mut paused = connector_fixture(7, "demo");
    paused.status = "paused".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/7/pause"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&paused))
        .expect(1)
        .mount(&server)
        .await;
    let mut active = connector_fixture(7, "demo");
    active.status = "active".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/7/resume"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&active))
        .expect(1)
        .mount(&server)
        .await;

    handle_connector_pause("demo".to_string(), true, &server.uri()).await;
    handle_connector_resume("demo".to_string(), true, &server.uri()).await;
}

#[tokio::test]
async fn remove_deletes_instance() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    Mock::given(method("DELETE"))
        .and(path("/connectors/7"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    handle_connector_remove("demo".to_string(), true, &server.uri()).await;
}

#[tokio::test]
async fn forget_trashes_facts_and_reports_count() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    Mock::given(method("POST"))
        .and(path("/connectors/7/forget"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ForgetConnectorResponse {
                forgotten_count: 12,
            }),
        )
        .expect(1)
        .mount(&server)
        .await;

    handle_connector_forget("demo".to_string(), true, true, &server.uri()).await;
}

// ---------------------------------------------------------------------------
// act
// ---------------------------------------------------------------------------

#[tokio::test]
async fn act_dispatches_inline_payload() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    Mock::given(method("POST"))
        .and(path("/connectors/7/actions"))
        .and(body_json(serde_json::json!({
            "kind": "create_event",
            "payload": {"summary": "Lunch", "start": "2026-08-12T12:00:00Z"}
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ActionResultResponse {
                success: true,
                native_id: Some("/cal/lunch.ics".to_string()),
                message: Some("etag-1".to_string()),
            }),
        )
        .expect(1)
        .mount(&server)
        .await;

    handle_connector_act(
        "demo".to_string(),
        "create_event".to_string(),
        Some(r#"{"summary": "Lunch", "start": "2026-08-12T12:00:00Z"}"#.to_string()),
        None,
        true,
        &server.uri(),
    )
    .await;
}

#[tokio::test]
async fn act_reads_payload_from_json_file() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    Mock::given(method("POST"))
        .and(path("/connectors/7/actions"))
        .and(body_json(serde_json::json!({
            "kind": "delete_event",
            "payload": {"href": "/cal/old.ics"}
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ActionResultResponse {
                success: true,
                native_id: None,
                message: None,
            }),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("payload.json");
    std::fs::write(&path, r#"{"href": "/cal/old.ics"}"#).unwrap();

    handle_connector_act(
        "demo".to_string(),
        "delete_event".to_string(),
        None,
        Some(path),
        true,
        &server.uri(),
    )
    .await;
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_with_token_ingests_api_token() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    let mut authenticated = connector_fixture(7, "demo");
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/7/tokens"))
        .and(body_json(
            serde_json::json!({"kind": "api_token", "token": "tok-123"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .mount(&server)
        .await;

    handle_connector_auth(
        "demo".to_string(),
        vec![],
        None,
        None,
        Some("tok-123".to_string()),
        true,
        &server.uri(),
    )
    .await;

    let token_requests = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/connectors/7/tokens")
        .count();
    assert_eq!(token_requests, 1);
}

#[tokio::test]
async fn auth_with_password_ingests_app_password() {
    let server = MockServer::start().await;
    mount_list(&server, vec![connector_fixture(7, "demo")]).await;
    let mut authenticated = connector_fixture(7, "demo");
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/7/tokens"))
        .and(body_json(serde_json::json!({
            "kind": "app_password",
            "password": "hunter2"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .expect(1)
        .mount(&server)
        .await;

    handle_connector_auth(
        "demo".to_string(),
        vec![],
        None,
        Some("hunter2".to_string()),
        None,
        true,
        &server.uri(),
    )
    .await;
}
