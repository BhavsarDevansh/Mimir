use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::warn;

use crate::config::PersonalityConfig;
use crate::paths;

/// The personality engine: resolves the active preset and composes system prompts.
#[derive(Debug, Clone, PartialEq)]
pub struct Personality {
    active_name: String,
    registry: HashMap<String, String>,
}

impl Personality {
    /// Create a `Personality` from the supplied config, scanning the default
    /// user personalities directory (`~/.config/mimir/personalities/`).
    pub fn new(config: &PersonalityConfig) -> Self {
        let presets_dir = match paths::personalities_dir() {
            Ok(dir) => dir,
            Err(e) => {
                warn!(error = %e, "failed to resolve personalities directory; custom personalities will not be loaded");
                // Return early with only built-in presets when path resolution fails
                return Self {
                    active_name: if ["transparent", "concise", "warm", "formal"].contains(&config.preset.as_str()) {
                        config.preset.clone()
                    } else {
                        warn!(
                            preset = %config.preset,
                            "unknown personality preset; falling back to 'transparent'"
                        );
                        "transparent".to_string()
                    },
                    registry: Self::built_in_presets(),
                };
            }
        };
        Self::from_path(&presets_dir, &config.preset)
    }

    /// Create a `Personality` using a custom presets directory and preset name.
    /// Useful in tests.
    pub fn from_path(presets_dir: &Path, preset_name: &str) -> Self {
        let mut registry = Self::built_in_presets();
        let custom = Self::scan_custom_presets(presets_dir);

        // Custom overrides built-in.
        for (name, prompt) in custom {
            registry.insert(name, prompt);
        }

        let active_name = if registry.contains_key(preset_name) {
            preset_name.to_string()
        } else {
            warn!(
                preset = %preset_name,
                "unknown personality preset; falling back to 'transparent'"
            );
            "transparent".to_string()
        };

        Self {
            active_name,
            registry,
        }
    }

    /// Return the system prompt for the active preset, optionally composed
    /// with persistent memory context.
    pub fn system_prompt(&self, memory_content: &str) -> String {
        let preset_prompt = self
            .registry
            .get(&self.active_name)
            .cloned()
            .unwrap_or_else(Self::built_in_transparent);

        if memory_content.trim().is_empty() {
            preset_prompt
        } else {
            format!(
                "{}\n\nKey facts I know about you:\n{}\n\nNote: This is not an exhaustive list. Use kg_query, kg_related, or kg_search tools if you need more information. Use the remember tool whenever the user shares something worth saving.",
                preset_prompt,
                memory_content.trim()
            )
        }
    }

    /// List all available preset names (built-in + custom), sorted alphabetically.
    pub fn list_presets(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.registry.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }

    /// Return the name of the active preset.
    pub fn active_name(&self) -> &str {
        &self.active_name
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    fn built_in_presets() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("transparent".to_string(), Self::built_in_transparent());
        m.insert("concise".to_string(), Self::built_in_concise());
        m.insert("warm".to_string(), Self::built_in_warm());
        m.insert("formal".to_string(), Self::built_in_formal());
        m
    }

    fn built_in_transparent() -> String {
        concat!(
            "You are Mimir, a personal intelligence assistant. ",
            "You are warm, efficient, and transparent. ",
            "You show your work when making suggestions, but keep it brief unless asked for detail. ",
            "You admit uncertainty clearly and never state inference as fact. ",
            "You respect the user's pace and never rush them into granting permissions. ",
            "You speak as a collaborator, not a servant. ",
            "Avoid excessive deference, apologies, or performative humility."
        )
        .to_string()
    }

    fn built_in_concise() -> String {
        concat!(
            "You are Mimir, a personal intelligence assistant. ",
            "Use minimal words and maximum information density. ",
            "Prefer bullet points over paragraphs. ",
            "Do not show reasoning unless explicitly asked. ",
            "Be direct and avoid filler. "
        )
        .to_string()
    }

    fn built_in_warm() -> String {
        concat!(
            "You are Mimir, a personal intelligence assistant. ",
            "You are conversational and companion-like. ",
            "Acknowledge context and effort naturally. ",
            "Use the user's name when you know it. ",
            "Be supportive without being overly familiar. "
        )
        .to_string()
    }

    fn built_in_formal() -> String {
        concat!(
            "You are Mimir, a personal intelligence assistant. ",
            "Use neutral, structured language. ",
            "Write full sentences with no contractions. ",
            "Use precise terminology. ",
            "Maintain professional distance and clarity."
        )
        .to_string()
    }

    fn scan_custom_presets(presets_dir: &Path) -> Vec<(String, String)> {
        let mut results = Vec::new();
        if !presets_dir.exists() {
            return results;
        }

        let entries = match std::fs::read_dir(presets_dir) {
            Ok(e) => e,
            Err(_) => return results,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext != "md" {
                    continue;
                }
            } else {
                continue;
            }

            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                // Only files ending in .personality.md
                if let Some(prefix) = stem.strip_suffix(".personality")
                    && let Ok(content) = std::fs::read_to_string(&path)
                {
                    results.push((prefix.to_string(), content));
                }
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_built_in_transparent_prompt_non_empty() {
        let p = Personality::from_path(Path::new("/nonexistent"), "transparent");
        let prompt = p.system_prompt("");
        assert!(!prompt.is_empty());
        assert!(prompt.contains("transparent"));
        assert!(prompt.contains("collaborator"));
    }

    #[test]
    fn test_built_in_concise_prompt_non_empty() {
        let p = Personality::from_path(Path::new("/nonexistent"), "concise");
        let prompt = p.system_prompt("");
        assert!(!prompt.is_empty());
        assert!(prompt.contains("bullet points"));
    }

    #[test]
    fn test_built_in_warm_prompt_non_empty() {
        let p = Personality::from_path(Path::new("/nonexistent"), "warm");
        let prompt = p.system_prompt("");
        assert!(!prompt.is_empty());
        assert!(prompt.contains("companion"));
    }

    #[test]
    fn test_built_in_formal_prompt_non_empty() {
        let p = Personality::from_path(Path::new("/nonexistent"), "formal");
        let prompt = p.system_prompt("");
        assert!(!prompt.is_empty());
        assert!(prompt.contains("precise"));
    }

    #[test]
    fn test_unknown_preset_falls_back_to_transparent() {
        let p = Personality::from_path(Path::new("/nonexistent"), "nonexistent");
        assert_eq!(p.active_name(), "transparent");
        let prompt = p.system_prompt("");
        assert!(prompt.contains("transparent"));
    }

    #[test]
    fn test_custom_preset_loading() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("cheerful.personality.md");
        fs::write(&file_path, "You are cheerful and upbeat!").unwrap();

        let p = Personality::from_path(dir.path(), "cheerful");
        assert_eq!(p.active_name(), "cheerful");
        assert_eq!(p.system_prompt(""), "You are cheerful and upbeat!");
    }

    #[test]
    fn test_custom_preset_overrides_built_in() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("transparent.personality.md");
        fs::write(&file_path, "Custom transparent override.").unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        assert_eq!(p.system_prompt(""), "Custom transparent override.");
    }

    #[test]
    fn test_system_prompt_with_memory() {
        let p = Personality::from_path(Path::new("/nonexistent"), "transparent");
        let prompt = p.system_prompt("User likes cats.");
        assert!(prompt.starts_with(&Personality::built_in_transparent()));
        assert!(prompt.contains("Key facts I know about you:"));
        assert!(prompt.contains("Note: This is not an exhaustive list."));
        assert!(prompt.contains("User likes cats."));
    }

    #[test]
    fn test_system_prompt_empty_memory_omits_section() {
        let p = Personality::from_path(Path::new("/nonexistent"), "transparent");
        let prompt = p.system_prompt("");
        assert!(!prompt.contains("Key facts I know about you:"));
        assert_eq!(prompt, Personality::built_in_transparent());
        assert!(!prompt.contains("Note: This is not an exhaustive list."));
    }

    #[test]
    fn test_list_presets_includes_built_ins_and_customs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("cheerful.personality.md"), "Cheerful!").unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let presets = p.list_presets();
        assert!(presets.contains(&"concise"));
        assert!(presets.contains(&"formal"));
        assert!(presets.contains(&"transparent"));
        assert!(presets.contains(&"warm"));
        assert!(presets.contains(&"cheerful"));
    }

    #[test]
    fn test_non_md_files_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("cheerful.personality.txt"), "Not valid.").unwrap();
        fs::write(dir.path().join("other.md"), "Also not valid.").unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let presets = p.list_presets();
        assert!(!presets.contains(&"cheerful"));
        assert!(!presets.contains(&"other"));
    }
}
