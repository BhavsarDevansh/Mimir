//! Unit tests for Calendar credential resolution (OAuth refresh path,
//! issue #240).

use super::*;
use crate::calendar::CalendarConfigDto;
use crate::calendar::caldav::CalDavAuth;
use chrono::Utc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn oauth_config(token_endpoint: &str) -> serde_json::Value {
    serde_json::json!({
        "calendar_url": "https://caldav.example.com/calendar/",
        "auth": {
            "kind": "oauth",
            "auth_uri": "https://oauth.example.com/authorize",
            "token_endpoint": token_endpoint,
            "client_id": "cid",
            "client_secret": "secret",
            "scopes": ["https://www.googleapis.com/auth/calendar.readonly"],
        },
    })
}

fn oauth_bundle(access_token: &str, expires_at: Option<chrono::DateTime<Utc>>) -> SecretBundle {
    SecretBundle::OAuth {
        access_token: access_token.into(),
        refresh_token: Some("rt".into()),
        expires_at,
    }
}

#[test]
fn oauth_config_without_auth_uri_deserializes_from_stored_record() {
    // Records persisted before the `auth_uri` field (pre-0.97.0) must still
    // load — `auth_uri` is only required when starting the interactive PKCE
    // flow, not for token refresh.
    let dto: CalendarConfigDto = serde_json::from_value(serde_json::json!({
        "calendar_url": "https://caldav.example.com/calendar/",
        "auth": {
            "kind": "oauth",
            "token_endpoint": "https://oauth.example.com/token",
            "client_id": "cid",
        },
    }))
    .expect("stored record without auth_uri must load");
    let CalendarAuthMethod::OAuth { auth_uri, .. } = dto.auth else {
        panic!("expected OAuth auth method");
    };
    assert_eq!(auth_uri, None);
}

#[tokio::test]
async fn resolve_auth_oauth_refreshes_expired_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "fresh",
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let connector = CalendarConnector::from_config(
        oauth_config(&format!("{}/token", server.uri())),
        None,
        None,
    )
    .expect("config");
    let bundle = oauth_bundle("stale", Some(Utc::now() - chrono::Duration::seconds(1)));
    let (auth, refreshed) = connector.resolve_auth(&bundle).await.expect("refresh");
    match auth {
        CalDavAuth::Bearer { token } => assert_eq!(token, "fresh"),
        other => panic!("expected Bearer auth, got {other:?}"),
    }
    let SecretBundle::OAuth {
        access_token,
        refresh_token,
        ..
    } = refreshed.expect("refreshed bundle")
    else {
        panic!("expected OAuth bundle");
    };
    assert_eq!(access_token, "fresh");
    assert_eq!(
        refresh_token.as_deref(),
        Some("rt"),
        "prior refresh token retained"
    );
}

#[tokio::test]
async fn resolve_auth_oauth_reuses_unexpired_token() {
    let connector =
        CalendarConnector::from_config(oauth_config("https://oauth.example.com/token"), None, None)
            .expect("config");
    let bundle = oauth_bundle("ya29.access", Some(Utc::now() + chrono::Duration::hours(1)));
    let (auth, refreshed) = connector.resolve_auth(&bundle).await.expect("ok");
    assert!(refreshed.is_none(), "no refresh expected for a live token");
    match auth {
        CalDavAuth::Bearer { token } => assert_eq!(token, "ya29.access"),
        other => panic!("expected Bearer auth, got {other:?}"),
    }
}
