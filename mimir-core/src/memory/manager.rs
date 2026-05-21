use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::fs;

use super::loader::MemoryLoader;

/// A frozen snapshot of memory.md taken at a point in time.
///
/// Changes to the underlying [`MemoryManager`] do not affect a snapshot
/// once it has been created. This preserves LLM prefix-cache performance
/// by keeping the system prompt stable for the duration of a session.
#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    content: String,
}

impl MemorySnapshot {
    /// Return the snapshot content.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// Manages the live contents of `memory.md`.
///
/// Provides CRUD operations, capacity tracking, and frozen snapshotting.
/// All mutating methods write to disk immediately.
pub struct MemoryManager {
    char_limit: u16,
    content: String,
    path: PathBuf,
}

impl MemoryManager {
    /// Load existing memory.md from `path` or create it with the default
    /// template if missing.
    pub async fn new(path: &Path, char_limit: u16) -> Result<Self> {
        if char_limit == 0 {
            anyhow::bail!("char_limit must be non-zero");
        }
        let content = MemoryLoader::load(path).await?;
        Ok(Self {
            char_limit,
            content,
            path: path.to_path_buf(),
        })
    }

    /// Return the current live content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Number of Unicode scalar points currently stored.
    pub fn current_chars(&self) -> usize {
        self.content.chars().count()
    }

    /// Remaining capacity in characters.
    pub fn remaining_chars(&self) -> usize {
        (self.char_limit as usize).saturating_sub(self.current_chars())
    }

    /// Whether the memory is at or over capacity.
    pub fn is_full(&self) -> bool {
        self.current_chars() >= self.char_limit as usize
    }

    /// Usage percentage (0.0–100.0).
    pub fn usage_pct(&self) -> f32 {
        (self.current_chars() as f32 / self.char_limit as f32) * 100.0
    }

    /// Append an entry to the live memory and persist to disk.
    ///
    /// Fails if the addition would exceed the character limit.
    pub async fn add(&mut self, entry: &str) -> Result<()> {
        let entry_chars = entry.chars().count();
        let separator_chars = if !self.content.is_empty() && !self.content.ends_with('\n') {
            1
        } else {
            0
        };
        if self.current_chars() + separator_chars + entry_chars > self.char_limit as usize {
            anyhow::bail!(
                "Memory full: {}/{} chars. Cannot add {} chars.",
                self.current_chars(),
                self.char_limit,
                entry_chars
            );
        }

        if separator_chars == 1 {
            self.content.push('\n');
        }
        self.content.push_str(entry);
        self.save().await?;
        Ok(())
    }

    /// Replace the first (and only) occurrence of `old_text` with `new_text`.
    ///
    /// Fails if the text is not found or occurs more than once, or if the
    /// replacement would exceed the character limit.
    pub async fn replace(&mut self, old_text: &str, new_text: &str) -> Result<()> {
        self.assert_unique_match(old_text)?;

        let old_chars = old_text.chars().count();
        let new_chars = new_text.chars().count();
        let size_delta = new_chars as i64 - old_chars as i64;
        let new_total = self.current_chars() as i64 + size_delta;

        if new_total > self.char_limit as i64 {
            anyhow::bail!(
                "Replace would exceed memory limit: {}/{} chars",
                new_total,
                self.char_limit
            );
        }

        self.content = self.content.replacen(old_text, new_text, 1);
        self.save().await?;
        Ok(())
    }

    /// Remove the first (and only) occurrence of `old_text`.
    ///
    /// Fails if the text is not found or occurs more than once.
    pub async fn remove(&mut self, old_text: &str) -> Result<()> {
        self.assert_unique_match(old_text)?;

        self.content = self.content.replacen(old_text, "", 1);
        self.save().await?;
        Ok(())
    }

    /// Take a frozen snapshot of the current content.
    ///
    /// The returned snapshot is independent of future mutations.
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            content: self.content.clone(),
        }
    }

    /// Reload content from disk.
    ///
    /// This is useful when starting a new session after the file may have
    /// been edited externally.
    pub async fn refresh(&mut self) -> Result<()> {
        self.content = fs::read_to_string(&self.path).await?;
        Ok(())
    }

    /// Persist the current content to disk.
    pub async fn save(&self) -> Result<()> {
        fs::write(&self.path, &self.content).await?;
        Ok(())
    }

    /// Ensure `text` occurs exactly once in the content.
    fn assert_unique_match(&self, text: &str) -> Result<()> {
        let count = self.content.matches(text).count();
        if count == 0 {
            anyhow::bail!("Text '{}' not found in memory", text);
        }
        if count > 1 {
            anyhow::bail!(
                "Text '{}' matches {} entries. Be more specific.",
                text,
                count
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_manager_loads_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Existing content").unwrap();

        let manager = MemoryManager::new(&path, 100).await.unwrap();
        assert_eq!(manager.content(), "Existing content");
        assert_eq!(manager.current_chars(), 16);
    }

    #[tokio::test]
    async fn test_manager_creates_default_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");

        let manager = MemoryManager::new(&path, 2500).await.unwrap();
        assert!(manager.content().contains("Mimir Working Memory"));
        assert!(std::fs::metadata(&path).is_ok());
    }

    #[tokio::test]
    async fn test_manager_add() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Base").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        manager.add("User: Devansh").await.unwrap();

        assert!(manager.content().contains("Base"));
        assert!(manager.content().contains("User: Devansh"));
        assert!(manager.content().contains("Base\nUser: Devansh"));
    }

    #[tokio::test]
    async fn test_manager_add_exceeds_limit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "12345").unwrap();

        let mut manager = MemoryManager::new(&path, 10).await.unwrap();
        let result = manager.add("67890").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Memory full"));
    }

    #[tokio::test]
    async fn test_manager_replace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "User: Dev\n").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        manager.replace("Dev", "Devansh").await.unwrap();

        assert_eq!(manager.content(), "User: Devansh\n");
    }

    #[tokio::test]
    async fn test_manager_replace_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Hello").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        let result = manager.replace("Missing", "Nope").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_manager_replace_ambiguous() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "cat cat").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        let result = manager.replace("cat", "dog").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("matches 2 entries")
        );
    }

    #[tokio::test]
    async fn test_manager_replace_would_exceed_limit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "ABCDE").unwrap();

        let mut manager = MemoryManager::new(&path, 10).await.unwrap();
        let result = manager.replace("ABCDE", "ABCDEFGHIJK").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceed memory limit")
        );
    }

    #[tokio::test]
    async fn test_manager_remove() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "A\nB\nC").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        manager.remove("B\n").await.unwrap();

        assert_eq!(manager.content(), "A\nC");
    }

    #[tokio::test]
    async fn test_manager_remove_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Hello").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        let result = manager.remove("Missing").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_manager_remove_ambiguous() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "dup dup").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        let result = manager.remove("dup").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("matches 2 entries")
        );
    }

    #[tokio::test]
    async fn test_snapshot_is_frozen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Original").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        let snapshot = manager.snapshot();

        manager.add(" Modified").await.unwrap();

        assert_eq!(snapshot.content(), "Original");
        assert!(manager.content().contains("Modified"));
    }

    #[tokio::test]
    async fn test_usage_tracking() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "ABCDE").unwrap();

        let mut manager = MemoryManager::new(&path, 20).await.unwrap();
        assert_eq!(manager.current_chars(), 5);
        assert_eq!(manager.remaining_chars(), 15);
        assert!((manager.usage_pct() - 25.0).abs() < f32::EPSILON);
        assert!(!manager.is_full());

        manager.add("FGHIJ").await.unwrap();
        assert_eq!(manager.current_chars(), 11); // "ABCDE\nFGHIJ"
        assert_eq!(manager.remaining_chars(), 9);

        manager.add("KLMNO").await.unwrap();
        assert_eq!(manager.current_chars(), 17); // 11 + 1 + 5
        assert!(!manager.is_full());

        manager.add("PQ").await.unwrap();
        assert_eq!(manager.current_chars(), 20); // 17 + 1 + 2
        assert_eq!(manager.remaining_chars(), 0);
        assert!((manager.usage_pct() - 100.0).abs() < f32::EPSILON);
        assert!(manager.is_full());

        let result = manager.add("X").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_refresh_reloads_from_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Old").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        std::fs::write(&path, "New").unwrap();
        manager.refresh().await.unwrap();

        assert_eq!(manager.content(), "New");
    }

    #[tokio::test]
    async fn test_save_persists_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "Initial").unwrap();

        let mut manager = MemoryManager::new(&path, 100).await.unwrap();
        manager.content = "Changed".to_string();
        manager.save().await.unwrap();

        let disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(disk, "Changed");
    }
}
