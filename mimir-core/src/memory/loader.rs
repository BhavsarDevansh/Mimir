use std::path::Path;

use anyhow::{Context, Result};
use tokio::fs;

use crate::paths;

/// Utility for loading `memory.md` from disk.
pub struct MemoryLoader;

impl MemoryLoader {
    /// Initialise memory.md at the default platform path.
    ///
    /// Creates the config directory and writes the default template if the file
    /// does not already exist. Returns `true` if the file was created, `false`
    /// if it already existed.
    pub async fn init() -> Result<bool> {
        let path = paths::memory_path()
            .context("Cannot initialise memory.md: unable to resolve platform path")?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                let default = Self::default_memory();
                tokio::io::AsyncWriteExt::write_all(&mut file, default.as_bytes()).await?;
                tracing::info!("Created default memory.md at {}", path.display());
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Load memory.md from `path`.
    ///
    /// If the file does not exist, creates the parent directories, writes the
    /// default template, and returns it.
    pub async fn load(path: &Path) -> Result<String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
        {
            Ok(mut file) => {
                let default = Self::default_memory();
                tokio::io::AsyncWriteExt::write_all(&mut file, default.as_bytes()).await?;
                Ok(default)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(fs::read_to_string(path).await?)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Return the default memory.md template.
    pub fn default_memory() -> String {
        r#"Mimir memory [0/2500]

No memories yet."#
        .to_string()
    }

    /// Return the platform-specific path for memory.md.
    pub fn get_memory_path() -> std::path::PathBuf {
        paths::memory_path().unwrap_or_else(|_| std::path::PathBuf::from("memory.md"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_loader_reads_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Hello world").unwrap();

        let content = MemoryLoader::load(&path).await.unwrap();
        assert_eq!(content, "Hello world");
    }

    #[tokio::test]
    async fn test_loader_creates_default_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");

        assert!(!path.exists());
        let content = MemoryLoader::load(&path).await.unwrap();

        assert!(path.exists());
        assert!(content.contains("Mimir memory ["));
        assert!(content.contains("No memories yet."));
    }

    #[tokio::test]
    async fn test_loader_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("dirs").join("memory.md");

        assert!(!path.parent().unwrap().exists());
        let _ = MemoryLoader::load(&path).await.unwrap();

        assert!(path.parent().unwrap().exists());
    }

    #[test]
    fn test_default_memory_has_all_sections() {
        let default = MemoryLoader::default_memory();
        assert!(default.contains("…"));
    }
}
