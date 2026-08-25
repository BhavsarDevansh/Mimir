//! Daemon-level E2E for the OAuth PKCE login (Phase 3 T2 / #207): the real
//! `mimir connector add` CLI runs the interactive PKCE flow against the
//! in-process mock OAuth server (via a `$BROWSER` fake browser) and ingests
//! the exchanged tokens into the real daemon.

#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;

use common::TestDaemon;
use mimir_connectors::mock_oauth::MockOAuthServer;

#[test]
fn oauth_add_runs_pkce_flow_against_mock_server_and_ingests_tokens() {
    let daemon = TestDaemon::start();
    let oauth = MockOAuthServer::start();

    // The fake browser script needs `curl`; fail with a clear prerequisite
    // message instead of a timeout deep inside the flow.
    let curl_check = std::process::Command::new("curl")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn curl --version");
    assert!(
        curl_check.success(),
        "the OAuth E2E test requires `curl` (used by the fake browser script)"
    );

    // A fake browser: `webbrowser` on Linux honours `$BROWSER`; the script
    // follows the HTTPS authorize redirect into the loopback callback
    // (`curl -k` accepts the mock's self-signed certificate).
    let browser = daemon.home_dir.join("fake-browser.sh");
    std::fs::write(
        &browser,
        "#!/bin/sh\nexec curl -k -L -s -o /dev/null \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();

    let config = serde_json::json!({
        "auth": {
            "kind": "oauth",
            "auth_uri": oauth.authorize_url(),
            "token_endpoint": oauth.token_url(),
            "client_id": "mimir-test-client",
            "scopes": ["read"],
        }
    });

    let (stdout, stderr, status) = daemon.run_cli_with_env(
        &[
            "connector",
            "add",
            "gmail",
            "--backend",
            "test",
            "--slug",
            "oauth-demo",
            "--config-json",
            &config.to_string(),
            "--json",
        ],
        &[("BROWSER", browser.to_str().expect("browser path"))],
    );
    assert!(
        status.success(),
        "connector add failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let created: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(created["slug"], "oauth-demo");
    assert_eq!(
        created["auth_state"], "authenticated",
        "the PKCE flow must exchange tokens and ingest them into the daemon"
    );

    // The mock server saw the full round trip: one authorize request and one
    // token exchange.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let authorizes = rt.block_on(oauth.authorize_requests());
    assert_eq!(authorizes.len(), 1, "exactly one authorize request");
    let tokens = rt.block_on(oauth.token_requests());
    assert_eq!(tokens.len(), 1, "exactly one token exchange");

    // Issue #507: re-authing an expired OAuth connector must not require the
    // user to re-supply the OAuth endpoints — the daemon surfaces the stored
    // non-secret auth config, so `mimir connector auth` re-runs the PKCE flow
    // from it and ingests a fresh bundle.
    let (stdout, stderr, status) = daemon.run_cli_with_env(
        &["connector", "auth", "oauth-demo", "--json"],
        &[("BROWSER", browser.to_str().expect("browser path"))],
    );
    assert!(
        status.success(),
        "connector auth without re-supplied config failed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let reauthed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(reauthed["slug"], "oauth-demo");
    assert_eq!(
        reauthed["auth_state"], "authenticated",
        "the re-auth PKCE flow must exchange tokens and ingest them"
    );
    assert_eq!(
        rt.block_on(oauth.authorize_requests()).len(),
        2,
        "the re-auth must run a second authorize round"
    );
    assert_eq!(
        rt.block_on(oauth.token_requests()).len(),
        2,
        "the re-auth must run a second token exchange"
    );

    // The authenticated instance can be resumed and synced.
    let resumed = daemon.run_cli_json(&["connector", "resume", "oauth-demo", "--json"]);
    assert_eq!(resumed["status"], "active");
    let synced = daemon.run_cli_json(&["connector", "sync", "oauth-demo", "--json"]);
    assert_eq!(
        synced["fetched"], 0,
        "the mock emits no facts without a facts knob"
    );

    daemon.stop();
}
