use super::super::config::*;

use chrono::Utc;

use crate::connector::{Connector, ConnectorError, ConnectorMode};
use crate::email::connector::EmailConnector;
use crate::email::imap::ImapAuth;
use crate::secrets::SecretBundle;
use mimir_knowledge::models::enums::ConnectorType;

pub(crate) fn app_config() -> serde_json::Value {
    serde_json::json!({
        "host": "imap.example.com",
        "auth": { "kind": "app_password", "username": "devansh@example.com" },
        "__slug": "gmail-personal",
        "__cursor": "17:42",
    })
}

#[test]
fn cursor_round_trip() {
    assert_eq!(encode_cursor(17, 42), "17:42");
    assert_eq!(parse_cursor("17:42"), Some((17, 42)));
    assert_eq!(parse_cursor("0:0"), Some((0, 0)));
    assert_eq!(parse_cursor(""), None);
    assert_eq!(parse_cursor("17"), None);
    assert_eq!(parse_cursor("abc:def"), None);
    assert_eq!(parse_cursor("17:42:9"), None);
}

#[test]
fn from_config_seeds_cursor_and_slug() {
    // The factory extracts `__cursor` from config and passes it as the
    // `cursor` param (mirroring the Calendar connector / supervisor).
    let connector =
        EmailConnector::from_config(app_config(), None, Some("17:42".into())).expect("config");
    assert_eq!(connector.id(), "gmail-personal");
    assert_eq!(connector.connector_type(), ConnectorType::Gmail);
    assert_eq!(connector.name(), "Gmail");
    assert_eq!(connector.port(), 993);
    assert_eq!(connector.mailbox(), "INBOX");
    assert_eq!(*connector.last_uid.try_lock().unwrap(), Some((17, 42)));
    // Auto mode with no capability probe yet → Push (IDLE preferred).
    assert!(matches!(connector.mode(), ConnectorMode::Push));
}

#[test]
fn from_config_poll_mode_returns_polling() {
    let mut cfg = app_config();
    cfg["mode"] = serde_json::json!("poll");
    cfg["poll_interval_secs"] = 120.into();
    cfg["poll_jitter_secs"] = 10.into();
    let connector = EmailConnector::from_config(cfg, None, None).expect("config");
    assert!(matches!(
        connector.mode(),
        ConnectorMode::Polling { interval, jitter } if interval == Duration::from_secs(120)
            && jitter == Duration::from_secs(10)
    ));
}

#[test]
fn from_config_explicit_idle_mode_is_push() {
    let mut cfg = app_config();
    cfg["mode"] = serde_json::json!("idle");
    let connector = EmailConnector::from_config(cfg, None, None).expect("config");
    assert!(matches!(connector.mode(), ConnectorMode::Push));
}

#[test]
fn from_config_custom_port_and_mailbox() {
    let mut cfg = app_config();
    cfg["port"] = 143.into();
    cfg["mailbox"] = "[Gmail]/All Mail".into();
    let connector = EmailConnector::from_config(cfg, None, None).expect("config");
    assert_eq!(connector.port(), 143);
    assert_eq!(connector.mailbox(), "[Gmail]/All Mail");
}

#[test]
fn from_config_rejects_bad_config() {
    let bad = serde_json::json!({ "host": "imap.example.com" }); // missing auth
    assert!(matches!(
        EmailConnector::from_config(bad, None, None),
        Err(ConnectorError::Config(_))
    ));
}

#[test]
fn auto_mode_falls_back_to_polling_when_idle_not_advertised() {
    let connector = EmailConnector::from_config(app_config(), None, None).expect("config");
    // Simulate the capability probe caching `false`.
    *connector.supports_idle.lock().unwrap() = Some(false);
    assert!(matches!(connector.mode(), ConnectorMode::Polling { .. }));
}

#[tokio::test]
async fn auth_method_mismatch_is_an_error() {
    // App-password config but an OAuth bundle stored.
    let connector = EmailConnector::from_config(app_config(), None, None).expect("config");
    let bundle = SecretBundle::OAuth {
        access_token: "t".into(),
        refresh_token: None,
        expires_at: None,
    };
    assert!(matches!(
        connector.resolve_auth(&bundle).await,
        Err(ConnectorError::Authentication(_))
    ));
}

#[tokio::test]
async fn resolve_auth_app_password_builds_login() {
    let connector = EmailConnector::from_config(app_config(), None, None).expect("config");
    let bundle = SecretBundle::AppPassword {
        password: "hunter2".into(),
    };
    let (auth, refreshed) = connector.resolve_auth(&bundle).await.expect("ok");
    assert!(refreshed.is_none());
    match auth {
        ImapAuth::Login { username, password } => {
            assert_eq!(username, "devansh@example.com");
            assert_eq!(password, "hunter2");
        }
        other => panic!("expected Login, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_auth_oauth_reuses_unexpired_token() {
    let mut cfg = app_config();
    cfg["auth"] = serde_json::json!({
        "kind": "oauth",
        "username": "devansh@example.com",
        "token_endpoint": "https://oauth.example.com/token",
        "client_id": "cid",
    });
    let connector = EmailConnector::from_config(cfg, None, None).expect("config");
    let bundle = SecretBundle::OAuth {
        access_token: "ya29.access".into(),
        refresh_token: Some("rt".into()),
        expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
    };
    let (auth, refreshed) = connector.resolve_auth(&bundle).await.expect("ok");
    assert!(refreshed.is_none(), "no refresh expected for a live token");
    match auth {
        ImapAuth::Xoauth2 {
            username,
            access_token,
        } => {
            assert_eq!(username, "devansh@example.com");
            assert_eq!(access_token, "ya29.access");
        }
        other => panic!("expected Xoauth2, got {other:?}"),
    }
}
