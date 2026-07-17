//! Integration tests for the connector secret store (Phase 3 F10 / #187).
//!
//! Exercises [`FileSecretStore`] and [`InMemorySecretStore`] against the
//! public [`SecretStore`] trait: round-trip for all three [`SecretBundle`]
//! kinds, slug sanitisation, file/dir permission enforcement (Unix), and
//! delete semantics.

#![deny(unsafe_code)]

use chrono::{TimeZone, Utc};
use mimir_connectors::secrets::{
    FileSecretStore, InMemorySecretStore, SecretBundle, SecretError, SecretStore,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;

// ---------------------------------------------------------------------------
// SecretBundle serde
// ---------------------------------------------------------------------------

#[test]
fn secret_bundle_oauth_round_trips() {
    let bundle = SecretBundle::OAuth {
        access_token: "ya29.access".to_string(),
        refresh_token: Some("1//refresh".to_string()),
        expires_at: Some(Utc.with_ymd_and_hms(2026, 7, 17, 12, 0, 0).unwrap()),
    };
    let json = serde_json::to_value(&bundle).unwrap();
    assert_eq!(json["kind"], "oauth");
    assert_eq!(json["access_token"], "ya29.access");
    let back: SecretBundle = serde_json::from_value(json).unwrap();
    assert_eq!(bundle, back);
}

#[test]
fn secret_bundle_oauth_nullable_fields_round_trip() {
    // Not all grants issue a refresh token or return an expiry.
    let bundle = SecretBundle::OAuth {
        access_token: "tok".to_string(),
        refresh_token: None,
        expires_at: None,
    };
    let json = serde_json::to_string(&bundle).unwrap();
    let back: SecretBundle = serde_json::from_str(&json).unwrap();
    assert_eq!(bundle, back);
}

#[test]
fn secret_bundle_api_token_round_trips() {
    let bundle = SecretBundle::ApiToken {
        token: "ghp_abc".to_string(),
    };
    let json = serde_json::to_value(&bundle).unwrap();
    assert_eq!(json["kind"], "api_token");
    assert_eq!(json["token"], "ghp_abc");
    let back: SecretBundle = serde_json::from_value(json).unwrap();
    assert_eq!(bundle, back);
}

#[test]
fn secret_bundle_app_password_round_trips() {
    let bundle = SecretBundle::AppPassword {
        password: "hunter2".to_string(),
    };
    let json = serde_json::to_value(&bundle).unwrap();
    assert_eq!(json["kind"], "app_password");
    assert_eq!(json["password"], "hunter2");
    let back: SecretBundle = serde_json::from_value(json).unwrap();
    assert_eq!(bundle, back);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn oauth_bundle() -> SecretBundle {
    SecretBundle::OAuth {
        access_token: "ya29.access".to_string(),
        refresh_token: Some("1//refresh".to_string()),
        expires_at: Some(Utc.with_ymd_and_hms(2026, 7, 17, 12, 0, 0).unwrap()),
    }
}

async fn round_trips<S: SecretStore>(store: &S, slug: &str, bundle: SecretBundle) {
    assert!(store.load(slug).await.unwrap().is_none(), "store not empty");
    store.store(slug, &bundle).await.unwrap();
    let loaded = store.load(slug).await.unwrap();
    assert_eq!(loaded.as_ref(), Some(&bundle), "round trip mismatch");
    store.delete(slug).await.unwrap();
    assert!(
        store.load(slug).await.unwrap().is_none(),
        "delete did not clear"
    );
}

// ---------------------------------------------------------------------------
// FileSecretStore round trips
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_store_round_trip_oauth() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(dir.path().to_path_buf());
    round_trips(&store, "gmail-personal", oauth_bundle()).await;
}

#[tokio::test]
async fn file_store_round_trip_api_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(dir.path().to_path_buf());
    round_trips(
        &store,
        "home-assistant",
        SecretBundle::ApiToken {
            token: "long-lived-token".to_string(),
        },
    )
    .await;
}

#[tokio::test]
async fn file_store_round_trip_app_password() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(dir.path().to_path_buf());
    round_trips(
        &store,
        "fastmail",
        SecretBundle::AppPassword {
            password: "app-pass".to_string(),
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// Load-missing + delete semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_store_load_missing_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(dir.path().to_path_buf());
    assert!(store.load("never-stored").await.unwrap().is_none());
}

#[tokio::test]
async fn file_store_delete_missing_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(dir.path().to_path_buf());
    // Deleting a slug that was never stored must not error.
    store.delete("never-stored").await.unwrap();
}

// ---------------------------------------------------------------------------
// Slug sanitisation / path-traversal guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_store_rejects_invalid_slug() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(dir.path().to_path_buf());
    let bad = ["", "..", "../etc", "a/b", "a b", "a.b", "café", "a:b"];
    for slug in bad {
        let bundle = SecretBundle::ApiToken {
            token: "x".to_string(),
        };
        let res = store.store(slug, &bundle).await;
        assert!(
            matches!(res, Err(SecretError::InvalidSlug { .. })),
            "slug `{slug}` should be rejected"
        );
    }
    // No files should have escaped the directory.
    assert!(
        fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .count()
            == 0
    );
}

#[tokio::test]
async fn file_store_accepts_valid_slugs() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(dir.path().to_path_buf());
    let good = [
        "gmail-personal",
        "Gmail_Personal",
        "cal1",
        "a",
        "photos-2026",
    ];
    for slug in good {
        store.store(slug, &oauth_bundle()).await.unwrap();
        assert!(
            store.load(slug).await.unwrap().is_some(),
            "valid slug `{slug}` not stored"
        );
    }
}

// ---------------------------------------------------------------------------
// Permissions (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn file_store_creates_dir_0700_and_file_0600() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(tmp.path().join("secrets"));
    store.store("slug", &oauth_bundle()).await.unwrap();

    let dir_mode = fs::metadata(store.dir()).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "secrets dir mode is {dir_mode:o}");

    let file_mode = fs::metadata(store.dir().join("slug.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600, "secret file mode is {file_mode:o}");
}

#[cfg(unix)]
#[tokio::test]
async fn file_store_refuses_too_open_file() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(tmp.path().join("secrets"));
    store.store("slug", &oauth_bundle()).await.unwrap();
    // Corrupt perms: world-readable.
    let mut perms = fs::metadata(store.dir().join("slug.json"))
        .unwrap()
        .permissions();
    perms.set_mode(0o644);
    fs::set_permissions(store.dir().join("slug.json"), perms).unwrap();

    let res = store.load("slug").await;
    assert!(
        matches!(res, Err(SecretError::InsecurePermissions { .. })),
        "load of too-open file should be refused"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn file_store_refuses_too_open_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(tmp.path().join("secrets"));
    store.store("slug", &oauth_bundle()).await.unwrap();
    // Make the parent dir world-traversable/readable.
    let mut perms = fs::metadata(store.dir()).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(store.dir(), perms).unwrap();

    let res = store.load("slug").await;
    assert!(
        matches!(res, Err(SecretError::InsecurePermissions { .. })),
        "load with too-open dir should be refused"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn file_store_store_restores_file_perms_if_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(tmp.path().join("secrets"));
    store.store("slug", &oauth_bundle()).await.unwrap();
    // Loosen then re-store: result must be 0600 again.
    let mut perms = fs::metadata(store.dir().join("slug.json"))
        .unwrap()
        .permissions();
    perms.set_mode(0o644);
    fs::set_permissions(store.dir().join("slug.json"), perms).unwrap();
    store.store("slug", &oauth_bundle()).await.unwrap();
    let mode = fs::metadata(store.dir().join("slug.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "re-store did not re-tighten perms to 0600: {mode:o}"
    );
}

// ---------------------------------------------------------------------------
// Atomic writes leave no stray temp files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_store_store_leaves_no_temp_files() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileSecretStore::with_dir(tmp.path().join("secrets"));
    store.store("slug", &oauth_bundle()).await.unwrap();
    // Only the one canonical file should be present.
    let entries: Vec<_> = fs::read_dir(store.dir())
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1, "stray files left in secrets dir");
    assert!(entries[0].file_name().to_string_lossy().ends_with(".json"));
}

// ---------------------------------------------------------------------------
// InMemorySecretStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn in_memory_store_round_trip_all_kinds() {
    let store = InMemorySecretStore::new();
    round_trips(&store, "gmail-personal", oauth_bundle()).await;
    round_trips(
        &store,
        "home-assistant",
        SecretBundle::ApiToken {
            token: "tok".to_string(),
        },
    )
    .await;
    round_trips(
        &store,
        "fastmail",
        SecretBundle::AppPassword {
            password: "pw".to_string(),
        },
    )
    .await;
}

#[tokio::test]
async fn in_memory_store_load_missing_returns_none() {
    let store = InMemorySecretStore::new();
    assert!(store.load("nothing").await.unwrap().is_none());
}

#[tokio::test]
async fn in_memory_store_delete_missing_is_idempotent() {
    let store = InMemorySecretStore::new();
    store.delete("nothing").await.unwrap();
}
