# Connector Secret Store (mimir-connectors)

> **Phase:** 3 — Connectors
>
> **Issue:** #187 / F10
>
> **Design source of truth:** `VISION/09-Roadmap/Phase-3-Plan.md`
>
> **Landed in:** v0.71.0

## Purpose

A single credential store for every connector auth kind. One `SecretBundle` enum — OAuth 2.0, API token, app password — is keyed by the connector instance slug, so the supervisor, CLI, and server routes never branch on *which* store to talk to: they fetch a bundle by slug and pattern-match the kind.

## Public API

```rust
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn load(&self, slug: &str)   -> Result<Option<SecretBundle>, SecretError>;
    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError>;
    async fn delete(&self, slug: &str) -> Result<(), SecretError>;
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SecretBundle {
    #[serde(rename = "oauth")]
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<DateTime<Utc>>,
    },
    ApiToken { token: String },
    AppPassword { password: String },
}
```

- `load` returns `Ok(None)` when a slug has no stored credentials (a connector that has not been authenticated yet).
- `store` is idempotent — storing over an existing slug overwrites it atomically.
- `delete` is idempotent — deleting a missing slug is `Ok(())`.

## FileSecretStore (V1 default)

One JSON file per connector instance at `~/.local/share/mimir/secrets/<slug>.json`, internally-tagged (`{"kind":"oauth","access_token":…}`), **plaintext at rest**, file mode `0600`, parent directory `0700`.

- **Fail-closed permissions.** On `load`, if the secret file *or* its parent directory has any group/other permission bits set, the store returns `SecretError::InsecurePermissions` instead of reading the file — it never leaks a credential whose protection has been weakened. `load` opens the file first and stats the open descriptor (not the path) before reading, so there is no time-of-check-to-time-of-use window where the file could be swapped between the permission check and the read.
- **Mode re-tightening.** `store` and the directory-ensure step always (re)apply `0600`/`0700`, so a manually-loosened file or dir is corrected on the next write. The temp file is tightened to `0600` *before* the atomic `rename`, so the secret is never observable at its final path with the looser default mode inherited from the umask.
- **Atomic writes.** Serialise to a uniquely-named sibling temp file, then `rename` onto the final path. The temp file name is `<slug>.json.tmp.<pid>.<counter>`, embedding the process id and a per-process monotonic counter so two concurrent `store` calls for the same slug (within one process or across processes sharing the secrets directory) never collide on the same temp file. A crash mid-write cannot leave a truncated secret that silently logs a connector out. On success the temp file is replaced by the rename; if the rename fails the temp file is best-effort removed so no stale temp files linger.
- **Path-traversal safety.** Slugs are validated against `[A-Za-z0-9_-]{1,128}` before any filesystem access — empty, `..`, path separators, spaces, dots, and non-ASCII are rejected with `SecretError::InvalidSlug`. The knowledge graph enforces slug uniqueness, but the store does not trust that.
- **Cheap clone.** `FileSecretStore` holds only a `PathBuf`; it is `Clone` and stateless, so multiple connectors can share one cheaply.

### Path helpers

Two helpers were added to `mimir-core::paths` so the store does not hardcode the layout (DRY / XDG-consistent):

```rust
pub fn secrets_dir()            -> Result<PathBuf, PathsError>; // data_dir().join("secrets")
pub fn secrets_file(slug: &str) -> Result<PathBuf, PathsError>; // secrets_dir().join("<slug>.json")
```

`FileSecretStore::new()` resolves the default root via `secrets_dir()`; `FileSecretStore::with_dir(path)` overrides it (used by tests). The directory is created lazily on first `store`, not at construction.

## InMemorySecretStore

A `Mutex<HashMap<String, SecretBundle>>` backend, included as a test/helper for the `mock` connector and unit tests. Not for production persistence. Wrap in `Arc` to share across tasks.

## KeyringSecretStore (F11 / #188, opt-in)

An OS-keychain backend over the `keyring` crate — macOS Keychain, Linux/BSD Secret Service (gnome-keyring / KWallet over D-Bus), and Windows Credential Manager. Feature-gated behind `secrets-keyring` (off by default, because headless systemd boxes often lack a Secret Service daemon) and selected at daemon startup with `secrets.backend = "keychain"` (config section `[secrets]`, env override `MIMIR_SECRETS_BACKEND`; see `docs/config-system.md`).

- **Entry mapping.** Each connector slug is one keyring entry: service `mimir`, account `<slug>`, payload the serialized `SecretBundle` JSON — the same bytes the file store writes, so bundles move losslessly between backends.
- **No silent downgrade.** A `keychain` backend requested in a build without the `secrets-keyring` cargo feature aborts daemon startup with an actionable error. The user explicitly chose the OS store; falling back to plaintext files would violate that security decision.
- **Availability is surfaced, not probed.** Construction is side-effect free; the first operation connects to the platform store. A headless box without a Secret Service daemon fails `load`/`store`/`delete` with `SecretError::Keyring` (wrapping the `keyring` error, e.g. `NoDefaultStore` / `NoStorageAccess`), which the connector auth paths already surface.
- **NoEntry semantics.** A missing entry is `Ok(None)` on `load` and `Ok(())` on `delete`, matching the `SecretStore` contract (`load` returns `Ok(None)` for unauthenticated connectors; `delete` is idempotent).
- **Same slug validation** as the file store (`[A-Za-z0-9_-]{1,128}`) runs before any keyring call, so a hostile slug can never be used as a keyring account name.
- **Platform notes.** On Linux the Secret Service backend is the pure-Rust zbus stack with `async-io` (keyring's `tokio` feature is intentionally not enabled — zbus's `block_on` then panics inside a tokio runtime). Building the feature needs no C libraries; the `keyring` crate is pinned to 3.6.3 because 4.x requires Rust 1.88, above the workspace MSRV 1.85 (same cap as `icalendar`).

## Security model and explicit deferrals

- **Plaintext at rest** is deliberate, consistent with the existing plaintext LLM API key in `config.toml` and the home-directory trust boundary. At-rest encryption (`argon2` + `chacha20poly1305`) is deferred (Phase 3 §7, out-of-scope). The earlier note in `VISION/03-Connectors/Technical-Design.md` saying tokens are "stored encrypted at rest" is **outdated**; the locked Phase-3 plan is the source of truth and was corrected in this change set.
- **OS keyring** backend is implemented (#188): `KeyringSecretStore`, feature-gated `secrets-keyring` (off by default — headless systemd boxes often lack a Secret Service daemon), selected via `secrets.backend = "keychain"`.
- **Non-Unix targets:** file-mode enforcement is skipped (no portable mode concept). V1 targets Linux primarily; this is a documented limitation, not a hole — the store still refuses to deserialize corrupt/unknown bundles.
- **Redacted `Debug`:** `SecretBundle` implements `std::fmt::Debug` manually so the variant discriminant (and non-secret fields like `expires_at`) print while the secret values (`access_token`, `refresh_token`, `token`, `password`) are replaced with `"<redacted>"`. This keeps `Debug`-formatting a `SecretStore` (via `ConnectorContext`), a `tracing` field, or a persisted error string from ever emitting plaintext credentials — the derived `Debug` would otherwise leak them through the `InMemorySecretStore` map.

## Design notes

- **Struct variants, not newtypes.** `SecretBundle` uses `ApiToken { token }` / `AppPassword { password }` rather than `ApiToken(String)` because serde's internally-tagged `kind` representation requires map-typed variant payloads; the named fields also make the on-disk JSON self-describing.
- **`OAuth` `Option` fields.** `refresh_token` and `expires_at` are `Option` since not all grants issue a refresh token or return an expiry (e.g. client-credentials, some OIDC providers).
- **Async trait.** `SecretStore` is `#[async_trait]`, which let the OS-keychain backend (#188) implement the trait without a breaking change, and composes with the async `Connector` pipeline. The file and keyring backends do blocking I/O, which is fast for tiny JSON payloads.
- **Shared mismatch error.** The Calendar and Email connectors build the `auth method X does not match stored secret kind` error through the shared `crate::secrets::mismatch_error` helper (issue #273), so the message text and the auth-kind `discriminant()` stay in sync across both backends and are pinned by unit tests. The `discriminant()` contract is shared via the `crate::secrets::AuthMethodDiscriminant` trait (issue #341), while the concrete `match` expressions remain connector-specific; each variant's mapping is pinned against its serde `kind` tag by unit tests.

## End-to-end secret wipe

The `connector remove` flow (server `DELETE /connectors/:id` + CLI `mimir connector remove`, Epic 4 issues #202/#204/#203) calls `SecretStore::delete` on removal. F10 delivers the `delete(slug)` capability and its tests; the end-to-end "remove wipes the secret file" acceptance is verified when Epic 4 lands.

## Verification

```bash
cargo test -p mimir-connectors --test secrets_store          # round-trip + perm + slug
cargo test -p mimir-connectors --features secrets-keyring    # full crate + keyring store tests
cargo test -p mimir-server --lib state::tests                # daemon wiring (fail-loud arm)
cargo build -p mimir-connectors --no-default-features        # framework + secrets ungated
cargo check -p mimir-server --features secrets-keyring       # feature-on daemon build
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

## System connections

- **`mimir-core::paths`** — `secrets_dir` / `secrets_file` resolve the root.
- **`mimir-connectors::Connector`** — connectors consume credentials injected at construction by their factory (F7), sourced from this store.
- **`mimir-connectors::ConnectorSupervisor`** — will read auth state from the store to drive `authenticate` and the auth-expiry pause path.
- **`mimir-server` / CLI (Epic 4, #202/#204/#203)** — owns a `SecretStore`, exposes token-ingest routes, and calls `delete` on connector removal.
