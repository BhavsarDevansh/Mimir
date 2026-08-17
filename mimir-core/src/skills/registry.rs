use super::SkillMetricsDb;
use super::{Skill, SkillContext, SkillError, SkillInput, SkillOutput, SkillSource};
use crate::tools::ToolPermission;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// Metadata for a registered skill.
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub permission: ToolPermission,
    pub version: String,
    pub tags: Vec<String>,
    /// Disk path for user-added skills, if known.
    pub source_path: Option<std::path::PathBuf>,
}

/// Entry in the registry combining the skill implementation with metadata.
pub struct SkillEntry {
    pub skill: Arc<dyn Skill>,
    pub metadata: SkillMetadata,
}

/// Dynamic registry for skill discovery, registration, and invocation.
pub struct SkillRegistry {
    entries: RwLock<HashMap<String, SkillEntry>>,
    metrics_db: RwLock<Option<Arc<SkillMetricsDb>>>,
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillRegistry {
    /// Attach a metrics database to record invocations automatically.
    pub fn set_metrics_db(&self, db: Arc<SkillMetricsDb>) {
        *self.metrics_db.write().unwrap() = Some(db);
    }
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            metrics_db: RwLock::new(None),
        }
    }

    /// Create a registry with all built-in skills registered.
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        registry.register_builtins();
        registry
    }

    /// Register native built-in skills.
    pub fn register_builtins(&self) {
        let _ = self.register_builtin(Arc::new(super::ResearchSynthesisSkill));
        let _ = self.register_builtin(Arc::new(super::TestDrivenDevelopmentSkill));
    }

    /// Register a built-in skill, respecting its declared default permission.
    pub fn register_builtin(&self, skill: Arc<dyn Skill>) -> Result<(), SkillError> {
        {
            let perm = skill.permission();
            self.register(skill, SkillSource::Builtin, perm)
        }
    }

    /// Register a user-added skill, respecting its declared default permission.
    pub fn register_user(&self, skill: Arc<dyn Skill>) -> Result<(), SkillError> {
        {
            let perm = skill.permission();
            self.register(skill, SkillSource::User, perm)
        }
    }

    /// Register a system-generated skill, respecting its declared default permission.
    pub fn register_generated(&self, skill: Arc<dyn Skill>) -> Result<(), SkillError> {
        {
            let perm = skill.permission();
            self.register(skill, SkillSource::Generated, perm)
        }
    }

    /// Register a skill with default metadata.
    ///
    /// Version defaults to "1.0.0" and tags default to empty.
    /// For user-loaded skills, prefer [`Self::register_with_metadata`].
    pub fn register(
        &self,
        skill: Arc<dyn Skill>,
        source: SkillSource,
        permission: ToolPermission,
    ) -> Result<(), SkillError> {
        let name = skill.name().to_string();
        let metadata = SkillMetadata {
            name: name.clone(),
            description: skill.description().to_string(),
            source,
            permission,
            version: "1.0.0".to_string(),
            tags: Vec::new(),
            source_path: None,
        };
        self.register_with_metadata(skill, metadata)
    }

    /// Register a skill with explicit metadata (used by MarkdownSkill loader).
    ///
    /// # Panics
    ///
    /// Panics if `metadata.name` does not match `skill.name()`.
    pub fn register_with_metadata(
        &self,
        skill: Arc<dyn Skill>,
        metadata: SkillMetadata,
    ) -> Result<(), SkillError> {
        let skill_name = skill.name();
        assert_eq!(
            metadata.name, skill_name,
            "metadata.name ({}) must match skill.name() ({})",
            metadata.name, skill_name
        );
        let mut entries = self.entries.write().unwrap();
        if entries.contains_key(skill_name) {
            return Err(SkillError::already_registered(skill_name));
        }
        entries.insert(skill_name.to_string(), SkillEntry { skill, metadata });
        Ok(())
    }

    /// Retrieve a skill by name.
    pub fn get(&self, name: &str) -> Option<(Arc<dyn Skill>, SkillMetadata)> {
        let entries = self.entries.read().unwrap();
        entries
            .get(name)
            .map(|entry| (Arc::clone(&entry.skill), entry.metadata.clone()))
    }

    /// Retrieve metadata for a skill by name.
    pub fn metadata(&self, name: &str) -> Option<SkillMetadata> {
        let entries = self.entries.read().unwrap();
        entries.get(name).map(|entry| entry.metadata.clone())
    }

    /// Set the permission for a skill.
    pub fn set_permission(&self, name: &str, permission: ToolPermission) -> Result<(), SkillError> {
        let mut entries = self.entries.write().unwrap();
        let entry = entries
            .get_mut(name)
            .ok_or_else(|| SkillError::not_found(name))?;
        entry.metadata.permission = permission;
        Ok(())
    }

    /// List all registered skills with metadata.
    pub fn list(&self) -> Vec<SkillMetadata> {
        let entries = self.entries.read().unwrap();
        entries.values().map(|e| e.metadata.clone()).collect()
    }

    /// List skills filtered by origin.
    pub fn list_by_source(&self, source: SkillSource) -> Vec<SkillMetadata> {
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|e| e.metadata.source == source)
            .map(|e| e.metadata.clone())
            .collect()
    }

    /// List skills filtered by tag.
    pub fn list_by_tag(&self, tag: &str) -> Vec<SkillMetadata> {
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|e| e.metadata.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
            .map(|e| e.metadata.clone())
            .collect()
    }

    /// Delete a user skill. Built-in and generated skills are protected.
    pub fn delete(&self, name: &str) -> Result<(), SkillError> {
        let mut entries = self.entries.write().unwrap();
        let entry = entries
            .get(name)
            .ok_or_else(|| SkillError::not_found(name))?;
        match entry.metadata.source {
            SkillSource::Builtin | SkillSource::Generated => {
                return Err(SkillError::protected(name));
            }
            SkillSource::User => {}
        }
        entries.remove(name);
        Ok(())
    }

    /// Export all skills in OpenAI-compatible function-calling format.
    /// Disabled skills are skipped so the model does not see them.
    pub fn export_openai_tools(&self) -> Vec<Value> {
        let entries = self.entries.read().unwrap();
        entries
            .values()
            .filter(|entry| entry.metadata.permission != ToolPermission::Disabled)
            .map(|entry| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": entry.metadata.name,
                        "description": entry.metadata.description,
                        "parameters": entry.skill.parameters_schema(),
                        "strict": true,
                    }
                })
            })
            .collect()
    }

    /// Execute a skill by name with the given context and JSON arguments.
    ///
    /// If a [`SkillMetricsDb`] has been attached via [`Self::set_metrics_db`],
    /// invocation latency and success/failure are recorded automatically.
    pub async fn execute(
        &self,
        name: &str,
        ctx: SkillContext,
        input: SkillInput,
    ) -> Result<SkillOutput, SkillError> {
        let (skill, metadata) = self.get(name).ok_or_else(|| SkillError::not_found(name))?;

        match metadata.permission {
            ToolPermission::Disabled => return Err(SkillError::disabled(name)),
            ToolPermission::Ask => return Err(SkillError::permission_denied(name)),
            ToolPermission::Auto => {}
        }

        let start = std::time::Instant::now();
        let result = skill.execute(ctx, input).await;
        let latency_ms = start.elapsed().as_millis() as u64;
        let success = result.is_ok();

        let metrics_db = self.metrics_db.read().unwrap().clone();
        if let Some(ref db) = metrics_db {
            // Fire-and-forget metrics recording; log on failure but don't fail the skill.
            if let Err(e) = db.record_invocation(name, success, latency_ms, None).await {
                warn!(skill = %name, error = %e, "failed to record skill metrics");
            }
        }

        result
    }

    /// Load user skills from a directory of Markdown files.
    ///
    /// Performs asynchronous file I/O so the daemon can load skills without
    /// blocking the runtime. Returns the number of skills registered.
    pub async fn load_user_skills(&self, dir: &Path) -> Result<usize, SkillError> {
        if !tokio::fs::try_exists(dir).await.map_err(load_io_error)? {
            return Ok(0);
        }

        let mut count = 0;
        let mut entries = tokio::fs::read_dir(dir).await.map_err(load_io_error)?;
        while let Some(entry) = entries.next_entry().await.map_err(load_io_error)? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Ok(contents) = tokio::fs::read_to_string(&path).await else {
                warn!(path = %path.display(), "failed to read skill file");
                continue;
            };
            let mut def = match super::markdown::parse_skill_file(&contents) {
                Ok(d) => d,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to parse skill file");
                    continue;
                }
            };
            def.source_path = Some(path.clone());
            let skill = match super::markdown::MarkdownSkill::from_definition(def) {
                Ok(s) => s,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to build markdown skill");
                    continue;
                }
            };
            let name = skill.name().to_string();
            let metadata =
                super::markdown::build_metadata(&skill, &skill.version, &skill.tags, Some(&path));
            if let Err(e) = self.register_with_metadata(Arc::new(skill), metadata) {
                warn!(skill = %name, error = %e, "failed to register user skill");
            } else {
                info!(skill = %name, path = %path.display(), "registered user skill");
                count += 1;
            }
        }
        Ok(count)
    }
}

fn load_io_error(e: std::io::Error) -> SkillError {
    SkillError::execution_failed("load_user_skills", e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillContext, SkillError, SkillInput, SkillOutput};
    use crate::tools::ToolPermission;
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;

    struct DummySkill;

    #[async_trait]
    impl Skill for DummySkill {
        fn name(&self) -> &str {
            "dummy"
        }
        fn description(&self) -> &str {
            "A dummy skill for testing."
        }
        fn parameters_schema(&self) -> Value {
            serde_json::json!({"type": "object", "properties": {}, "required": []})
        }
        fn permission(&self) -> ToolPermission {
            ToolPermission::Auto
        }
        async fn execute(
            &self,
            _ctx: SkillContext,
            _input: SkillInput,
        ) -> Result<SkillOutput, SkillError> {
            Ok(SkillOutput {
                result: Some(Value::String("ok".to_string())),
                ..Default::default()
            })
        }
    }

    #[test]
    fn register_and_get_skill() {
        let registry = SkillRegistry::new();
        let skill = Arc::new(DummySkill);
        registry.register_builtin(skill.clone()).unwrap();

        let (got, meta) = registry.get("dummy").unwrap();
        assert_eq!(got.name(), "dummy");
        assert_eq!(meta.name, "dummy");
        assert_eq!(meta.source, SkillSource::Builtin);
    }

    #[test]
    fn duplicate_registration_fails() {
        let registry = SkillRegistry::new();
        let skill = Arc::new(DummySkill);
        registry.register_builtin(skill.clone()).unwrap();
        let result = registry.register_builtin(skill.clone());
        assert!(matches!(result, Err(SkillError::AlreadyRegistered(_))));
    }

    #[test]
    fn list_skills() {
        let registry = SkillRegistry::new();
        registry.register_builtin(Arc::new(DummySkill)).unwrap();
        let skills = registry.list();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "dummy");
    }

    #[test]
    fn set_permission() {
        let registry = SkillRegistry::new();
        registry.register_builtin(Arc::new(DummySkill)).unwrap();
        registry
            .set_permission("dummy", ToolPermission::Disabled)
            .unwrap();
        let meta = registry.metadata("dummy").unwrap();
        assert_eq!(meta.permission, ToolPermission::Disabled);
    }

    #[test]
    fn delete_user_skill() {
        let registry = SkillRegistry::new();
        registry.register_user(Arc::new(DummySkill)).unwrap();
        registry.delete("dummy").unwrap();
        assert!(registry.get("dummy").is_none());
    }

    #[test]
    fn delete_builtin_skill_fails() {
        let registry = SkillRegistry::new();
        registry.register_builtin(Arc::new(DummySkill)).unwrap();
        let result = registry.delete("dummy");
        assert!(matches!(result, Err(SkillError::Protected(_))));
    }

    #[test]
    fn export_openai_tools_skips_disabled() {
        let registry = SkillRegistry::new();
        registry.register_builtin(Arc::new(DummySkill)).unwrap();
        let exported = registry.export_openai_tools();
        assert_eq!(exported.len(), 1);

        registry
            .set_permission("dummy", ToolPermission::Disabled)
            .unwrap();
        let exported = registry.export_openai_tools();
        assert!(exported.is_empty());
    }

    #[test]
    fn export_openai_tools_format() {
        let registry = SkillRegistry::new();
        registry.register_builtin(Arc::new(DummySkill)).unwrap();
        let exported = registry.export_openai_tools();
        let obj = exported[0].as_object().unwrap();
        assert_eq!(obj["type"], "function");
        let func = obj["function"].as_object().unwrap();
        assert_eq!(func["name"], "dummy");
        assert_eq!(func["strict"], true);
    }
}
