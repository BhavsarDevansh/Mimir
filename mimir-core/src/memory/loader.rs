use std::path::Path;

use anyhow::Result;
use tokio::fs;

/// Utility for loading `memory.md` from disk.
pub struct MemoryLoader;

impl MemoryLoader {
    /// Load memory.md from `path`.
    ///
    /// If the file does not exist, creates the parent directories, writes the
    /// default template, and returns it.
    pub async fn load(path: &Path) -> Result<String> {
        if path.exists() {
            Ok(fs::read_to_string(path).await?)
        } else {
            let default = Self::default_memory();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(path, &default).await?;
            Ok(default)
        }
    }

    /// Return the default memory.md template.
    pub fn default_memory() -> String {
        r#"═══════════════════════════════════════════════════════════
MEMORY [0 / 2,500 chars] — Mimir Working Memory
═══════════════════════════════════════════════════════════

User: (not yet configured)
Location: (not yet configured)

Active Projects: (none)
Preferences: (none)
Temporal: (none)
KB Pointers: (none)
═══════════════════════════════════════════════════════════"#
            .to_string()
    }

    /// Return the platform-specific path for memory.md.
    pub fn get_memory_path() -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("mimir")
            .join("memory.md")
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
        assert!(content.contains("Mimir Working Memory"));
        assert!(content.contains("User: (not yet configured)"));
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
        assert!(default.contains("Active Projects"));
        assert!(default.contains("Preferences"));
        assert!(default.contains("Temporal"));
        assert!(default.contains("KB Pointers"));
    }
}
