use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::memory::MemoryManager;

use super::super::{Tool, ToolError, ToolOutput, ToolPermission};

/// Tool for updating Mimir's persistent working memory (memory.md).
///
/// Supports adding, replacing, and removing entries. The underlying
/// MemoryManager is lazily initialised on first execution so the tool
/// can be constructed synchronously.
pub struct MemoryTool {
    path: PathBuf,
    char_limit: u16,
    manager: Mutex<Option<MemoryManager>>,
}

impl MemoryTool {
    /// Create a new MemoryTool for the given path and character limit.
    pub fn new(path: PathBuf, char_limit: u16) -> Self {
        Self {
            path,
            char_limit,
            manager: Mutex::new(None),
        }
    }

    /// Return the lazily-initialised MemoryManager.
    async fn manager(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<MemoryManager>>, ToolError> {
        let mut guard = self.manager.lock().await;
        if guard.is_none() {
            let manager = MemoryManager::new(&self.path, self.char_limit)
                .await
                .map_err(|e| ToolError::execution_failed("memory", e.to_string()))?;
            *guard = Some(manager);
        }
        Ok(guard)
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Update Mimir's persistent working memory (memory.md). \
         memory.md is your personal scratchpad — a free-form text file where you \
         record facts about the user and context that should persist across sessions. \
         Write compact, self-contained notes (one thought per line or bullet). Group \
         related facts together, but do not use rigid sections or prefixes. Prefer \
         'replace' to update an existing note. Use 'add' for new observations. Use \
         'remove' to delete stale notes. Be token-conscious: abbreviate, drop filler \
         words, use shorthand. The file has a 2500 character limit."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "replace", "remove"],
                    "description": "The operation to perform. \
                        'replace' is preferred: find old_text and substitute content. Use this to update an existing note and avoid duplication. \
                        'add' appends content to memory. Use for new observations. \
                        'remove' deletes the first occurrence of old_text."
                },
                "content": {
                    "type": "string",
                    "description": "Concise note text. Required for 'add' and 'replace'. One thought per line or bullet. Be token-conscious: abbreviate, drop filler words."
                },
                "old_text": {
                    "type": "string",
                    "description": "Exact current note text to find and replace or remove. Required for 'replace' and 'remove'. Must match the full existing note exactly."
                }
            },
            "required": ["action"],
            "additionalProperties": false,
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Auto
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_arguments("memory", "missing 'action'"))?;

        let mut guard = self.manager().await?;
        let manager = guard
            .as_mut()
            .expect("manager is initialised by self.manager()");

        match action {
            "add" => {
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::invalid_arguments("memory", "missing 'content' for add")
                    })?;
                manager
                    .add(content)
                    .await
                    .map_err(|e| ToolError::execution_failed("memory", e.to_string()))?;
                Ok(ToolOutput {
                    result: Some(Value::String("Added to memory.".to_string())),
                    ..Default::default()
                })
            }
            "replace" => {
                let old_text = args
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::invalid_arguments("memory", "missing 'old_text' for replace")
                    })?;
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::invalid_arguments("memory", "missing 'content' for replace")
                    })?;
                manager
                    .replace(old_text, content)
                    .await
                    .map_err(|e| ToolError::execution_failed("memory", e.to_string()))?;
                Ok(ToolOutput {
                    result: Some(Value::String("Replaced in memory.".to_string())),
                    ..Default::default()
                })
            }
            "remove" => {
                let old_text = args
                    .get("old_text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::invalid_arguments("memory", "missing 'old_text' for remove")
                    })?;
                manager
                    .remove(old_text)
                    .await
                    .map_err(|e| ToolError::execution_failed("memory", e.to_string()))?;
                Ok(ToolOutput {
                    result: Some(Value::String("Removed from memory.".to_string())),
                    ..Default::default()
                })
            }
            _ => Err(ToolError::invalid_arguments(
                "memory",
                format!("unknown action '{}'", action),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_memory_tool_add() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "No memories yet.").unwrap();

        let tool = MemoryTool::new(path.clone(), 2500);
        let result = tool
            .execute(json!({"action": "add", "content": "\nLocation: Berlin"}))
            .await
            .unwrap();

        assert_eq!(result.result, Some(json!("Added to memory.")));
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(disk.contains("Location: Berlin"));
    }

    #[tokio::test]
    async fn test_memory_tool_replace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "No memories yet.").unwrap();

        let tool = MemoryTool::new(path.clone(), 2500);
        let result = tool
            .execute(json!({
                "action": "replace",
                "old_text": "No memories yet.",
                "content": "User is Alice."
            }))
            .await
            .unwrap();

        assert_eq!(result.result, Some(json!("Replaced in memory.")));
        let disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(disk, "User is Alice.");
    }

    #[tokio::test]
    async fn test_memory_tool_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "A\nB\nC").unwrap();

        let tool = MemoryTool::new(path.clone(), 2500);
        let result = tool
            .execute(json!({"action": "remove", "old_text": "B\n"}))
            .await
            .unwrap();

        assert_eq!(result.result, Some(json!("Removed from memory.")));
        let disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(disk, "A\nC");
    }

    #[tokio::test]
    async fn test_memory_tool_add_exceeds_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "ABCDEFGHIJ").unwrap();

        let tool = MemoryTool::new(path.clone(), 15);
        let result = tool
            .execute(json!({"action": "add", "content": "XXX"}))
            .await;

        // 10 chars + 1 newline + 3 chars = 14, fits.
        assert!(result.is_ok());

        let result2 = tool
            .execute(json!({"action": "add", "content": "ZZZZZ"}))
            .await;
        // 14 + 1 + 5 = 20, exceeds 15.
        assert!(result2.is_err());
    }

    #[tokio::test]
    async fn test_memory_tool_missing_action() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "test").unwrap();

        let tool = MemoryTool::new(path, 2500);
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing 'action'"));
    }

    #[tokio::test]
    async fn test_memory_tool_missing_content_for_add() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "test").unwrap();

        let tool = MemoryTool::new(path, 2500);
        let result = tool.execute(json!({"action": "add"})).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing 'content'")
        );
    }

    #[tokio::test]
    async fn test_memory_tool_unknown_action() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.md");
        std::fs::write(&path, "test").unwrap();

        let tool = MemoryTool::new(path, 2500);
        let result = tool.execute(json!({"action": "boom"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown action"));
    }
}
