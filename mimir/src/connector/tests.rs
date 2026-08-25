//! Unit + wiremock-backed tests for the connector CLI module.

use mimir_api_types::{
    ActionResultResponse, ConnectorAuthConfig, ConnectorCatalogEntry, ConnectorCatalogResponse,
    ConnectorListResponse, ConnectorResponse, ForgetConnectorResponse, SyncConnectorResponse,
};
use mimir_client::MimirClient;
use mimir_connectors::SecretBundle;
use mimir_connectors::test_utils::{mount_token_endpoint, self_callback_opener};

use crate::transport::DaemonTransport;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_partial_json, method, path},
};

use super::add::handle_connector_add_with_opener;
use super::auth::handle_connector_auth_with_opener;
use super::oauth::{oauth_flow_config, oauth_flow_config_with_secret, oauth_ingest_request};
use super::wizard::{
    PromptDriver, build_wizard_config, handle_connector_add_wizard_with_deps, parse_scopes,
    password_prompt, slugify,
};
use super::*;

/// Scripted [`PromptDriver`] for wizard tests: each prompt consumes the next
/// canned answer, so the whole interactive flow runs without a TTY. Every
/// prompt message is recorded too, so tests can pin guidance text (issue
/// #467 pins the Microsoft account-type and client-ID help this way).
struct ScriptedPrompt {
    answers: std::cell::RefCell<Vec<ScriptedAnswer>>,
    messages: std::cell::RefCell<Vec<String>>,
}

#[derive(Debug)]
enum ScriptedAnswer {
    Select(usize),
    Input(String),
    Password(String),
}

impl ScriptedPrompt {
    fn new(answers: Vec<ScriptedAnswer>) -> Self {
        Self {
            answers: std::cell::RefCell::new(answers),
            messages: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// The prompt messages asked so far, in order.
    fn messages(&self) -> Vec<String> {
        self.messages.borrow().clone()
    }

    fn record(&self, message: &str) {
        self.messages.borrow_mut().push(message.to_string());
    }

    fn take(&self) -> ScriptedAnswer {
        self.answers
            .borrow_mut()
            .drain(..1)
            .next()
            .unwrap_or_else(|| panic!("scripted prompt ran out of answers"))
    }
}

impl PromptDriver for ScriptedPrompt {
    fn select(&self, message: &str, _options: &[String]) -> Result<usize, String> {
        self.record(message);
        match self.take() {
            ScriptedAnswer::Select(index) => Ok(index),
            other => panic!("expected a Select answer, got {other:?}"),
        }
    }

    fn input(&self, message: &str, default: Option<&str>) -> Result<String, String> {
        self.record(message);
        match self.take() {
            ScriptedAnswer::Input(value) => {
                if value.is_empty() {
                    // Mirrors `inquire`: an empty answer accepts the default.
                    match default {
                        Some(default) => Ok(default.to_string()),
                        None => Ok(value),
                    }
                } else {
                    Ok(value)
                }
            }
            other => panic!("expected an Input answer, got {other:?}"),
        }
    }

    fn password(&self, message: &str) -> Result<String, String> {
        self.record(message);
        match self.take() {
            ScriptedAnswer::Password(value) => Ok(value),
            other => panic!("expected a Password answer, got {other:?}"),
        }
    }
}

fn catalog(entries: &[(&str, &str)]) -> mimir_api_types::ConnectorCatalogResponse {
    mimir_api_types::ConnectorCatalogResponse {
        entries: entries
            .iter()
            .map(
                |(connector_type, backend)| mimir_api_types::ConnectorCatalogEntry {
                    connector_type: connector_type.to_string(),
                    backend: backend.to_string(),
                },
            )
            .collect(),
    }
}

/// The daemon POST /connectors mock: returns the created row (id 1, setup).
async fn mount_add(server: &MockServer, created: &ConnectorResponse) {
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .mount(server)
        .await;
}

fn connector_fixture(id: i32, slug: &str) -> ConnectorResponse {
    ConnectorResponse {
        id,
        connector_type: "email".to_string(),
        slug: slug.to_string(),
        backend: "test".to_string(),
        display_name: slug.to_string(),
        status: "setup".to_string(),
        auth_state: "unauthenticated".to_string(),
        mode: Some("push".to_string()),
        sync_cursor: None,
        last_sync_at: None,
        last_error: None,
        created_at: "2026-08-11T00:00:00Z".to_string(),
        updated_at: "2026-08-11T00:00:00Z".to_string(),
        item_count: 0,
        auth: None,
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
    assert_eq!(
        parse_config_scalar("\"[1, 2, 3]\""),
        serde_json::json!("[1, 2, 3]")
    );
    assert_eq!(
        parse_config_scalar("\"{not json}\""),
        serde_json::json!("{not json}")
    );
}

#[test]
fn parse_config_scalar_parses_json_arrays_and_objects() {
    assert_eq!(
        parse_config_scalar(
            r#"["https://mail.google.com/", "https://www.googleapis.com/auth/calendar.readonly"]"#
        ),
        serde_json::json!([
            "https://mail.google.com/",
            "https://www.googleapis.com/auth/calendar.readonly"
        ])
    );
    assert_eq!(
        parse_config_scalar("[1, 2, 3]"),
        serde_json::json!([1, 2, 3])
    );
    assert_eq!(parse_config_scalar("[]"), serde_json::json!([]));
    assert_eq!(
        parse_config_scalar(r#"{"interval_seconds": 900}"#),
        serde_json::json!({"interval_seconds": 900})
    );
    assert_eq!(parse_config_scalar("{}"), serde_json::json!({}));
}

#[test]
fn parse_config_scalar_falls_back_to_string_for_malformed_json() {
    assert_eq!(
        parse_config_scalar("[unterminated"),
        serde_json::json!("[unterminated")
    );
    assert_eq!(
        parse_config_scalar("{not json}"),
        serde_json::json!("{not json}")
    );
    assert_eq!(parse_config_scalar("["), serde_json::json!("["));
}

#[test]
fn merge_config_json_arrays_reach_merged_config() {
    let merged = merge_config(
        &[
            "auth.kind=oauth".to_string(),
            r#"auth.scopes=["https://mail.google.com/", "https://www.googleapis.com/auth/calendar.readonly"]"#.to_string(),
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        merged["auth"]["scopes"],
        serde_json::json!([
            "https://mail.google.com/",
            "https://www.googleapis.com/auth/calendar.readonly"
        ])
    );
}

#[test]
fn merge_config_json_objects_reach_merged_config() {
    let merged = merge_config(&["limits={\"max_items\": 50}".to_string()], None).unwrap();
    assert_eq!(merged["limits"], serde_json::json!({"max_items": 50}));
}

#[test]
fn oauth_flow_config_scopes_from_key_value_pairs() {
    let merged = merge_config(
        &[
            "auth.kind=oauth".to_string(),
            "auth.auth_uri=https://oauth.example.com/authorize".to_string(),
            "auth.token_endpoint=https://oauth.example.com/token".to_string(),
            "auth.client_id=test-client".to_string(),
            r#"auth.scopes=["scope-a", "scope-b"]"#.to_string(),
        ],
        None,
    )
    .unwrap();
    let flow = oauth_flow_config(&merged).expect("merged config should drive the flow");
    assert_eq!(
        flow.scopes.as_deref(),
        Some(&["scope-a".to_string(), "scope-b".to_string()][..])
    );
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
    assert_eq!(title_case("email"), "Email");
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
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorCatalogResponse {
                entries: vec![ConnectorCatalogEntry {
                    connector_type: "email".to_string(),
                    backend: "imap".to_string(),
                }],
            }),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/connectors"))
        .and(body_json(serde_json::json!({
            "connector_type": "email",
            "backend": "imap",
            "slug": "email",
            "display_name": "Email",
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
        false,
        None,
        false,
        true,
        &DaemonTransport::Tcp(server.uri()),
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
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(&ConnectorCatalogResponse {
                entries: vec![ConnectorCatalogEntry {
                    connector_type: "calendar".to_string(),
                    backend: "caldav".to_string(),
                }],
            }),
        )
        .mount(&daemon)
        .await;
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
    mount_token_endpoint(&token_server, 1).await;

    handle_connector_add_with_opener(
        "calendar".to_string(),
        "caldav".to_string(),
        oauth_config_pairs(&format!("{}/token", token_server.uri())),
        None,
        None,
        None,
        None,
        false,
        None,
        false,
        true,
        &DaemonTransport::Tcp(daemon.uri()),
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
fn oauth_flow_config_with_secret_overrides_config_value() {
    let config = serde_json::json!({
        "auth": {
            "kind": "oauth",
            "auth_uri": "https://oauth.example.com/authorize",
            "token_endpoint": "https://oauth.example.com/token",
            "client_id": "cid",
            "client_secret": "config-secret",
        }
    });
    let flow = oauth_flow_config_with_secret(&config, Some("wizard-secret")).expect("config");
    assert_eq!(flow.client_secret.as_deref(), Some("wizard-secret"));
    let flow = oauth_flow_config_with_secret(&config, None).expect("config");
    assert_eq!(flow.client_secret.as_deref(), Some("config-secret"));
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
        client_secret: Some("s3cret".to_string()),
    };
    let req = oauth_ingest_request(&bundle);
    match req {
        mimir_api_types::IngestTokenRequest::OAuth {
            access_token,
            refresh_token,
            expires_at,
            client_secret,
        } => {
            assert_eq!(access_token, "ya29.access");
            assert_eq!(refresh_token.as_deref(), Some("rt"));
            assert_eq!(client_secret.as_deref(), Some("s3cret"));
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
    mount_token_endpoint(&token_server, 1).await;

    handle_connector_auth_with_opener(
        "calendar".to_string(),
        oauth_config_pairs(&format!("{}/token", token_server.uri())),
        None,
        None,
        false,
        None,
        false,
        true,
        &DaemonTransport::Tcp(daemon.uri()),
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
        false,
        None,
        false,
        true,
        &DaemonTransport::Tcp(server.uri()),
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

#[tokio::test]
async fn auth_with_stored_oauth_config_runs_pkce_flow_without_resupplied_config() {
    // Issue #507: the daemon surfaces the stored non-secret auth config, so
    // `mimir connector auth <slug>` re-runs the PKCE flow from it — no
    // `auth.kind=oauth auth.auth_uri=...` re-supply needed.
    let daemon = MockServer::start().await;
    let token_server = MockServer::start().await;
    let mut conn = connector_fixture(1, "calendar");
    conn.auth = Some(ConnectorAuthConfig {
        kind: "oauth".to_string(),
        username: Some("devansh@example.com".to_string()),
        auth_uri: Some("https://oauth.example.com/authorize".to_string()),
        token_endpoint: Some(format!("{}/token", token_server.uri())),
        client_id: Some("test-client".to_string()),
        scopes: Some(vec!["read".to_string()]),
    });
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
    mount_token_endpoint(&token_server, 1).await;

    handle_connector_auth_with_opener(
        "calendar".to_string(),
        vec![],
        None,
        None,
        false,
        None,
        false,
        true,
        &DaemonTransport::Tcp(daemon.uri()),
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
        &DaemonTransport::Tcp(server.uri()),
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

    handle_connector_sync(
        "demo".to_string(),
        true,
        None,
        true,
        &DaemonTransport::Tcp(server.uri()),
    )
    .await;
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

    handle_connector_pause(
        "demo".to_string(),
        true,
        &DaemonTransport::Tcp(server.uri()),
    )
    .await;
    handle_connector_resume(
        "demo".to_string(),
        true,
        &DaemonTransport::Tcp(server.uri()),
    )
    .await;
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

    handle_connector_remove(
        "demo".to_string(),
        true,
        &DaemonTransport::Tcp(server.uri()),
    )
    .await;
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

    handle_connector_forget(
        "demo".to_string(),
        true,
        true,
        &DaemonTransport::Tcp(server.uri()),
    )
    .await;
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
        &DaemonTransport::Tcp(server.uri()),
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
        &DaemonTransport::Tcp(server.uri()),
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
        false,
        Some("tok-123".to_string()),
        false,
        true,
        &DaemonTransport::Tcp(server.uri()),
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
        false,
        None,
        false,
        true,
        &DaemonTransport::Tcp(server.uri()),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Interactive add wizard (issue: `mimir connector add` with no arguments)
// ---------------------------------------------------------------------------

#[test]
fn slugify_defaults_slug_from_display_name() {
    assert_eq!(slugify("Work Gmail"), "work-gmail");
    assert_eq!(slugify("Gmail — Personal!"), "gmail-personal");
    assert_eq!(slugify("café"), "caf");
    assert_eq!(slugify("Photos"), "photos");
    assert_eq!(slugify(""), "");
}

#[test]
fn wizard_password_prompt_disables_confirmation() {
    // Regression (issue #399): inquire 0.9.4 enables password confirmation
    // by default, so the wizard's hidden secret prompts asked twice — the
    // second masked "Confirmation:" input looked like a hang right before
    // the OAuth browser opened. Secrets are pasted, so each secret must be
    // prompted exactly once.
    let prompt = password_prompt("OAuth client secret (blank if none)");
    assert!(
        !prompt.enable_confirmation,
        "wizard secrets must not ask for a confirmation input"
    );
}

#[test]
fn wizard_email_gmail_oauth_config_uses_google_defaults() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),                         // Email provider → Gmail
        ScriptedAnswer::Input(String::new()),              // host → default
        ScriptedAnswer::Input(String::new()),              // port → default
        ScriptedAnswer::Input(String::new()),              // mailbox → default
        ScriptedAnswer::Input("me@gmail.com".to_string()), // account email
        ScriptedAnswer::Select(0),                         // Sync mode → push (recommended)
        ScriptedAnswer::Select(0),                         // Existing mailbox content — import
        ScriptedAnswer::Select(0),                         // OAuth browser login
        ScriptedAnswer::Input(String::new()),              // auth_uri → Google default
        ScriptedAnswer::Input(String::new()),              // token endpoint → Google default
        ScriptedAnswer::Input("client-123".to_string()),   // client id
        ScriptedAnswer::Password(String::new()),           // client secret → none
        ScriptedAnswer::Input(String::new()),              // scopes → default Gmail
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-gmail", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(config["host"], "imap.gmail.com");
    assert_eq!(config["port"], 993);
    assert_eq!(config["mailbox"], "INBOX");
    assert_eq!(
        config["mode"], "auto",
        "push maps to auto (IDLE when advertised)"
    );
    assert_eq!(config["initial_backfill"], true);
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(config["auth"]["username"], "me@gmail.com");
    assert_eq!(
        config["auth"]["auth_uri"],
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(
        config["auth"]["token_endpoint"],
        "https://oauth2.googleapis.com/token"
    );
    assert_eq!(config["auth"]["client_id"], "client-123");
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!(["https://mail.google.com/"])
    );
    assert!(
        config["auth"].get("client_secret").is_none(),
        "blank client secret must be omitted"
    );
}

#[test]
fn wizard_email_gmail_oauth_client_secret_stays_out_of_config_json() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),                         // Email provider → Gmail
        ScriptedAnswer::Input(String::new()),              // host → default
        ScriptedAnswer::Input(String::new()),              // port → default
        ScriptedAnswer::Input(String::new()),              // mailbox → default
        ScriptedAnswer::Input("me@gmail.com".to_string()), // account email
        ScriptedAnswer::Select(0),                         // Sync mode — push (recommended)
        ScriptedAnswer::Select(0),                         // Existing mailbox content — import
        ScriptedAnswer::Select(0),                         // OAuth browser login
        ScriptedAnswer::Input(String::new()),              // auth_uri → Google default
        ScriptedAnswer::Input(String::new()),              // token endpoint → Google default
        ScriptedAnswer::Input("client-123".to_string()),   // client id
        ScriptedAnswer::Password("s3cret".to_string()),    // client secret (hidden)
        ScriptedAnswer::Input(String::new()),              // scopes → default Gmail
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-gmail", &prompts).unwrap();
    assert!(
        config["auth"].get("client_secret").is_none(),
        "client secret must never be written into config_json"
    );
    match credential {
        super::wizard::WizardCredential::OAuth { client_secret } => {
            assert_eq!(client_secret.as_deref(), Some("s3cret"));
        }
        other => panic!("expected OAuth credential, got {other:?}"),
    }
}

#[test]
fn wizard_email_gmail_polling_custom_interval_and_new_only_maps_to_config() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),                         // Email provider → Gmail
        ScriptedAnswer::Input(String::new()),              // host → default
        ScriptedAnswer::Input(String::new()),              // port → default
        ScriptedAnswer::Input(String::new()),              // mailbox → default
        ScriptedAnswer::Input("me@gmail.com".to_string()), // account email
        ScriptedAnswer::Select(1),                         // Sync mode — every N minutes
        ScriptedAnswer::Select(4),                         // Custom interval
        ScriptedAnswer::Input("7".to_string()),            // 7 minutes
        ScriptedAnswer::Select(1),                         // Existing mailbox content — new only
        ScriptedAnswer::Select(1),                         // App password
        ScriptedAnswer::Password("abcd efgh ijkl mnop".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-gmail", &prompts).unwrap();
    assert_eq!(config["mode"], "poll");
    assert_eq!(config["poll_interval_secs"], 420);
    assert_eq!(config["initial_backfill"], false);
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::Secret(_)
    ));
}

#[test]
fn wizard_email_gmail_polling_preset_interval_uses_preset() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),                         // Email provider → Gmail
        ScriptedAnswer::Input(String::new()),              // host
        ScriptedAnswer::Input(String::new()),              // port
        ScriptedAnswer::Input(String::new()),              // mailbox
        ScriptedAnswer::Input("me@gmail.com".to_string()), // account email
        ScriptedAnswer::Select(1),                         // Sync mode — every N minutes
        ScriptedAnswer::Select(2),                         // 30 minutes
        ScriptedAnswer::Select(0),                         // Existing mailbox content — import
        ScriptedAnswer::Select(1),                         // App password
        ScriptedAnswer::Password("abcd efgh ijkl mnop".to_string()),
    ]);
    let (config, _) = build_wizard_config(&entry, "personal-gmail", &prompts).unwrap();
    assert_eq!(config["mode"], "poll");
    assert_eq!(config["poll_interval_secs"], 30 * 60);
    assert_eq!(config["initial_backfill"], true);
}

#[test]
fn wizard_email_gmail_custom_interval_rejects_zero_before_registration() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),                         // Email provider → Gmail
        ScriptedAnswer::Input(String::new()),              // host
        ScriptedAnswer::Input(String::new()),              // port
        ScriptedAnswer::Input(String::new()),              // mailbox
        ScriptedAnswer::Input("me@gmail.com".to_string()), // account email
        ScriptedAnswer::Select(1),                         // Sync mode — every N minutes
        ScriptedAnswer::Select(4),                         // Custom interval
        ScriptedAnswer::Input("0".to_string()),            // invalid: zero minutes
    ]);
    let err = build_wizard_config(&entry, "personal-gmail", &prompts).unwrap_err();
    assert!(
        err.contains("at least 1 minute"),
        "zero must be rejected with a clear message: {err}"
    );
}

#[test]
fn wizard_email_gmail_custom_interval_rejects_overflowing_value() {
    // A user-typed interval must never overflow `u64` on the secs
    // conversion (`minutes * 60`), even for an absurd input.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),                         // Email provider → Gmail
        ScriptedAnswer::Input(String::new()),              // host
        ScriptedAnswer::Input(String::new()),              // port
        ScriptedAnswer::Input(String::new()),              // mailbox
        ScriptedAnswer::Input("me@gmail.com".to_string()), // account email
        ScriptedAnswer::Select(1),                         // Sync mode — every N minutes
        ScriptedAnswer::Select(4),                         // Custom interval
        ScriptedAnswer::Input(u64::MAX.to_string()),       // absurd: overflows * 60
    ]);
    let err = build_wizard_config(&entry, "personal-gmail", &prompts).unwrap_err();
    assert!(
        err.contains("too large"),
        "an overflowing interval must be rejected with a clear message: {err}"
    );
}

fn email_catalog_entry() -> mimir_api_types::ConnectorCatalogEntry {
    mimir_api_types::ConnectorCatalogEntry {
        connector_type: "email".to_string(),
        backend: "imap".to_string(),
    }
}

#[test]
fn wizard_sync_summary_describes_push_and_polling_modes() {
    let push = serde_json::json!({ "mode": "auto", "initial_backfill": true });
    assert_eq!(
        super::wizard::wizard_sync_summary(&push).as_deref(),
        Some(
            "push — importing existing mailbox content, then listening for new mail via IMAP IDLE"
        )
    );
    let push_new_only = serde_json::json!({ "mode": "auto", "initial_backfill": false });
    assert_eq!(
        super::wizard::wizard_sync_summary(&push_new_only).as_deref(),
        Some("push — listening for new mail via IMAP IDLE (existing mailbox content skipped)")
    );
    let poll = serde_json::json!({ "mode": "poll", "poll_interval_secs": 900 });
    assert_eq!(
        super::wizard::wizard_sync_summary(&poll).as_deref(),
        Some("polling every 15 minutes")
    );
    let calendar = serde_json::json!({ "calendar_url": "https://dav.example.com/cal" });
    assert_eq!(super::wizard::wizard_sync_summary(&calendar), None);
}

#[tokio::test]
async fn wizard_gmail_app_password_registers_end_to_end() {
    let daemon = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog(&[
            ("email", "imap"),
            ("calendar", "caldav"),
            ("photos", "local"),
        ])))
        .mount(&daemon)
        .await;
    let created = connector_fixture(1, "personal-gmail");
    mount_add(&daemon, &created).await;
    let mut authenticated = created;
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/1/tokens"))
        .and(body_json(serde_json::json!({
            "kind": "app_password",
            "password": "abcd efgh ijkl mnop"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .expect(1)
        .mount(&daemon)
        .await;
    // Issue #397: the wizard auto-activates the connector after credential
    // ingest, so the daemon receives a `resume` without any user action.
    let mut active = authenticated;
    active.status = "active".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/1/resume"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&active))
        .expect(1)
        .mount(&daemon)
        .await;

    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0), // Email (imap)
        ScriptedAnswer::Input("Personal Gmail".to_string()),
        ScriptedAnswer::Input(String::new()), // slug → personal-gmail
        ScriptedAnswer::Select(0),            // Email provider → Gmail
        ScriptedAnswer::Input(String::new()), // host → default
        ScriptedAnswer::Input(String::new()), // port → default
        ScriptedAnswer::Input(String::new()), // mailbox → default
        ScriptedAnswer::Input("me@gmail.com".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push (recommended)
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Select(1), // App password
        ScriptedAnswer::Password("abcd efgh ijkl mnop".to_string()),
    ]);
    handle_connector_add_wizard_with_deps(
        false,
        &DaemonTransport::Tcp(daemon.uri()),
        &prompts,
        &|_| {},
    )
    .await;

    let requests = daemon.received_requests().await.unwrap();
    let add = requests
        .iter()
        .find(|r| r.url.path() == "/connectors")
        .expect("connector add POST");
    let body: serde_json::Value = serde_json::from_slice(&add.body).unwrap();
    assert_eq!(body["slug"], "personal-gmail");
    assert_eq!(body["display_name"], "Personal Gmail");
    assert_eq!(body["config_json"]["auth"]["username"], "me@gmail.com");
    assert_eq!(body["config_json"]["mode"], "auto");
    assert_eq!(body["config_json"]["initial_backfill"], true);
    let resumes = requests
        .iter()
        .filter(|r| r.url.path() == "/connectors/1/resume")
        .count();
    assert_eq!(
        resumes, 1,
        "the wizard must auto-activate the connector after credential ingest"
    );
}

#[tokio::test]
async fn wizard_oauth_with_client_secret_keeps_it_out_of_config_json() {
    let daemon = MockServer::start().await;
    let token_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog(&[
            ("email", "imap"),
            ("calendar", "caldav"),
            ("photos", "local"),
        ])))
        .mount(&daemon)
        .await;
    let created = connector_fixture(1, "personal-gmail");
    mount_add(&daemon, &created).await;
    let mut authenticated = created;
    authenticated.auth_state = "authenticated".to_string();
    Mock::given(method("POST"))
        .and(path("/connectors/1/tokens"))
        .and(body_partial_json(serde_json::json!({
            "kind": "oauth",
            "access_token": "ya29.access",
            "refresh_token": "rt",
            "client_secret": "s3cret",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&authenticated))
        .expect(1)
        .mount(&daemon)
        .await;
    mount_token_endpoint(&token_server, 1).await;

    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(1), // Calendar (caldav)
        ScriptedAnswer::Input("Personal Gmail".to_string()),
        ScriptedAnswer::Input(String::new()), // slug → personal-gmail
        ScriptedAnswer::Select(3),            // Calendar provider → Custom CalDAV
        ScriptedAnswer::Input("https://dav.example.com/cal".to_string()),
        ScriptedAnswer::Input("me@gmail.com".to_string()),
        ScriptedAnswer::Select(1), // OAuth 2.0 — browser login
        ScriptedAnswer::Input("https://accounts.example.com/auth".to_string()),
        ScriptedAnswer::Input(format!("{}/token", token_server.uri())),
        ScriptedAnswer::Input("client-456".to_string()),
        ScriptedAnswer::Password("s3cret".to_string()), // client secret, hidden
        ScriptedAnswer::Input(String::new()),           // scopes → default
    ]);
    handle_connector_add_wizard_with_deps(
        false,
        &DaemonTransport::Tcp(daemon.uri()),
        &prompts,
        &self_callback_opener("auth-code"),
    )
    .await;

    let requests = daemon.received_requests().await.unwrap();
    let add = requests
        .iter()
        .find(|r| r.url.path() == "/connectors")
        .expect("connector add POST");
    let body: serde_json::Value = serde_json::from_slice(&add.body).unwrap();
    assert!(
        body["config_json"]["auth"].get("client_secret").is_none(),
        "client secret must not be stored in config_json"
    );
    let token_requests = requests
        .iter()
        .filter(|r| r.url.path() == "/connectors/1/tokens")
        .count();
    assert_eq!(token_requests, 1, "OAuth bundle must be ingested once");
}

#[tokio::test]
async fn wizard_photos_creates_without_credentials() {
    let daemon = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/connectors/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(catalog(&[("photos", "local")])))
        .mount(&daemon)
        .await;
    let created = connector_fixture(1, "photos");
    mount_add(&daemon, &created).await;

    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0), // Photos (local)
        ScriptedAnswer::Input("Photos".to_string()),
        ScriptedAnswer::Input(String::new()), // slug → photos
        ScriptedAnswer::Input("/tmp/photos".to_string()), // watch dir
        ScriptedAnswer::Input(String::new()), // owner → slug
    ]);
    handle_connector_add_wizard_with_deps(
        false,
        &DaemonTransport::Tcp(daemon.uri()),
        &prompts,
        &|_| {},
    )
    .await;

    let requests = daemon.received_requests().await.unwrap();
    let add = requests
        .iter()
        .find(|r| r.url.path() == "/connectors")
        .expect("connector add POST");
    let body: serde_json::Value = serde_json::from_slice(&add.body).unwrap();
    assert_eq!(body["slug"], "photos");
    assert_eq!(body["config_json"]["watch_dir"], "/tmp/photos");
    let token_requests = requests
        .iter()
        .filter(|r| r.url.path().contains("/tokens"))
        .count();
    assert_eq!(token_requests, 0, "photos needs no credential");
}

#[test]
fn wizard_unknown_backend_errors_with_flag_form_hint() {
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "email".to_string(),
        backend: "test".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![]);
    let err = build_wizard_config(&entry, "email", &prompts).unwrap_err();
    assert!(
        err.contains("no interactive profile for 'email/test'"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains("flag form"),
        "should point at the flag form: {err}"
    );
}

#[test]
fn wizard_required_field_error_mentions_field() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),            // Email provider → Gmail
        ScriptedAnswer::Input(String::new()), // host → default
        ScriptedAnswer::Input(String::new()), // port → default
        ScriptedAnswer::Input(String::new()), // mailbox → default
        ScriptedAnswer::Input(String::new()), // account email → empty
    ]);
    let err = build_wizard_config(&entry, "gmail", &prompts).unwrap_err();
    assert_eq!(err, "Account email is required");
}

#[test]
fn wizard_email_gmail_empty_app_password_is_rejected() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0),            // Email provider → Gmail
        ScriptedAnswer::Input(String::new()), // host → default
        ScriptedAnswer::Input(String::new()), // port → default
        ScriptedAnswer::Input(String::new()), // mailbox → default
        ScriptedAnswer::Input("me@gmail.com".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push (recommended)
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Select(1), // App password
        ScriptedAnswer::Password(String::new()), // empty app password
    ]);
    let err = build_wizard_config(&entry, "gmail", &prompts).unwrap_err();
    assert_eq!(err, "App password is required");
}

#[test]
fn wizard_email_outlook_preset_is_oauth_only() {
    // Microsoft retired basic-auth app passwords for Outlook.com and
    // Exchange Online IMAP, so the preset must go straight to the OAuth
    // prompts — there is no Authentication select and no app-password path.
    // The account-type question (issue #467) is asked right after the
    // provider pick; here the third choice keeps the `/common/` endpoints.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(1),            // Outlook / Office 365
        ScriptedAnswer::Select(2),            // Account type — any (common)
        ScriptedAnswer::Input(String::new()), // host → default
        ScriptedAnswer::Input(String::new()), // port → default
        ScriptedAnswer::Input(String::new()), // mailbox → default
        ScriptedAnswer::Input("me@outlook.com".to_string()),
        ScriptedAnswer::Select(0),            // Sync mode — push
        ScriptedAnswer::Select(0),            // Existing mailbox content — import
        ScriptedAnswer::Input(String::new()), // auth_uri → default
        ScriptedAnswer::Input(String::new()), // token endpoint → default
        ScriptedAnswer::Input("client-ms-123".to_string()),
        ScriptedAnswer::Password(String::new()), // client secret → none
        ScriptedAnswer::Input(String::new()),    // scopes → default
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-outlook", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(config["host"], "outlook.office365.com");
    assert_eq!(config["port"], 993);
    assert_eq!(config["mailbox"], "INBOX");
    assert_eq!(config["mode"], "auto");
    assert_eq!(config["initial_backfill"], true);
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(config["auth"]["username"], "me@outlook.com");
    assert_eq!(
        config["auth"]["auth_uri"],
        "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
    );
    assert_eq!(
        config["auth"]["token_endpoint"],
        "https://login.microsoftonline.com/common/oauth2/v2.0/token"
    );
    assert_eq!(config["auth"]["client_id"], "client-ms-123");
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!([
            "https://outlook.office.com/IMAP.AccessAsUser.All",
            "offline_access"
        ])
    );
}

#[test]
fn wizard_email_outlook_personal_account_preselects_consumers_endpoints() {
    // Issue #467: a personal-only app registration (Supported account
    // types = "Personal Microsoft accounts only") fails the `/common/`
    // authorize request, so the preset must pre-fill `/consumers/` when the
    // user connects a personal Outlook.com / Hotmail account.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(1),            // Outlook / Office 365
        ScriptedAnswer::Select(0),            // Microsoft account type — personal
        ScriptedAnswer::Input(String::new()), // host → default
        ScriptedAnswer::Input(String::new()), // port → default
        ScriptedAnswer::Input(String::new()), // mailbox → default
        ScriptedAnswer::Input("me@outlook.com".to_string()),
        ScriptedAnswer::Select(0),            // Sync mode — push
        ScriptedAnswer::Select(0),            // Existing mailbox content — import
        ScriptedAnswer::Input(String::new()), // auth_uri → default
        ScriptedAnswer::Input(String::new()), // token endpoint → default
        ScriptedAnswer::Input("client-ms-123".to_string()),
        ScriptedAnswer::Password(String::new()), // client secret → none
        ScriptedAnswer::Input(String::new()),    // scopes → default
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-outlook", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(
        config["auth"]["auth_uri"],
        "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize"
    );
    assert_eq!(
        config["auth"]["token_endpoint"],
        "https://login.microsoftonline.com/consumers/oauth2/v2.0/token"
    );
    assert_eq!(config["auth"]["client_id"], "client-ms-123");
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!([
            "https://outlook.office.com/IMAP.AccessAsUser.All",
            "offline_access"
        ])
    );
}

#[test]
fn wizard_email_outlook_work_account_preselects_organizations_endpoints() {
    // Issue #467: a multitenant org registration ("Accounts in any
    // organizational directory") must use the `/organizations/` tenant —
    // `/common/` fails for it too.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(1),            // Outlook / Office 365
        ScriptedAnswer::Select(1),            // Microsoft account type — work or school
        ScriptedAnswer::Input(String::new()), // host → default
        ScriptedAnswer::Input(String::new()), // port → default
        ScriptedAnswer::Input(String::new()), // mailbox → default
        ScriptedAnswer::Input("me@contoso.com".to_string()),
        ScriptedAnswer::Select(0),            // Sync mode — push
        ScriptedAnswer::Select(0),            // Existing mailbox content — import
        ScriptedAnswer::Input(String::new()), // auth_uri → default
        ScriptedAnswer::Input(String::new()), // token endpoint → default
        ScriptedAnswer::Input("client-ms-123".to_string()),
        ScriptedAnswer::Password(String::new()), // client secret → none
        ScriptedAnswer::Input(String::new()),    // scopes → default
    ]);
    let (config, credential) = build_wizard_config(&entry, "work-outlook", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(
        config["auth"]["auth_uri"],
        "https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize"
    );
    assert_eq!(
        config["auth"]["token_endpoint"],
        "https://login.microsoftonline.com/organizations/oauth2/v2.0/token"
    );
    assert_eq!(config["auth"]["client_id"], "client-ms-123");
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!([
            "https://outlook.office.com/IMAP.AccessAsUser.All",
            "offline_access"
        ])
    );
}

#[test]
fn wizard_email_outlook_single_tenant_uses_tenant_specific_endpoints() {
    // Issue #467: a single-tenant registration ("Accounts in this
    // organizational directory only") cannot use `/organizations/` — the
    // wizard collects the tenant ID or domain and builds both endpoints
    // from it.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(1),                        // Outlook / Office 365
        ScriptedAnswer::Select(3),                        // Microsoft account type — single-tenant
        ScriptedAnswer::Input("contoso.com".to_string()), // tenant ID or domain
        ScriptedAnswer::Input(String::new()),             // host → default
        ScriptedAnswer::Input(String::new()),             // port → default
        ScriptedAnswer::Input(String::new()),             // mailbox → default
        ScriptedAnswer::Input("me@contoso.com".to_string()),
        ScriptedAnswer::Select(0),            // Sync mode — push
        ScriptedAnswer::Select(0),            // Existing mailbox content — import
        ScriptedAnswer::Input(String::new()), // auth_uri → default
        ScriptedAnswer::Input(String::new()), // token endpoint → default
        ScriptedAnswer::Input("client-ms-123".to_string()),
        ScriptedAnswer::Password(String::new()), // client secret → none
        ScriptedAnswer::Input(String::new()),    // scopes → default
    ]);
    let (config, credential) =
        build_wizard_config(&entry, "single-tenant-outlook", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(
        config["auth"]["auth_uri"],
        "https://login.microsoftonline.com/contoso.com/oauth2/v2.0/authorize"
    );
    assert_eq!(
        config["auth"]["token_endpoint"],
        "https://login.microsoftonline.com/contoso.com/oauth2/v2.0/token"
    );
    assert_eq!(config["auth"]["client_id"], "client-ms-123");
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!([
            "https://outlook.office.com/IMAP.AccessAsUser.All",
            "offline_access"
        ])
    );
}

#[test]
fn wizard_email_outlook_account_type_prompt_and_client_id_help_are_pinned() {
    // Issue #467: the Outlook preset must ask which Microsoft account type
    // applies, and the client-ID guidance must state the audience
    // requirement plus the loopback redirect URI that must be registered.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(1),            // Outlook / Office 365
        ScriptedAnswer::Select(0),            // Microsoft account type — personal
        ScriptedAnswer::Input(String::new()), // host → default
        ScriptedAnswer::Input(String::new()), // port → default
        ScriptedAnswer::Input(String::new()), // mailbox → default
        ScriptedAnswer::Input("me@outlook.com".to_string()),
        ScriptedAnswer::Select(0),            // Sync mode — push
        ScriptedAnswer::Select(0),            // Existing mailbox content — import
        ScriptedAnswer::Input(String::new()), // auth_uri → default
        ScriptedAnswer::Input(String::new()), // token endpoint → default
        ScriptedAnswer::Input("client-ms-123".to_string()),
        ScriptedAnswer::Password(String::new()), // client secret → none
        ScriptedAnswer::Input(String::new()),    // scopes → default
    ]);
    build_wizard_config(&entry, "personal-outlook", &prompts).unwrap();

    let messages = prompts.messages();
    assert!(
        messages
            .iter()
            .any(|message| message == "Microsoft account type"),
        "the wizard must ask the Microsoft account type, got: {messages:?}"
    );
    let client_id_prompt = messages
        .iter()
        .find(|message| message.starts_with("OAuth client ID"))
        .unwrap_or_else(|| panic!("no client-ID prompt in: {messages:?}"));
    assert!(
        client_id_prompt.contains("Supported account types"),
        "client-ID help must state the audience requirement: {client_id_prompt}"
    );
    assert!(
        client_id_prompt.contains("http://localhost/callback"),
        "client-ID help must mention the loopback redirect URI: {client_id_prompt}"
    );
}

#[test]
fn wizard_email_yahoo_preset_uses_yahoo_defaults_and_app_password_only() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(2),            // Yahoo
        ScriptedAnswer::Input(String::new()), // host → imap.mail.yahoo.com
        ScriptedAnswer::Input(String::new()), // port → 993
        ScriptedAnswer::Input(String::new()), // mailbox → INBOX
        ScriptedAnswer::Input("me@yahoo.com".to_string()),
        ScriptedAnswer::Select(1), // Sync mode — polling
        ScriptedAnswer::Select(1), // 15 minutes
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Password("abcd efgh ijkl mnop".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-yahoo", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::Secret(_)
    ));
    assert_eq!(config["host"], "imap.mail.yahoo.com");
    assert_eq!(config["port"], 993);
    assert_eq!(config["mode"], "poll");
    assert_eq!(config["poll_interval_secs"], 15 * 60);
    assert_eq!(config["auth"]["kind"], "app_password");
    assert_eq!(config["auth"]["username"], "me@yahoo.com");
}

#[test]
fn wizard_email_icloud_preset_uses_icloud_defaults_and_app_password_only() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(4),            // iCloud
        ScriptedAnswer::Input(String::new()), // host → imap.mail.me.com
        ScriptedAnswer::Input(String::new()), // port → 993
        ScriptedAnswer::Input(String::new()), // mailbox → INBOX
        ScriptedAnswer::Input("me@icloud.com".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Password("abcd-efgh-ijkl-mnop".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-icloud", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::Secret(_)
    ));
    assert_eq!(config["host"], "imap.mail.me.com");
    assert_eq!(config["port"], 993);
    assert_eq!(config["mailbox"], "INBOX");
    assert_eq!(config["auth"]["kind"], "app_password");
    assert_eq!(config["auth"]["username"], "me@icloud.com");
}

#[test]
fn wizard_email_proton_preset_uses_bridge_defaults_and_app_password_only() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(3),            // Proton Mail (Bridge)
        ScriptedAnswer::Input(String::new()), // host → 127.0.0.1
        ScriptedAnswer::Input(String::new()), // port → 1143
        ScriptedAnswer::Input(String::new()), // mailbox → INBOX
        ScriptedAnswer::Input("me@proton.me".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Password("abcd efgh ijkl mnop".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "personal-proton", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::Secret(_)
    ));
    assert_eq!(config["host"], "127.0.0.1");
    assert_eq!(config["port"], 1143);
    assert_eq!(config["mailbox"], "INBOX");
    assert_eq!(config["auth"]["kind"], "app_password");
    assert_eq!(config["auth"]["username"], "me@proton.me");
}

#[test]
fn wizard_email_custom_imap_keeps_free_form_flow() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(5), // Custom IMAP
        ScriptedAnswer::Input("imap.example.com".to_string()),
        ScriptedAnswer::Input("143".to_string()),
        ScriptedAnswer::Input("Archive".to_string()),
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push
        ScriptedAnswer::Select(1), // Existing mailbox content — new only
        ScriptedAnswer::Select(0), // App password
        ScriptedAnswer::Password("hunter2".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "custom-mail", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::Secret(_)
    ));
    assert_eq!(config["host"], "imap.example.com");
    assert_eq!(config["port"], 143);
    assert_eq!(config["mailbox"], "Archive");
    assert_eq!(config["mode"], "auto");
    assert_eq!(config["initial_backfill"], false);
    assert_eq!(config["auth"]["kind"], "app_password");
    assert_eq!(config["auth"]["username"], "me@example.com");
}

#[test]
fn wizard_email_custom_imap_oauth_accepts_user_endpoints() {
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(5), // Custom IMAP
        ScriptedAnswer::Input("imap.example.com".to_string()),
        ScriptedAnswer::Input(String::new()), // port → 993
        ScriptedAnswer::Input(String::new()), // mailbox → INBOX
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Select(1), // OAuth 2.0 — browser login
        ScriptedAnswer::Input("https://auth.example.com/authorize".to_string()),
        ScriptedAnswer::Input("https://auth.example.com/token".to_string()),
        ScriptedAnswer::Input("client-custom".to_string()),
        ScriptedAnswer::Password(String::new()),
        ScriptedAnswer::Input("mail.read".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "custom-mail", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(
        config["auth"]["auth_uri"],
        "https://auth.example.com/authorize"
    );
    assert_eq!(
        config["auth"]["token_endpoint"],
        "https://auth.example.com/token"
    );
    assert_eq!(config["auth"]["scopes"], serde_json::json!(["mail.read"]));
}

#[test]
fn wizard_email_custom_imap_oauth_rejects_empty_scopes() {
    // The custom IMAP preset has no default scope, so accepting a blank
    // scopes prompt would build an authorize request with no scope and the
    // flow could never obtain mailbox access — reject it like every other
    // required prompt.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(5), // Custom IMAP
        ScriptedAnswer::Input("imap.example.com".to_string()),
        ScriptedAnswer::Input(String::new()), // port → 993
        ScriptedAnswer::Input(String::new()), // mailbox → INBOX
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Select(1), // OAuth 2.0 — browser login
        ScriptedAnswer::Input("https://auth.example.com/authorize".to_string()),
        ScriptedAnswer::Input("https://auth.example.com/token".to_string()),
        ScriptedAnswer::Input("client-custom".to_string()),
        ScriptedAnswer::Password(String::new()),
        ScriptedAnswer::Input(String::new()), // scopes → blank, no default
    ]);
    let err = build_wizard_config(&entry, "custom-mail", &prompts).unwrap_err();
    assert_eq!(err, "OAuth scopes is required");
}

#[test]
fn wizard_email_custom_imap_oauth_rejects_parsed_empty_scopes() {
    // A non-blank answer that parses to zero scopes (e.g. `", ,"`) must be
    // rejected too — `required` only checks the raw input, so without this
    // guard the wizard would build an authorize request with no scope.
    let entry = email_catalog_entry();
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(5), // Custom IMAP
        ScriptedAnswer::Input("imap.example.com".to_string()),
        ScriptedAnswer::Input(String::new()), // port → 993
        ScriptedAnswer::Input(String::new()), // mailbox → INBOX
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(0), // Sync mode — push
        ScriptedAnswer::Select(0), // Existing mailbox content — import
        ScriptedAnswer::Select(1), // OAuth 2.0 — browser login
        ScriptedAnswer::Input("https://auth.example.com/authorize".to_string()),
        ScriptedAnswer::Input("https://auth.example.com/token".to_string()),
        ScriptedAnswer::Input("client-custom".to_string()),
        ScriptedAnswer::Password(String::new()),
        ScriptedAnswer::Input(", ,".to_string()), // scopes → parses to []
    ]);
    let err = build_wizard_config(&entry, "custom-mail", &prompts).unwrap_err();
    assert_eq!(err, "OAuth scopes is required");
}

#[test]
fn wizard_calendar_google_preset_uses_google_endpoints_and_computed_url() {
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "calendar".to_string(),
        backend: "caldav".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(0), // Google Calendar
        ScriptedAnswer::Input("me@gmail.com".to_string()),
        ScriptedAnswer::Input(String::new()), // URL → computed default
        ScriptedAnswer::Input("client-google".to_string()),
        ScriptedAnswer::Password(String::new()), // client secret → none
        ScriptedAnswer::Input(String::new()),    // scopes → default
    ]);
    let (config, credential) = build_wizard_config(&entry, "google-cal", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(
        config["calendar_url"],
        "https://apidata.googleusercontent.com/caldav/v2/me@gmail.com/events"
    );
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(config["auth"]["username"], "me@gmail.com");
    assert_eq!(
        config["auth"]["auth_uri"],
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!(["https://www.googleapis.com/auth/calendar"])
    );
}

#[test]
fn wizard_calendar_icloud_preset_uses_icloud_url_and_app_password() {
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "calendar".to_string(),
        backend: "caldav".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(1),            // iCloud
        ScriptedAnswer::Input(String::new()), // URL → default
        ScriptedAnswer::Input("me@icloud.com".to_string()),
        ScriptedAnswer::Password("abcd-efgh-ijkl-mnop".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "icloud-cal", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::Secret(_)
    ));
    assert_eq!(config["calendar_url"], "https://caldav.icloud.com/");
    assert_eq!(config["auth"]["kind"], "app_password");
    assert_eq!(config["auth"]["username"], "me@icloud.com");
}

#[test]
fn wizard_calendar_yahoo_preset_uses_yahoo_url_and_app_password() {
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "calendar".to_string(),
        backend: "caldav".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(2),            // Yahoo
        ScriptedAnswer::Input(String::new()), // URL → default
        ScriptedAnswer::Input("me@yahoo.com".to_string()),
        ScriptedAnswer::Password("abcd efgh ijkl mnop".to_string()),
    ]);
    let (config, credential) = build_wizard_config(&entry, "yahoo-cal", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::Secret(_)
    ));
    assert_eq!(config["calendar_url"], "https://caldav.calendar.yahoo.com/");
    assert_eq!(config["auth"]["kind"], "app_password");
    assert_eq!(config["auth"]["username"], "me@yahoo.com");
}

#[test]
fn wizard_caldav_whitespace_only_app_password_is_rejected() {
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "calendar".to_string(),
        backend: "caldav".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(3), // Calendar provider → Custom CalDAV
        ScriptedAnswer::Input("https://dav.example.com/cal".to_string()),
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(0),                   // App password
        ScriptedAnswer::Password("   ".to_string()), // whitespace-only
    ]);
    let err = build_wizard_config(&entry, "calendar", &prompts).unwrap_err();
    assert_eq!(err, "App password is required");
}

#[test]
fn wizard_caldav_oauth_config_includes_scopes() {
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "calendar".to_string(),
        backend: "caldav".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(3), // Calendar provider → Custom CalDAV
        ScriptedAnswer::Input("https://dav.example.com/cal".to_string()),
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(1), // OAuth 2.0 — browser login
        ScriptedAnswer::Input("https://accounts.example.com/auth".to_string()),
        ScriptedAnswer::Input("https://accounts.example.com/token".to_string()),
        ScriptedAnswer::Input("client-456".to_string()),
        ScriptedAnswer::Password(String::new()), // client secret → none
        ScriptedAnswer::Input(String::new()),    // scopes → default Google Calendar
    ]);
    let (config, credential) = build_wizard_config(&entry, "calendar", &prompts).unwrap();
    assert!(matches!(
        credential,
        super::wizard::WizardCredential::OAuth {
            client_secret: None
        }
    ));
    assert_eq!(config["calendar_url"], "https://dav.example.com/cal");
    assert_eq!(config["auth"]["kind"], "oauth");
    assert_eq!(config["auth"]["username"], "me@example.com");
    assert_eq!(config["auth"]["client_id"], "client-456");
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!(["https://www.googleapis.com/auth/calendar"])
    );
    assert!(
        config["auth"].get("client_secret").is_none(),
        "blank client secret must be omitted"
    );
}

#[test]
fn wizard_caldav_oauth_scope_prompt_accepts_custom_list() {
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "calendar".to_string(),
        backend: "caldav".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(3), // Calendar provider → Custom CalDAV
        ScriptedAnswer::Input("https://dav.example.com/cal".to_string()),
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(1), // OAuth
        ScriptedAnswer::Input("https://auth.example.com/auth".to_string()),
        ScriptedAnswer::Input("https://auth.example.com/token".to_string()),
        ScriptedAnswer::Input("client-456".to_string()),
        ScriptedAnswer::Password(String::new()),
        ScriptedAnswer::Input("scope.a scope.b,scope.c".to_string()),
    ]);
    let (config, _) = build_wizard_config(&entry, "calendar", &prompts).unwrap();
    assert_eq!(
        config["auth"]["scopes"],
        serde_json::json!(["scope.a", "scope.b", "scope.c"])
    );
}

#[test]
fn wizard_caldav_oauth_rejects_parsed_empty_scopes() {
    // A blank scopes answer falls back to the CALDAV_SCOPE default, but a
    // non-blank answer that parses to zero scopes (e.g. `", ,"`) must be
    // rejected — otherwise the wizard would build an authorize request with
    // no scope, the same scope-less failure the email path guards against.
    let entry = mimir_api_types::ConnectorCatalogEntry {
        connector_type: "calendar".to_string(),
        backend: "caldav".to_string(),
    };
    let prompts = ScriptedPrompt::new(vec![
        ScriptedAnswer::Select(3), // Calendar provider → Custom CalDAV
        ScriptedAnswer::Input("https://dav.example.com/cal".to_string()),
        ScriptedAnswer::Input("me@example.com".to_string()),
        ScriptedAnswer::Select(1), // OAuth 2.0 — browser login
        ScriptedAnswer::Input("https://auth.example.com/auth".to_string()),
        ScriptedAnswer::Input("https://auth.example.com/token".to_string()),
        ScriptedAnswer::Input("client-456".to_string()),
        ScriptedAnswer::Password(String::new()),
        ScriptedAnswer::Input(", ,".to_string()), // scopes → parses to []
    ]);
    let err = build_wizard_config(&entry, "calendar", &prompts).unwrap_err();
    assert_eq!(err, "OAuth scopes is required");
}

#[test]
fn parse_scopes_splits_on_commas_and_whitespace() {
    assert_eq!(parse_scopes("a,b c\td"), vec!["a", "b", "c", "d"]);
    assert_eq!(parse_scopes("  ,  "), Vec::<String>::new());
    assert_eq!(parse_scopes(""), Vec::<String>::new());
    assert_eq!(parse_scopes("single"), vec!["single"]);
}
