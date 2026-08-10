//! V1 default [`SecretStore`]: the on-disk, per-slug JSON file store.
//!
//! This module owns the filesystem details (atomic writes, `0600`/`0700`
//! permissions, TOCTOU-safe reads) behind [`FileSecretStore`]; the store
//! contract and slug rules live in [`super::store`].

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tracing::warn;

use mimir_core::paths;

use super::bundle::SecretBundle;
use super::error::SecretError;
use super::store::{SecretStore, validate_slug};

static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_temp_counter() -> u64 {
    TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// FileSecretStore
// ---------------------------------------------------------------------------

/// V1 default [`SecretStore`]: one JSON file per connector instance.
///
/// Files live under a base directory (default `~/.local/share/mimir/secrets/`,
/// overridable with [`FileSecretStore::with_dir`] for tests), named
/// `<slug>.json`. The directory is created with mode `0700` and each file with
/// mode `0600`; loads refuse to read a file or directory that is more
/// permissive. Writes are atomic (temp file + `rename`) so a crash cannot
/// leave a truncated secret file that silently logs a connector out.
///
/// The store is cheaply [`Clone`]able (it holds only a [`PathBuf`]) and
/// stateless, so multiple connectors can share one cheaply.
#[derive(Debug, Clone)]
pub struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    /// Create a store rooted at the default Mimir secrets directory
    /// (`~/.local/share/mimir/secrets`). The directory is *not* created here;
    /// it is created on first [`Self::store`] (and validated on
    /// [`Self::load`]).
    pub fn new() -> Result<Self, SecretError> {
        Ok(Self {
            dir: paths::secrets_dir()?,
        })
    }

    /// Create a store rooted at an explicit directory (used by tests and, in
    /// future, for configurable secret roots).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The base directory this store reads and writes under.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the secret file for `slug`. Caller is expected to have validated
    /// `slug`; this is kept private to avoid exposing a way to bypass
    /// validation.
    fn secret_path(&self, slug: &str) -> PathBuf {
        self.dir.join(format!("{slug}.json"))
    }

    /// Ensure the base directory exists with mode `0700`.
    ///
    /// `create_dir_all` is idempotent, so this is safe to call every `store`.
    /// The mode is then (re)applied on Unix so a manually-loosened dir is
    /// re-tightened rather than silently leaving secrets readable.
    fn ensure_dir(&self) -> Result<(), SecretError> {
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Atomic write: serialise to a uniquely-named sibling temp file, tighten
    /// it to `0600` *before* `rename`-ing onto the final path.
    ///
    /// The temp file name embeds the process id and a per-process monotonic
    /// counter, so two concurrent `store` calls for the *same* slug never
    /// collide on the same temp file (the supervisor serialises per-connector
    /// already, but the store does not rely on that). If `rename` fails the
    /// temp file is best-effort removed so no stale files linger.
    ///
    /// Permissions are applied to the temp file before the rename so the
    /// secret is never observable at its final path with the (potentially
    /// looser) default mode inherited from the umask. `rename` then lands the
    /// already-restrictive file atomically, so a fresh store and an overwrite
    /// of a previously-loosened file both end up at `0600`.
    fn write_bundle(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError> {
        let path = self.secret_path(slug);
        let bytes = serde_json::to_vec_pretty(bundle)?;

        let tmp = {
            let n = next_temp_counter();
            self.dir
                .join(format!("{slug}.json.tmp.{}.{}", std::process::id(), n))
        };
        std::fs::write(&tmp, &bytes)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }

        let rename_res = std::fs::rename(&tmp, &path);
        if rename_res.is_err() {
            // Best-effort: don't leave the temp file behind on rename failure.
            let _ = std::fs::remove_file(&tmp);
        }
        rename_res?;

        Ok(())
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn load(&self, slug: &str) -> Result<Option<SecretBundle>, SecretError> {
        validate_slug(slug)?;
        let path = self.secret_path(slug);
        // Open the file first, then check permissions on the open handle and
        // read through it. This avoids the TOCTOU race between a path-based
        // `metadata` and a subsequent path-based `read` (an attacker could swap
        // the file between the two calls). `NotFound` means "no secret stored
        // yet"; everything else is read after the permission check.
        let mut file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let meta = file.metadata()?;
        self.assert_permissions(slug, &meta)?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)?;
        let bundle = serde_json::from_slice::<SecretBundle>(&bytes).map_err(|source| {
            SecretError::Corrupt {
                slug: slug.to_string(),
                source,
            }
        })?;
        Ok(Some(bundle))
    }

    async fn store(&self, slug: &str, bundle: &SecretBundle) -> Result<(), SecretError> {
        validate_slug(slug)?;
        self.ensure_dir()?;
        self.write_bundle(slug, bundle)
    }

    async fn delete(&self, slug: &str) -> Result<(), SecretError> {
        validate_slug(slug)?;
        let path = self.secret_path(slug);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl FileSecretStore {
    /// Refuse to read if the secret file or its parent dir has any group/other
    /// permission bits set. The canonical modes are `0600` (file) and `0700`
    /// (dir); we additionally tolerate an owner-exec bit on a directory
    /// (some platforms set it), but never any group/other bits.
    /// Refuse to read if the secret file or its parent dir has any group/other
    /// permission bits set. The canonical modes are `0600` (file) and `0700`
    /// (dir); owner bits (including owner-exec on a dir) are tolerated, but
    /// never any group/other bits. `file_meta` is the already-fetched file
    /// metadata from [`Self::load`] so the file is stat'd only once.
    #[cfg(unix)]
    fn assert_permissions(
        &self,
        slug: &str,
        file_meta: &std::fs::Metadata,
    ) -> Result<(), SecretError> {
        use std::os::unix::fs::PermissionsExt;
        let file_mode = file_meta.permissions().mode() & 0o777;
        if file_mode & 0o077 != 0 {
            warn!(
                slug = slug,
                mode = format!("{file_mode:o}"),
                "refusing to read secret file with group/other permission bits set"
            );
            return Err(SecretError::InsecurePermissions {
                slug: slug.to_string(),
            });
        }
        let dir_mode = std::fs::metadata(&self.dir)?.permissions().mode() & 0o777;
        if dir_mode & 0o077 != 0 {
            warn!(
                dir = %self.dir.display(),
                mode = format!("{dir_mode:o}"),
                "refusing to read secret: parent dir has group/other permission bits set"
            );
            return Err(SecretError::InsecurePermissions {
                slug: slug.to_string(),
            });
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn assert_permissions(
        &self,
        _slug: &str,
        _file_meta: &std::fs::Metadata,
    ) -> Result<(), SecretError> {
        // Non-Unix targets have no portable file-mode concept; V1 targets
        // Linux and skips enforcement here.
        Ok(())
    }
}
