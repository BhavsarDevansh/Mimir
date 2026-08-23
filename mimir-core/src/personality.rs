use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use tracing::warn;

use crate::config::PersonalityConfig;
use crate::paths;

/// Where a preset comes from (issue #387).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PresetSource {
    /// Compiled into the binary.
    Builtin,
    /// Loaded from a `<name>.personality.md` file in the config directory.
    Custom,
}

impl PresetSource {
    /// Stable human-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            PresetSource::Builtin => "Builtin",
            PresetSource::Custom => "Custom",
        }
    }
}

impl std::fmt::Display for PresetSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A discovered preset: name, source, and optional description (issue #387).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PresetInfo {
    pub name: String,
    pub source: PresetSource,
    pub description: Option<String>,
}

/// A non-fatal diagnostic emitted while resolving presets (issue #387).
///
/// The daemon logs these as warnings; `mimir personality list` prints them
/// to stderr so a missing or malformed preset is never silently ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetWarning {
    /// The file the diagnostic refers to, when it is file-related.
    pub path: Option<PathBuf>,
    /// Human-readable reason.
    pub reason: String,
}

/// A loaded preset: its verbatim prompt text plus discovery metadata.
#[derive(Debug, Clone, PartialEq)]
struct PresetEntry {
    prompt: String,
    source: PresetSource,
    description: Option<String>,
}

/// The personality engine: resolves the active preset and composes system prompts.
#[derive(Debug, Clone, PartialEq)]
pub struct Personality {
    active_name: String,
    registry: HashMap<String, PresetEntry>,
    warnings: Vec<PresetWarning>,
}

/// Advisory maximum size of a custom preset file in bytes (1 MiB, matching
/// `MAX_SKILL_FILE_SIZE`). Larger files still load, but the scan flags them
/// so the user knows every rescan reads them in full (issue #453).
const MAX_PRESET_FILE_SIZE: u64 = 1_048_576;

/// A daemon-owned cache of the custom-preset scan, keyed by a cheap
/// directory fingerprint (file names, sizes, and mtimes), so the hot chat
/// path never re-reads or re-parses preset files unless they changed
/// (issue #453). The active preset is still resolved per call, so
/// per-request `personality_preset` overrides always resolve against the
/// cached registry.
#[derive(Debug, Default)]
pub struct PersonalityCache {
    inner: Mutex<CachedPresetScan>,
}

/// The cached result of the last scan of one presets directory.
#[derive(Debug, Default)]
struct CachedPresetScan {
    dir: PathBuf,
    fingerprint: Option<DirFingerprint>,
    custom: Vec<(String, PresetEntry)>,
    warnings: Vec<PresetWarning>,
    scans: u64,
}

/// Cheap invalidation key for a presets directory: the directory's own
/// mtime plus one fingerprint per matching preset file. File contents are
/// never read while computing it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DirFingerprint {
    dir_modified: Option<SystemTime>,
    entries: Vec<EntryFingerprint>,
}

/// Per-file part of a [`DirFingerprint`]: identity, size, mtime, and kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EntryFingerprint {
    name: String,
    len: u64,
    modified: Option<SystemTime>,
    is_dir: bool,
}

impl Personality {
    /// Operating directives appended to every preset (issue #138). These are
    /// behavioural invariants of Mimir — retrieval and honesty — and apply
    /// uniformly to built-in and custom personalities. They are kept out of
    /// the per-preset tone text (DRY) and composed in [`system_prompt`].
    /// Learning is deliberately absent: remembering is hook-driven in Rust
    /// (issue #386), never delegated to the conversational LLM.
    const OPERATING_DIRECTIVES: &str = "\
Operating principles:
- Do not invent facts about the user. If you do not know the answer, say so.
- If you need more information, use the `retrieve_context` tool to dispatch a retrieval agent that investigates the knowledge graph and conversation history. If its findings are still not enough, refine the task and dispatch again. Continue until you have a confident answer or have confirmed the information is not in your knowledge base.";

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
        let personality = match paths::personalities_dir() {
            Ok(presets_dir) => Self::from_path(&presets_dir, &config.preset),
            Err(error) => Self::fallback_with_warning(config, error),
        };
        Self::log_warnings(&personality.warnings);
        personality
    }

    /// Create a `Personality` using a custom presets directory and preset
    /// name. Used by tests and by `mimir personality list`, which scans the
    /// same directory the daemon would.
    pub fn from_path(presets_dir: &Path, preset_name: &str) -> Self {
        let (custom, warnings) = Self::scan_custom_presets(presets_dir);
        Self::from_scan(custom, warnings, preset_name)
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
            .map(|entry| entry.prompt.clone())
            .unwrap_or_else(Self::built_in_transparent);

        let base = format!("{}\n\n{}", preset_prompt, Self::OPERATING_DIRECTIVES);
        let memory = memory_content.trim();

        if memory.is_empty() {
            base
        } else {
            format!("{}\n\n{}\n{}", base, Self::CORE_FACTS_HEADER, memory)
        }
    }

    /// List all available presets (built-in + custom), sorted by name.
    pub fn list_presets(&self) -> Vec<PresetInfo> {
        let mut presets: Vec<PresetInfo> = self
            .registry
            .iter()
            .map(|(name, entry)| PresetInfo {
                name: name.clone(),
                source: entry.source,
                description: entry.description.clone(),
            })
            .collect();
        presets.sort_by(|a, b| a.name.cmp(&b.name));
        presets
    }

    /// Return the name of the active preset.
    pub fn active_name(&self) -> &str {
        &self.active_name
    }

    /// Whether a preset with the given name exists (built-in or custom).
    ///
    /// Used by the OpenAI-compatible provider surface to decide whether a
    /// request `model` selects a personality preset or passes through as an
    /// upstream model override (issue #388).
    pub fn has_preset(&self, name: &str) -> bool {
        self.registry.contains_key(name)
    }

    /// Non-fatal diagnostics collected while resolving presets: malformed or
    /// unreadable custom preset files, unknown frontmatter keys, and an
    /// unknown configured preset (issue #387).
    pub fn warnings(&self) -> &[PresetWarning] {
        &self.warnings
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    fn built_in_presets() -> HashMap<String, PresetEntry> {
        let mut registry = HashMap::new();
        registry.insert(
            "transparent".to_string(),
            PresetEntry {
                prompt: Self::built_in_transparent(),
                description: Some(
                    "Warm, efficient, shows its work and admits uncertainty — the default"
                        .to_string(),
                ),
                source: PresetSource::Builtin,
            },
        );
        registry.insert(
            "concise".to_string(),
            PresetEntry {
                prompt: Self::built_in_concise(),
                description: Some(
                    "Minimal words, bullet points, no reasoning unless asked".to_string(),
                ),
                source: PresetSource::Builtin,
            },
        );
        registry.insert(
            "warm".to_string(),
            PresetEntry {
                prompt: Self::built_in_warm(),
                description: Some("Conversational and companion-like, uses your name".to_string()),
                source: PresetSource::Builtin,
            },
        );
        registry.insert(
            "formal".to_string(),
            PresetEntry {
                prompt: Self::built_in_formal(),
                description: Some("Neutral, structured, professional, no contractions".to_string()),
                source: PresetSource::Builtin,
            },
        );
        registry
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

    /// Scan the custom presets directory for `<name>.personality.md` files.
    ///
    /// Returns the discovered (name, entry) pairs plus non-fatal diagnostics:
    /// unreadable entries, invalid UTF-8, and malformed frontmatter are
    /// skipped with a warning; unknown frontmatter keys are ignored with a
    /// warning and the preset is still loaded (issue #387).
    fn scan_custom_presets(presets_dir: &Path) -> (Vec<(String, PresetEntry)>, Vec<PresetWarning>) {
        let mut results = Vec::new();
        let mut warnings = Vec::new();

        match presets_dir.try_exists() {
            Ok(true) => {}
            Ok(false) => return (results, warnings),
            Err(error) => {
                warnings.push(PresetWarning {
                    path: Some(presets_dir.to_path_buf()),
                    reason: format!("cannot access personalities directory: {error}"),
                });
                return (results, warnings);
            }
        }

        let entries = match std::fs::read_dir(presets_dir) {
            Ok(entries) => entries,
            Err(error) => {
                warnings.push(PresetWarning {
                    path: Some(presets_dir.to_path_buf()),
                    reason: format!("cannot read personalities directory: {error}"),
                });
                return (results, warnings);
            }
        };

        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    warnings.push(PresetWarning {
                        path: Some(presets_dir.to_path_buf()),
                        reason: format!("cannot read directory entry: {error}"),
                    });
                    continue;
                }
            };
            let Some(name) = Self::preset_name_from_path(&path) else {
                // Files that do not match the `<name>.personality.md`
                // convention are ignored by design, not invalid presets.
                continue;
            };
            if let Some(size) = std::fs::metadata(&path).ok().map(|m| m.len()) {
                if size > MAX_PRESET_FILE_SIZE {
                    Self::push_soft_warning(
                        &mut warnings,
                        &path,
                        format!(
                            "preset file exceeds {MAX_PRESET_FILE_SIZE} bytes and is read in full on every rescan; consider trimming it"
                        ),
                    );
                }
            }
            match Self::parse_preset_content(&path, &mut warnings) {
                Ok(entry) => results.push((name, entry)),
                Err(reason) => warnings.push(PresetWarning {
                    path: Some(path),
                    reason,
                }),
            }
        }

        (results, warnings)
    }

    /// Extract the preset name from a `<name>.personality.md` path, returning
    /// `None` for files that do not match the custom-preset naming
    /// convention or would produce an empty name (e.g. `.personality.md`).
    fn preset_name_from_path(path: &Path) -> Option<String> {
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            return None;
        }
        let stem = path.file_stem().and_then(|stem| stem.to_str())?;
        stem.strip_suffix(".personality")
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string())
    }

    /// Parse one custom preset file: optional YAML frontmatter with a
    /// `description` key, followed by verbatim prompt text.
    ///
    /// Hard failures (unreadable file, invalid UTF-8, malformed frontmatter)
    /// are returned as `Err(reason)` so the caller can warn and skip the
    /// file. Non-fatal problems (unknown keys, non-string description) are
    /// pushed to `warnings` and the file is still loaded.
    fn parse_preset_content(
        path: &Path,
        warnings: &mut Vec<PresetWarning>,
    ) -> Result<PresetEntry, String> {
        let contents =
            std::fs::read_to_string(path).map_err(|error| format!("cannot read file: {error}"))?;

        let Some(split) = crate::frontmatter::split_yaml_frontmatter(&contents) else {
            // No frontmatter: the whole body is the prompt, matching presets
            // written before descriptions existed (backwards compatible).
            return Ok(PresetEntry {
                prompt: contents,
                source: PresetSource::Custom,
                description: None,
            });
        };
        let (yaml, body) = split.map_err(str::to_string)?;

        let value: serde_yaml::Value = serde_yaml::from_str(yaml)
            .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;

        let mut description = None;
        match value {
            serde_yaml::Value::Mapping(mapping) => {
                for (key, value) in mapping {
                    let Some(key_str) = key.as_str() else {
                        Self::push_soft_warning(
                            warnings,
                            path,
                            "non-string frontmatter key ignored",
                        );
                        continue;
                    };
                    match (key_str, value) {
                        ("description", serde_yaml::Value::String(text)) => {
                            // Collapse internal newlines so a folded or
                            // literal block stays a single-line label.
                            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                            if !text.is_empty() {
                                description = Some(text);
                            }
                        }
                        ("description", _) => Self::push_soft_warning(
                            warnings,
                            path,
                            "frontmatter `description` must be a string; ignoring it",
                        ),
                        (other, _) => Self::push_soft_warning(
                            warnings,
                            path,
                            format!(
                                "unsupported frontmatter key `{other}` ignored; only `description` is supported"
                            ),
                        ),
                    }
                }
            }
            // An empty document (`---\n---`) is an empty frontmatter: valid,
            // with no description.
            serde_yaml::Value::Null => {}
            _ => Self::push_soft_warning(
                warnings,
                path,
                "frontmatter must be a YAML mapping; ignoring frontmatter",
            ),
        }

        Ok(PresetEntry {
            prompt: body.to_string(),
            source: PresetSource::Custom,
            description,
        })
    }

    /// Push a non-fatal, file-scoped diagnostic that does not prevent the
    /// preset from loading.
    fn push_soft_warning(
        warnings: &mut Vec<PresetWarning>,
        path: &Path,
        reason: impl std::fmt::Display,
    ) {
        warnings.push(PresetWarning {
            path: Some(path.to_path_buf()),
            reason: reason.to_string(),
        });
    }

    /// Resolve the active preset name, recording a diagnostic (and falling
    /// back to `transparent`) when the configured name is unknown.
    fn resolve_active_name(
        registry: &HashMap<String, PresetEntry>,
        preset_name: &str,
        warnings: &mut Vec<PresetWarning>,
    ) -> String {
        if registry.contains_key(preset_name) {
            preset_name.to_string()
        } else {
            warnings.push(PresetWarning {
                path: None,
                reason: format!(
                    "unknown personality preset '{preset_name}'; falling back to 'transparent'"
                ),
            });
            "transparent".to_string()
        }
    }

    /// Log every stored diagnostic as a daemon-side warning.
    fn log_warnings(warnings: &[PresetWarning]) {
        for warning in warnings {
            warn!(
                path = ?warning.path,
                reason = %warning.reason,
                "personality preset diagnostic"
            );
        }
    }

    /// Build a `Personality` from an already-scanned custom-preset set,
    /// merging built-ins (custom wins on name collision) and resolving the
    /// active preset. Shared by the one-shot constructors and the daemon
    /// scan cache (DRY, issue #453).
    fn from_scan(
        custom: Vec<(String, PresetEntry)>,
        mut warnings: Vec<PresetWarning>,
        preset_name: &str,
    ) -> Self {
        let mut registry = Self::built_in_presets();
        // Custom overrides built-in.
        for (name, entry) in custom {
            registry.insert(name, entry);
        }
        let active_name = Self::resolve_active_name(&registry, preset_name, &mut warnings);
        Self {
            active_name,
            registry,
            warnings,
        }
    }

    /// Build a `Personality` with only built-in presets when the
    /// personalities directory itself cannot be resolved.
    fn fallback_with_warning(config: &PersonalityConfig, error: impl std::fmt::Display) -> Self {
        let warnings = vec![PresetWarning {
            path: None,
            reason: format!("failed to resolve personalities directory: {error}"),
        }];
        Self::from_scan(Vec::new(), warnings, &config.preset)
    }
}

impl PersonalityCache {
    /// Built-in preset names, kept in sync with [`Personality::built_in_presets`].
    const BUILTIN_PRESET_NAMES: [&str; 4] = ["transparent", "concise", "warm", "formal"];

    /// Create an empty preset cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of directory scans this cache has performed. Every call to
    /// [`resolve`](Self::resolve) that finds a matching fingerprint is
    /// served from the cache without counting.
    pub fn scan_count(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .scans
    }

    /// Resolve the active personality from the default user personalities
    /// directory, re-reading preset files only when the directory
    /// fingerprint changed. The daemon's hot chat path uses this instead of
    /// [`Personality::new`] so requests never re-read or re-parse preset
    /// files unless they changed (issue #453).
    pub fn resolve(&self, config: &PersonalityConfig) -> Personality {
        match paths::personalities_dir() {
            Ok(presets_dir) => self.resolve_from_path(&presets_dir, &config.preset),
            Err(error) => {
                let personality = Personality::fallback_with_warning(config, error);
                Personality::log_warnings(&personality.warnings);
                personality
            }
        }
    }

    /// Path-based variant of [`resolve`](Self::resolve): resolve `preset_name`
    /// against a cache of `presets_dir`, rescanning only when the directory
    /// fingerprint changed. Used by tests and mirroring
    /// [`Personality::from_path`].
    pub fn resolve_from_path(&self, presets_dir: &Path, preset_name: &str) -> Personality {
        let fingerprint = Self::fingerprint(presets_dir);
        // A poisoned lock (a panic while scanning) must not brick every
        // subsequent chat request; recover the cached state and continue.
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hit =
            fingerprint.is_some() && guard.dir == presets_dir && guard.fingerprint == fingerprint;
        let (custom, warnings, fresh, scan_warning_count) = if hit {
            (
                guard.custom.clone(),
                guard.warnings.clone(),
                false,
                guard.warnings.len(),
            )
        } else {
            let (custom, warnings) = Personality::scan_custom_presets(presets_dir);
            let scan_warning_count = warnings.len();
            guard.dir = presets_dir.to_path_buf();
            guard.fingerprint = fingerprint;
            guard.custom = custom.clone();
            guard.warnings = warnings.clone();
            guard.scans += 1;
            (custom, warnings, true, scan_warning_count)
        };
        drop(guard);
        let personality = Personality::from_scan(custom, warnings, preset_name);
        // Scan diagnostics are logged once per scan; on a cache hit only the
        // per-request diagnostics added by resolution (an unknown preset
        // falling back to `transparent`) are logged, so a persistently
        // malformed preset does not re-log on every request while
        // request-scoped diagnostics are never silently dropped.
        let warnings = if fresh {
            &personality.warnings[..]
        } else {
            &personality.warnings[scan_warning_count..]
        };
        Personality::log_warnings(warnings);
        personality
    }

    /// Whether a preset with the given name exists (built-in or custom),
    /// without emitting per-request diagnostics.
    ///
    /// The OpenAI model-override path uses this probe so an unknown model
    /// name — a normal upstream override, not a configuration mistake — does
    /// not log an unknown-preset warning on every request (PR #466 review).
    pub fn has_preset(&self, name: &str) -> bool {
        if Self::is_builtin_preset(name) {
            return true;
        }
        match paths::personalities_dir() {
            Ok(presets_dir) => self.has_custom_preset_from_path(&presets_dir, name),
            Err(_) => false,
        }
    }

    /// Whether a custom preset exists under `presets_dir`, rescanning only
    /// when the directory fingerprint changed (mirrors
    /// [`Self::resolve_from_path`]). Scan diagnostics are logged by the next
    /// `resolve`; this probe stays silent.
    fn has_custom_preset_from_path(&self, presets_dir: &Path, name: &str) -> bool {
        let fingerprint = Self::fingerprint(presets_dir);
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let hit =
            fingerprint.is_some() && guard.dir == presets_dir && guard.fingerprint == fingerprint;
        if !hit {
            let (custom, warnings) = Personality::scan_custom_presets(presets_dir);
            guard.dir = presets_dir.to_path_buf();
            guard.fingerprint = fingerprint;
            guard.custom = custom;
            guard.warnings = warnings;
            guard.scans += 1;
        }
        guard
            .custom
            .iter()
            .any(|(preset_name, _)| preset_name == name)
    }

    fn is_builtin_preset(name: &str) -> bool {
        Self::BUILTIN_PRESET_NAMES.contains(&name)
    }

    /// Compute a cheap fingerprint of the presets directory: the directory's
    /// own mtime plus `(name, size, mtime, kind)` for every file matching the
    /// `<name>.personality.md` convention. File contents are never read.
    ///
    /// Returns `None` when the directory cannot be enumerated, so an
    /// unreadable directory always rescans (and re-warns) instead of being
    /// served from a stale cache. A *missing* directory has a stable,
    /// cacheable fingerprint so the daemon does not rescan a directory the
    /// user never created. A single preset-named entry that cannot be
    /// stat-ed is fingerprinted as a zero-sized, mtime-less entry (the scan
    /// reports it as a warning), so one bad entry cannot disable the cache.
    fn fingerprint(presets_dir: &Path) -> Option<DirFingerprint> {
        let dir_modified = presets_dir.metadata().ok().and_then(|m| m.modified().ok());
        let mut entries = Vec::new();
        match presets_dir.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                return Some(DirFingerprint {
                    dir_modified: None,
                    entries,
                });
            }
            Err(_) => return None,
        }
        let read_dir = std::fs::read_dir(presets_dir).ok()?;
        for entry in read_dir {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(_) => {
                    // A failing directory entry cannot be named or stat-ed;
                    // the scan reports it as a warning, and it reappears in
                    // the fingerprint as soon as the directory yields it
                    // again. Skipping it keeps the rest of the fingerprint
                    // cacheable instead of disabling the cache entirely.
                    continue;
                }
            };
            let Some(name) = Personality::preset_name_from_path(&path) else {
                // Non-preset entries never affect the scan, so they are
                // excluded from the fingerprint.
                continue;
            };
            // Follow symlinks (`Path::metadata`, like the scan's
            // `read_to_string` and size check) so edits to a symlinked
            // preset's target invalidate the cache instead of pinning it. A
            // preset-named entry that cannot be stat-ed (for example a
            // dangling symlink) is recorded as a zero-sized, mtime-less
            // entry so one bad entry cannot disable the cache; the scan
            // reports it as a warning, and it is re-statted as soon as its
            // target reappears.
            let metadata = std::fs::metadata(&path).ok();
            entries.push(EntryFingerprint {
                name,
                len: metadata.as_ref().map(|m| m.len()).unwrap_or_default(),
                modified: metadata.as_ref().and_then(|m| m.modified().ok()),
                is_dir: metadata.as_ref().is_some_and(|m| m.is_dir()),
            });
        }
        entries.sort();
        Some(DirFingerprint {
            dir_modified,
            entries,
        })
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
                !prompt.contains("remember"),
                "preset `{preset}` must not mention the removed remember tool"
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
        assert!(!prompt.contains("remember"));
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
        for name in ["concise", "formal", "transparent", "warm", "cheerful"] {
            assert!(
                presets.iter().any(|info| info.name == name),
                "missing {name}"
            );
        }
    }

    #[test]
    fn test_non_md_files_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("cheerful.personality.txt"), "Not valid.").unwrap();
        fs::write(dir.path().join("other.md"), "Also not valid.").unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let presets = p.list_presets();
        assert!(!presets.iter().any(|info| info.name == "cheerful"));
        assert!(!presets.iter().any(|info| info.name == "other"));
        assert!(p.warnings().is_empty());
    }

    #[test]
    fn test_empty_preset_name_ignored() {
        let dir = tempfile::tempdir().unwrap();
        // `.personality.md` would strip to an empty name; it must not be
        // registered as a preset.
        fs::write(dir.path().join(".personality.md"), "Not a preset.").unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let presets = p.list_presets();
        assert!(!presets.iter().any(|info| info.name.is_empty()));
        assert!(p.warnings().is_empty());
    }

    // ------------------------------------------------------------------
    // Preset discovery: descriptions, sources, diagnostics (issue #387)
    // ------------------------------------------------------------------

    fn preset_info<'a>(presets: &'a [PresetInfo], name: &str) -> &'a PresetInfo {
        presets
            .iter()
            .find(|info| info.name == name)
            .unwrap_or_else(|| panic!("preset `{name}` not found in {presets:#?}"))
    }

    #[test]
    fn test_built_in_presets_have_descriptions() {
        let p = Personality::from_path(Path::new("/nonexistent"), "transparent");
        let presets = p.list_presets();
        for name in ["transparent", "concise", "warm", "formal"] {
            let info = preset_info(&presets, name);
            assert_eq!(info.source, PresetSource::Builtin);
            assert!(
                info.description.as_deref().is_some_and(|d| !d.is_empty()),
                "built-in preset `{name}` must carry a description"
            );
        }
    }

    #[test]
    fn test_list_presets_reports_custom_source_and_description() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "---\ndescription: Cheerful and upbeat\n---\nYou are cheerful!",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let presets = p.list_presets();
        let info = preset_info(&presets, "cheerful");
        assert_eq!(info.source, PresetSource::Custom);
        assert_eq!(info.description.as_deref(), Some("Cheerful and upbeat"));
        assert!(p.warnings().is_empty());
    }

    #[test]
    fn test_frontmatter_body_is_verbatim_prompt() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "---\ndescription: Cheerful\n---\n  You are cheerful!\n\nSecond line.",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "cheerful");
        let prompt = p.system_prompt("");
        // The body after the closing fence is preserved verbatim as the
        // prompt text, including leading whitespace.
        assert!(prompt.starts_with("  You are cheerful!\n\nSecond line."));
        assert!(prompt.contains(Personality::OPERATING_DIRECTIVES));
    }

    #[cfg(unix)]
    #[test]
    fn test_unreadable_presets_directory_warns() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let presets_dir = dir.path().join("personalities");
        fs::create_dir(&presets_dir).unwrap();
        // Blocking the parent directory makes the presets dir itself
        // unstat-able, which `try_exists` reports as an access error.
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o000)).unwrap();

        // Root bypasses permission checks, so the error path cannot be
        // exercised; restore permissions and skip rather than assert on a
        // warning that will not fire.
        if fs::metadata(&presets_dir).is_ok() {
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }

        let p = Personality::from_path(&presets_dir, "transparent");
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].reason.contains("cannot access"));

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn test_custom_preset_without_frontmatter_has_no_description() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("plain.personality.md"), "You are plain.").unwrap();

        let p = Personality::from_path(dir.path(), "plain");
        let presets = p.list_presets();
        let info = preset_info(&presets, "plain");
        assert_eq!(info.source, PresetSource::Custom);
        assert_eq!(info.description, None);
        assert!(p.warnings().is_empty());
        assert!(p.system_prompt("").starts_with("You are plain."));
    }

    #[test]
    fn test_frontmatter_without_description_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("plain.personality.md"),
            "---\n---\nYou are plain.",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "plain");
        let presets = p.list_presets();
        let info = preset_info(&presets, "plain");
        assert_eq!(info.description, None);
        assert!(p.warnings().is_empty());
    }

    #[test]
    fn test_unterminated_frontmatter_warns_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("broken.personality.md"),
            "---\ndescription: Broken\nbody never closes",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        assert!(!p.list_presets().iter().any(|info| info.name == "broken"));
        assert_eq!(p.warnings().len(), 1);
        assert_eq!(
            p.warnings()[0].path.as_deref(),
            Some(dir.path().join("broken.personality.md").as_path())
        );
        assert!(p.warnings()[0].reason.contains("not closed"));
    }

    #[test]
    fn test_invalid_yaml_frontmatter_warns_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("broken.personality.md"),
            "---\ndescription: [unclosed\n---\nbody",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        assert!(!p.list_presets().iter().any(|info| info.name == "broken"));
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].reason.contains("YAML"));
    }

    #[test]
    fn test_unknown_frontmatter_key_warns_but_loads() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "---\nstyle: concise\ndescription: Cheerful\n---\nYou are cheerful!",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "cheerful");
        let presets = p.list_presets();
        let info = preset_info(&presets, "cheerful");
        assert_eq!(info.description.as_deref(), Some("Cheerful"));
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].reason.contains("style"));
    }

    #[test]
    fn test_non_string_description_warns_but_loads() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("weird.personality.md"),
            "---\ndescription: [a, b]\n---\nYou are weird!",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "weird");
        let presets = p.list_presets();
        let info = preset_info(&presets, "weird");
        assert_eq!(info.description, None);
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].reason.contains("description"));
    }

    #[test]
    fn test_multiline_description_collapses_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("multi.personality.md"),
            "---\ndescription: |\n  Line one\n  line two\n---\nbody",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "multi");
        let presets = p.list_presets();
        let info = preset_info(&presets, "multi");
        assert_eq!(info.description.as_deref(), Some("Line one line two"));
    }

    #[test]
    fn test_unreadable_custom_preset_warns_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        // A directory named like a preset file cannot be read as a file.
        fs::create_dir(dir.path().join("broken.personality.md")).unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        assert!(!p.list_presets().iter().any(|info| info.name == "broken"));
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].reason.contains("cannot read"));
    }

    #[test]
    fn test_invalid_utf8_custom_preset_warns_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("broken.personality.md"),
            [0xff, 0xfe, 0x00, 0x41],
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        assert!(!p.list_presets().iter().any(|info| info.name == "broken"));
        assert_eq!(p.warnings().len(), 1);
    }

    #[test]
    fn test_unknown_configured_preset_stores_warning() {
        let dir = tempfile::tempdir().unwrap();
        let p = Personality::from_path(dir.path(), "ghost");
        assert_eq!(p.active_name(), "transparent");
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].reason.contains("ghost"));
    }

    #[test]
    fn test_custom_override_of_builtin_reports_custom_source_and_description() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("transparent.personality.md"),
            "---\ndescription: My own transparent\n---\nCustom transparent override.",
        )
        .unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let prompt = p.system_prompt("");
        assert!(prompt.starts_with("Custom transparent override."));
        let presets = p.list_presets();
        let info = preset_info(&presets, "transparent");
        assert_eq!(info.source, PresetSource::Custom);
        assert_eq!(info.description.as_deref(), Some("My own transparent"));
    }

    #[test]
    fn test_list_presets_is_sorted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("zeta.personality.md"), "Z").unwrap();
        fs::write(dir.path().join("alpha.personality.md"), "A").unwrap();

        let p = Personality::from_path(dir.path(), "transparent");
        let presets = p.list_presets();
        let names: Vec<&str> = presets.iter().map(|info| info.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    // ------------------------------------------------------------------
    // Preset-scan caching (issue #453)
    // ------------------------------------------------------------------

    #[test]
    fn test_cache_resolves_custom_preset() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "You are cheerful!",
        )
        .unwrap();

        let cache = PersonalityCache::default();
        let p = cache.resolve_from_path(dir.path(), "cheerful");
        assert_eq!(p.active_name(), "cheerful");
        assert!(p.system_prompt("").starts_with("You are cheerful!"));
        assert_eq!(cache.scan_count(), 1);
    }

    #[test]
    fn test_cache_hit_does_not_rescan_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "You are cheerful!",
        )
        .unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "cheerful");
        assert_eq!(first.active_name(), "cheerful");

        // A per-request preset override resolves against the cached
        // registry without re-scanning the directory (issue #453).
        let second = cache.resolve_from_path(dir.path(), "transparent");
        assert_eq!(second.active_name(), "transparent");
        assert!(
            second
                .list_presets()
                .iter()
                .any(|info| info.name == "cheerful")
        );
        assert_eq!(cache.scan_count(), 1);
    }

    #[test]
    fn test_cache_has_preset_probe_finds_builtin_and_custom() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "You are cheerful!",
        )
        .unwrap();

        let cache = PersonalityCache::default();
        assert!(cache.has_preset("transparent"), "built-in preset");
        assert!(
            cache.has_custom_preset_from_path(dir.path(), "cheerful"),
            "custom preset"
        );
        assert!(
            !cache.has_custom_preset_from_path(dir.path(), "gpt-4o"),
            "upstream model name"
        );
        assert_eq!(cache.scan_count(), 1, "one scan for the custom probe");
    }

    #[test]
    fn test_builtin_preset_names_match_registry() {
        let registry = Personality::built_in_presets();
        let names: Vec<&str> = registry.keys().map(String::as_str).collect();
        for name in PersonalityCache::BUILTIN_PRESET_NAMES {
            assert!(names.contains(&name), "missing built-in: {name}");
        }
        assert_eq!(
            names.len(),
            PersonalityCache::BUILTIN_PRESET_NAMES.len(),
            "built-in registry must not drift from the name list"
        );
    }

    #[test]
    fn test_cache_invalidates_when_preset_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cheerful.personality.md");
        fs::write(&file, "Version one").unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "cheerful");
        assert!(first.system_prompt("").starts_with("Version one"));

        fs::write(&file, "Version two, longer").unwrap();
        let second = cache.resolve_from_path(dir.path(), "cheerful");
        assert!(second.system_prompt("").starts_with("Version two"));
        assert_eq!(cache.scan_count(), 2);
    }

    #[test]
    fn test_cache_invalidates_when_preset_file_added() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("alpha.personality.md"), "Alpha").unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "transparent");
        assert!(!first.list_presets().iter().any(|info| info.name == "beta"));

        fs::write(dir.path().join("beta.personality.md"), "Beta").unwrap();
        let second = cache.resolve_from_path(dir.path(), "beta");
        assert_eq!(second.active_name(), "beta");
        assert_eq!(cache.scan_count(), 2);
    }

    #[test]
    fn test_cache_invalidates_when_preset_file_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cheerful.personality.md");
        fs::write(&file, "You are cheerful!").unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "cheerful");
        assert_eq!(first.active_name(), "cheerful");

        fs::remove_file(&file).unwrap();
        let second = cache.resolve_from_path(dir.path(), "cheerful");
        assert_eq!(second.active_name(), "transparent");
        assert!(
            !second
                .list_presets()
                .iter()
                .any(|info| info.name == "cheerful")
        );
        assert_eq!(cache.scan_count(), 2);
    }

    #[test]
    fn test_cache_detects_personalities_directory_creation() {
        let dir = tempfile::tempdir().unwrap();
        let presets = dir.path().join("personalities");

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(&presets, "transparent");
        assert_eq!(first.active_name(), "transparent");
        assert!(
            !first
                .list_presets()
                .iter()
                .any(|info| info.name == "cheerful")
        );

        fs::create_dir(&presets).unwrap();
        fs::write(presets.join("cheerful.personality.md"), "You are cheerful!").unwrap();
        let second = cache.resolve_from_path(&presets, "cheerful");
        assert_eq!(second.active_name(), "cheerful");
        assert_eq!(cache.scan_count(), 2);
    }

    #[test]
    fn test_cache_fresh_scan_reports_malformed_file_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("broken.personality.md");
        fs::write(&file, "---\ndescription: [unclosed\n---\nbody").unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "transparent");
        assert_eq!(first.warnings().len(), 1);
        assert!(first.warnings()[0].reason.contains("YAML"));

        // Fixing the file invalidates the cached warnings.
        fs::write(&file, "Fixed").unwrap();
        let second = cache.resolve_from_path(dir.path(), "transparent");
        assert!(second.warnings().is_empty());
        assert_eq!(cache.scan_count(), 2);
    }

    #[test]
    fn test_cache_warns_on_oversized_preset_file() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(MAX_PRESET_FILE_SIZE as usize + 1);
        fs::write(dir.path().join("big.personality.md"), &big).unwrap();

        let cache = PersonalityCache::default();
        let p = cache.resolve_from_path(dir.path(), "big");
        // The oversized file still loads, but the scan flags it so the user
        // knows every rescan reads it in full (issue #453).
        assert!(p.list_presets().iter().any(|info| info.name == "big"));
        assert!(
            p.warnings()
                .iter()
                .any(|warning| warning.reason.contains("exceeds"))
        );
    }

    #[test]
    fn test_cache_hit_still_reports_unknown_preset_warning() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("cheerful.personality.md"),
            "You are cheerful!",
        )
        .unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "cheerful");
        assert_eq!(first.active_name(), "cheerful");

        // A cache hit with an unknown per-request override still reports the
        // fallback diagnostic instead of silently swallowing it.
        let second = cache.resolve_from_path(dir.path(), "does-not-exist");
        assert_eq!(second.active_name(), "transparent");
        assert_eq!(cache.scan_count(), 1);
        assert!(
            second
                .warnings()
                .iter()
                .any(|warning| warning.reason.contains("unknown personality preset"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_cache_invalidates_when_symlinked_preset_target_changes() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("preset-target.txt");
        let link = dir.path().join("cheerful.personality.md");
        fs::write(&target, "Version one").unwrap();
        symlink(&target, &link).unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "cheerful");
        assert!(first.system_prompt("").starts_with("Version one"));

        // Same-length rewrite: only the target mtime changes, so the cache
        // must track the target's metadata, not the symlink's.
        fs::write(&target, "Version two").unwrap();
        let second = cache.resolve_from_path(dir.path(), "cheerful");
        assert!(second.system_prompt("").starts_with("Version two"));
        assert_eq!(cache.scan_count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn test_cache_hit_with_dangling_symlink_preset() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("good.personality.md"), "You are good!").unwrap();
        let link = dir.path().join("broken.personality.md");
        let missing_target = dir.path().join("missing-target.txt");
        symlink(&missing_target, &link).unwrap();

        let cache = PersonalityCache::default();
        let first = cache.resolve_from_path(dir.path(), "good");
        assert!(first.system_prompt("").starts_with("You are good!"));
        assert!(
            first
                .warnings()
                .iter()
                .any(|warning| warning.reason.contains("cannot read file"))
        );

        // The dangling symlink cannot be stat-ed but must not disable the
        // cache: the second resolution is a hit served from the cached scan
        // (with its warning) instead of rescanning the directory.
        let second = cache.resolve_from_path(dir.path(), "good");
        assert!(second.system_prompt("").starts_with("You are good!"));
        assert!(
            second
                .warnings()
                .iter()
                .any(|warning| warning.reason.contains("cannot read file"))
        );
        assert_eq!(cache.scan_count(), 1);

        // Creating the symlink target changes the entry's metadata, so the
        // next resolution rescans and picks the preset up.
        fs::write(&missing_target, "Now it exists!").unwrap();
        let third = cache.resolve_from_path(dir.path(), "broken");
        assert_eq!(third.active_name(), "broken");
        assert!(third.system_prompt("").starts_with("Now it exists!"));
        assert_eq!(cache.scan_count(), 2);
    }
}
