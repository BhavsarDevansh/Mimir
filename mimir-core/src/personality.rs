use std::collections::HashMap;
use std::path::Path;

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
    /// Operating directives appended to every preset (issue #138). These are
    /// behavioural invariants of Mimir — retrieval, honesty, and learning —
    /// and apply uniformly to built-in and custom personalities. They are kept
    /// out of the per-preset tone text (DRY) and composed in [`system_prompt`].
    const OPERATING_DIRECTIVES: &str = "\
Operating principles:
- Do not invent facts about the user. If you do not know the answer, say so.
- If you need more information, use the `retrieve_context` tool to dispatch a retrieval agent that investigates the knowledge graph and conversation history. If its findings are still not enough, refine the task and dispatch again. Continue until you have a confident answer or have confirmed the information is not in your knowledge base.
- Call the `remember` tool whenever the user states or reveals something worth saving — explicit assertions, corrections, and meaningful casual mentions. Do not call it for pure chitchat or greetings.";

    /// Header for the injected core-facts block (issue #138). The label and
    /// the condensed-subset framing are merged into one line, third person.
    ///
    /// Shared with the Librarian Agent so the background extraction prompt
    /// injects the same core-facts block the core agent uses (DRY, #139).
    pub const CORE_FACTS_HEADER: &str = "\
Core facts about the user (condensed subset — not a complete picture; treat as starting context, not exhaustive):";

    /// Create a `Personality` from the supplied config, scanning the default
    /// user personalities directory (`~/.config/mimir/personalities/`).
    pub fn new(config: &PersonalityConfig) -> Self {
        let presets_dir = match paths::personalities_dir() {
            Ok(dir) => dir,
            Err(e) => {
                warn!(error = %e, "failed to resolve personalities directory; custom personalities will not be loaded");
                // Return early with only built-in presets when path resolution fails
                return Self {
                    active_name: if ["transparent", "concise", "warm", "formal"]
                        .contains(&config.preset.as_str())
                    {
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

    /// Return the system prompt for the active preset, composed with the
    /// shared operating directives and (when present) the core-facts block.
    ///
    /// Composition order is: preset tone text → operating directives →
    /// core-facts block (only when `memory_content` is non-empty). The
    /// directives are always appended so the behavioural contract holds
    /// even when no facts are injected (issue #138).
    pub fn system_prompt(&self, memory_content: &str) -> String {
        let preset_prompt = self
            .registry
            .get(&self.active_name)
            .cloned()
            .unwrap_or_else(Self::built_in_transparent);

        let base = format!("{}\n\n{}", preset_prompt, Self::OPERATING_DIRECTIVES);
        let memory = memory_content.trim();

        if memory.is_empty() {
            base
        } else {
            format!("{}\n\n{}\n{}", base, Self::CORE_FACTS_HEADER, memory)
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
        // Custom presets still lead the prompt, but the shared operating
        // directives are appended to every preset (issue #138).
        let prompt = p.system_prompt("");
        assert!(prompt.starts_with("You are cheerful and upbeat!"));
        assert!(prompt.contains(Personality::OPERATING_DIRECTIVES));
    }

    #[test]
    fn test_custom_preset_overrides_built_in() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("transparent.personality.md");
        fs::write(&file_path, "Custom transparent override.").unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let prompt = p.system_prompt("");
        assert!(prompt.starts_with("Custom transparent override."));
        assert!(prompt.contains(Personality::OPERATING_DIRECTIVES));
    }

    #[test]
    fn test_system_prompt_with_memory() {
        let p = Personality::from_path(Path::new("/nonexistent"), "transparent");
        let prompt = p.system_prompt("User likes cats.");
        assert!(prompt.starts_with(&Personality::built_in_transparent()));
        // New core-facts framing (issue #138), third person.
        assert!(prompt.contains(Personality::CORE_FACTS_HEADER));
        assert!(prompt.contains("User likes cats."));
        // Operating directives are always present.
        assert!(prompt.contains(Personality::OPERATING_DIRECTIVES));
        // Legacy wording must be gone.
        assert!(!prompt.contains("Key facts I know about you:"));
        assert!(!prompt.contains("Note: This is not an exhaustive list."));
        assert!(!prompt.contains("kg_query"));
    }

    #[test]
    fn test_system_prompt_empty_memory_omits_core_facts_block() {
        let p = Personality::from_path(Path::new("/nonexistent"), "transparent");
        let prompt = p.system_prompt("");
        // Directives are always appended; the core-facts block is not.
        let expected = format!(
            "{}\n\n{}",
            Personality::built_in_transparent(),
            Personality::OPERATING_DIRECTIVES
        );
        assert_eq!(prompt, expected);
        assert!(!prompt.contains(Personality::CORE_FACTS_HEADER));
        assert!(!prompt.contains("Key facts I know about you:"));
    }

    #[test]
    fn test_operating_directives_present_for_all_built_ins() {
        for preset in ["transparent", "concise", "warm", "formal"] {
            let p = Personality::from_path(Path::new("/nonexistent"), preset);
            let prompt = p.system_prompt("");
            assert!(
                prompt.contains("Do not invent facts about the user."),
                "preset `{preset}` missing no-invention directive"
            );
            assert!(
                prompt.contains("retrieve_context"),
                "preset `{preset}` missing retrieve_context directive"
            );
            assert!(
                prompt.contains("remember"),
                "preset `{preset}` missing remember encouragement"
            );
            // Internal retrieval tools must not be surfaced to the core LLM.
            assert!(
                !prompt.contains("kg_query"),
                "preset `{preset}` mentions kg_query"
            );
            assert!(
                !prompt.contains("kg_search"),
                "preset `{preset}` mentions kg_search"
            );
            assert!(
                !prompt.contains("kg_related"),
                "preset `{preset}` mentions kg_related"
            );
        }
    }

    #[test]
    fn test_operating_directives_present_for_custom_preset() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "You are cheerful and upbeat!",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "cheerful");
        let prompt = p.system_prompt("");
        assert!(prompt.contains(Personality::OPERATING_DIRECTIVES));
        assert!(prompt.contains("retrieve_context"));
        assert!(prompt.contains("remember"));
        assert!(!prompt.contains("kg_query"));
    }

    #[test]
    fn test_core_facts_block_only_when_memory_present() {
        let p = Personality::from_path(Path::new("/nonexistent"), "transparent");

        let empty = p.system_prompt("");
        assert!(!empty.contains(Personality::CORE_FACTS_HEADER));

        let with_mem = p.system_prompt("User likes cats.");
        assert!(with_mem.contains(Personality::CORE_FACTS_HEADER));
        assert!(with_mem.contains("User likes cats."));
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
