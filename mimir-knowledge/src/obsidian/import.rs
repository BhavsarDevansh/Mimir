//! Import planning and application: Obsidian Markdown → Knowledge Graph.
//!
//! Files are parsed with the shared [`grammar`](super::grammar), entities are
//! resolved through the canonical chain (exact name → alias → FTS5 fuzzy →
//! create, issue #182), and facts flow through
//! [`normalize_and_insert`](crate::normalize::normalize_and_insert) with
//! `source_type=Import`, `extraction_method=StructuredParse`, and a
//! `raw_reference` of `obsidian:<relative-path>` for provenance.
//!
//! Dry-run mode plans everything (entity resolution, existence checks) but
//! never writes: the reported counts are exactly what an apply would change.

use std::path::{Path, PathBuf};

use crate::models::audit_log::ChangedBy;
use crate::models::entity::{Entity, EntityType};
use crate::models::enums::EventType;
use crate::models::preference::{NewPreference, PreferenceSourceType, UpsertPreferenceInput};
use crate::models::source::{ExtractionMethod, SourceType};
use crate::normalize::{NormalizedFact, Provenance, normalize_and_insert};
use crate::normalize::{pick_resolution, resolve_or_create};
use crate::queries;
use crate::{KnowledgeError, KnowledgeGraph};

use super::grammar::{
    Frontmatter, ObsidianObject, ParsedFactLine, ParsedPreference, SECTION_DATES, SECTION_FACTS,
    SECTION_PREFERENCES, SECTION_RELATIONSHIPS, parse_fact_line, parse_preference_line,
};

/// One Markdown file participating in an import (or produced by an export).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObsidianFile {
    /// Vault-relative path (e.g. `sub/Alice.md`), used for reporting and as
    /// the `obsidian:` provenance reference on imported facts.
    pub relative_path: String,
    /// Full Markdown document content.
    pub content: String,
}

/// What an import would change (or did change), in dry-run and apply modes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObsidianImportCounts {
    pub entities_new: usize,
    pub entities_updated: usize,
    pub facts_new: usize,
    pub facts_existing: usize,
    pub preferences_new: usize,
    pub preferences_updated: usize,
    /// Facts parsed from the `Dates` section (also counted in `facts_new`).
    pub dates_new: usize,
}

/// Result of planning/applying an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObsidianImportOutcome {
    pub dry_run: bool,
    pub counts: ObsidianImportCounts,
    /// Per-file parse failures and per-fact pipeline errors, prefixed with the
    /// vault-relative path. Tolerated per file: one broken note never aborts
    /// the rest of the vault.
    pub errors: Vec<String>,
}

/// Collect every `.md` file under `dir` (recursive, deterministic order).
///
/// Hidden entries are skipped; non-markdown files are ignored.
pub fn scan_markdown_files(dir: &Path) -> Result<Vec<PathBuf>, KnowledgeError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(KnowledgeError::Io)?;
        let mut dirs = Vec::new();
        for entry in entries {
            let entry = entry.map_err(KnowledgeError::Io)?;
            let path = entry.path();
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if file_name.to_ascii_lowercase().ends_with(".md") {
                out.push(path);
            }
        }
        stack.extend(dirs);
    }
    out.sort();
    Ok(out)
}

/// Plan and (unless `dry_run`) apply an import for the given files.
pub(crate) async fn import_all(
    kg: &KnowledgeGraph,
    files: &[ObsidianFile],
    dry_run: bool,
) -> Result<ObsidianImportOutcome, KnowledgeError> {
    let mut outcome = ObsidianImportOutcome {
        dry_run,
        counts: ObsidianImportCounts::default(),
        errors: Vec::new(),
    };

    for file in files {
        let document = match parse_document(file) {
            Ok(doc) => doc,
            Err(error) => {
                outcome
                    .errors
                    .push(format!("{}: {error}", file.relative_path));
                continue;
            }
        };

        if let Err(error) = import_document(kg, file, document, dry_run, &mut outcome).await {
            outcome
                .errors
                .push(format!("{}: {error}", file.relative_path));
        }
    }

    Ok(outcome)
}

/// A parsed document: frontmatter + entity name + per-section lines.
struct ParsedDocument {
    frontmatter: Frontmatter,
    name: String,
    entity_type: EntityType,
    dates: Vec<ParsedFactLine>,
    relationships: Vec<ParsedFactLine>,
    preferences: Vec<ParsedPreference>,
    facts: Vec<ParsedFactLine>,
}

fn parse_document(file: &ObsidianFile) -> Result<ParsedDocument, String> {
    let (frontmatter_raw, body) =
        match mimir_core::frontmatter::split_yaml_frontmatter(&file.content) {
            None => (None, file.content.as_str()),
            Some(Ok((yaml, body))) => (Some(yaml), body),
            Some(Err(_)) => {
                return Err(
                    "malformed YAML frontmatter: opened with `---` but never closed".to_string(),
                );
            }
        };
    let frontmatter = match frontmatter_raw {
        Some(raw) => Frontmatter::parse(raw)?,
        None => Frontmatter {
            entity_id: None,
            entity_type: None,
            aliases: Vec::new(),
            created: None,
            updated: None,
        },
    };
    let entity_type = frontmatter
        .entity_type
        .as_deref()
        .map(|raw| {
            raw.parse::<EntityType>()
                .map_err(|_| format!("unknown entity type {raw:?}"))
        })
        .transpose()?
        .unwrap_or(EntityType::Concept);

    let mut name: Option<String> = None;
    let mut section: Option<&'static str> = None;
    let mut dates = Vec::new();
    let mut relationships = Vec::new();
    let mut preferences = Vec::new();
    let mut facts = Vec::new();

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("# ") {
            if !heading.starts_with('#') && name.is_none() {
                name = Some(heading.trim().to_string());
            }
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            section = match heading.trim() {
                SECTION_DATES => Some(SECTION_DATES),
                SECTION_RELATIONSHIPS => Some(SECTION_RELATIONSHIPS),
                SECTION_PREFERENCES => Some(SECTION_PREFERENCES),
                SECTION_FACTS => Some(SECTION_FACTS),
                _ => None, // prose sections are ignored
            };
            continue;
        }
        if !trimmed.starts_with("- ") {
            continue; // prose lines are ignored
        }
        let Some(current) = section else {
            return Err(format!(
                "fact line {trimmed:?} appears before any section heading"
            ));
        };
        if current == SECTION_PREFERENCES {
            preferences.push(parse_preference_line(trimmed)?);
        } else {
            let parsed = parse_fact_line(current, trimmed)?;
            match current {
                SECTION_DATES => dates.push(parsed),
                SECTION_RELATIONSHIPS => relationships.push(parsed),
                _ => facts.push(parsed),
            }
        }
    }

    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| entity_name_from_path(&file.relative_path));

    Ok(ParsedDocument {
        frontmatter,
        name,
        entity_type,
        dates,
        relationships,
        preferences,
        facts,
    })
}

/// Fall back to the file stem when a document has no `# heading`.
fn entity_name_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "entity".to_string())
}

/// Process one document: resolve/create the entity, plan + apply its facts
/// and preferences.
async fn import_document(
    kg: &KnowledgeGraph,
    file: &ObsidianFile,
    document: ParsedDocument,
    dry_run: bool,
    outcome: &mut ObsidianImportOutcome,
) -> Result<(), KnowledgeError> {
    // ------------------------------------------------------------------
    // Subject entity: an anchored entity_id wins; otherwise resolve the name
    // through the canonical chain (never creating in dry-run).
    // ------------------------------------------------------------------
    let subject: Option<Entity> = if let Some(entity_id) = document.frontmatter.entity_id {
        match kg.get_entity(entity_id).await? {
            Some(existing) => {
                let (entity, changed) =
                    apply_entity_updates(kg, &existing, &document, dry_run).await?;
                if changed {
                    outcome.counts.entities_updated += 1;
                }
                Some(entity)
            }
            None => {
                outcome.counts.entities_new += 1;
                if dry_run {
                    None
                } else {
                    Some(
                        kg.create_entity(
                            &document.name,
                            document.entity_type,
                            &document
                                .frontmatter
                                .aliases
                                .iter()
                                .map(String::as_str)
                                .collect::<Vec<_>>(),
                        )
                        .await?,
                    )
                }
            }
        }
    } else {
        let (resolved, created) =
            resolve_subject(kg, &document.name, document.entity_type, dry_run).await?;
        if created {
            outcome.counts.entities_new += 1;
            resolved
        } else if let Some(entity) = resolved {
            let (entity, changed) = apply_entity_updates(kg, &entity, &document, dry_run).await?;
            if changed {
                outcome.counts.entities_updated += 1;
            }
            Some(entity)
        } else {
            None
        }
    };

    // ------------------------------------------------------------------
    // Facts (Dates / Relationships / Facts sections)
    // ------------------------------------------------------------------
    let mut batch: Vec<NormalizedFact> = Vec::new();
    let all_lines = document
        .dates
        .iter()
        .map(|line| (SECTION_DATES, line))
        .chain(
            document
                .relationships
                .iter()
                .map(|line| (SECTION_RELATIONSHIPS, line)),
        )
        .chain(document.facts.iter().map(|line| (SECTION_FACTS, line)));

    for (section, line) in all_lines {
        let (object_name, object_id) =
            plan_object(kg, &line.object, dry_run, &mut outcome.counts).await?;
        let existing = if let Some(subject) = &subject {
            let predicate_id = kg.relationship_type_id(&line.predicate).await;
            let literal = object_literal_of(line);
            match (predicate_id, object_id) {
                (Some(predicate_id), Some(object_id)) => {
                    queries::fact::exists_triple(
                        kg.pool(),
                        subject.id,
                        predicate_id,
                        Some(object_id),
                        None,
                    )
                    .await?
                }
                (Some(predicate_id), None) => {
                    queries::fact::exists_triple(
                        kg.pool(),
                        subject.id,
                        predicate_id,
                        None,
                        Some(literal),
                    )
                    .await?
                }
                (None, _) => false,
            }
        } else {
            false
        };

        if existing {
            outcome.counts.facts_existing += 1;
            continue;
        }

        if section == SECTION_DATES {
            outcome.counts.dates_new += 1;
        }
        outcome.counts.facts_new += 1;

        if dry_run {
            continue;
        }
        let Some(subject) = &subject else {
            // Apply mode always resolves/creates both sides before this point.
            continue;
        };
        batch.push(normalized_fact(
            subject,
            line,
            object_name,
            object_id,
            &file.relative_path,
        ));
    }

    if !dry_run && !batch.is_empty() {
        let pipeline = normalize_and_insert(
            kg,
            batch,
            Provenance::chat(ExtractionMethod::StructuredParse),
        )
        .await?;
        for error in pipeline.errors {
            outcome
                .errors
                .push(format!("{}: {error}", file.relative_path));
        }
    }

    // ------------------------------------------------------------------
    // Preferences
    // ------------------------------------------------------------------
    for preference in &document.preferences {
        let existing = match &subject {
            Some(entity) => {
                kg.get_preference(Some(entity.id), &preference.key, &[])
                    .await?
            }
            None => None,
        };
        if existing.is_some() {
            outcome.counts.preferences_updated += 1;
        } else {
            outcome.counts.preferences_new += 1;
        }
        if dry_run {
            continue;
        }
        let Some(entity) = &subject else {
            continue; // apply mode always has the subject entity
        };
        kg.upsert_preference(UpsertPreferenceInput {
            preference: NewPreference {
                entity_id: Some(entity.id),
                category: preference.category,
                key: preference.key.clone(),
                value: preference.value.clone(),
                confidence: 0.80,
                overridden_by_user: false,
                source_fact_id: None,
            },
            changed_by: ChangedBy::User,
            contexts: Vec::new(),
            sources: vec![(
                PreferenceSourceType::UserEdit,
                format!("obsidian:{}", file.relative_path),
            )],
        })
        .await?;
    }

    Ok(())
}

/// Resolve a fact line's object entity and update the entity-creation count.
///
/// Returns the object's canonical name and id (`None` id when the object is a
/// literal, or when the entity would be created in dry-run mode).
async fn plan_object(
    kg: &KnowledgeGraph,
    object: &ObsidianObject,
    dry_run: bool,
    counts: &mut ObsidianImportCounts,
) -> Result<(String, Option<i32>), KnowledgeError> {
    match object {
        ObsidianObject::Literal(value) => Ok((value.clone(), None)),
        ObsidianObject::Entity(name) => {
            // Canonical resolution is type-filtered (issue #182); the object
            // type is not recorded in the file, so the Concept filter can
            // miss a same-name entity of another type (e.g. a `Person`
            // `[[Alice]]`). `create_entity`'s upsert reuses that entity, so
            // the exact same-name fallback keeps dry-run and apply identical.
            let results =
                queries::entity::get_by_name_typed(kg.pool(), name, EntityType::Concept).await?;
            if let Some(entity) = pick_resolution(&results) {
                return Ok((entity.name.clone(), Some(entity.id)));
            }
            if let Some(entity) = queries::entity::get_exact_name(kg.pool(), name).await? {
                return Ok((entity.name.clone(), Some(entity.id)));
            }
            counts.entities_new += 1;
            if dry_run {
                Ok((name.clone(), None))
            } else {
                let (entity, _) = resolve_or_create(kg, name, EntityType::Concept).await?;
                Ok((entity.name.clone(), Some(entity.id)))
            }
        }
    }
}

/// Resolve the subject entity through the canonical chain without creating in
/// dry-run mode.
async fn resolve_subject(
    kg: &KnowledgeGraph,
    name: &str,
    entity_type: EntityType,
    dry_run: bool,
) -> Result<(Option<Entity>, bool), KnowledgeError> {
    let results = queries::entity::get_by_name_typed(kg.pool(), name, entity_type).await?;
    if let Some(entity) = pick_resolution(&results) {
        return Ok((Some(entity.clone()), false));
    }
    // Same-name entity of another type: reuse it (`apply_entity_updates`
    // reconciles the type) instead of double-counting a phantom "new".
    if let Some(entity) = queries::entity::get_exact_name(kg.pool(), name).await? {
        return Ok((Some(entity), false));
    }
    if dry_run {
        Ok((None, true))
    } else {
        let (entity, created) = resolve_or_create(kg, name, entity_type).await?;
        Ok((Some(entity), created))
    }
}

/// Apply name/type/alias updates for an existing entity.
///
/// Returns the current entity (re-fetched after any rename so downstream
/// planning never uses a stale name) and whether anything changed.
async fn apply_entity_updates(
    kg: &KnowledgeGraph,
    entity: &Entity,
    document: &ParsedDocument,
    dry_run: bool,
) -> Result<(Entity, bool), KnowledgeError> {
    let type_changed =
        EntityType::try_from(entity.entity_type_id).ok() != Some(document.entity_type);
    let name_changed = entity.name != document.name;
    let known_aliases: Vec<String> = entity
        .aliases
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    let aliases_missing: Vec<&String> = document
        .frontmatter
        .aliases
        .iter()
        .filter(|alias| !known_aliases.iter().any(|known| known == *alias))
        .collect();

    if !dry_run {
        if name_changed || type_changed {
            kg.update_entity(entity.id, &document.name, document.entity_type)
                .await?;
        }
        for alias in &aliases_missing {
            kg.add_alias(entity.id, alias).await?;
        }
        let fresh = kg
            .get_entity(entity.id)
            .await?
            .ok_or_else(|| KnowledgeError::EntityNotFound(entity.id))?;
        return Ok((
            fresh,
            name_changed || type_changed || !aliases_missing.is_empty(),
        ));
    }
    Ok((
        entity.clone(),
        name_changed || type_changed || !aliases_missing.is_empty(),
    ))
}

fn normalized_fact(
    subject: &Entity,
    line: &ParsedFactLine,
    object_name: String,
    object_id: Option<i32>,
    relative_path: &str,
) -> NormalizedFact {
    let requires_user_action =
        matches!(line.event_type, Some(EventType::Task | EventType::Deadline));
    NormalizedFact {
        source_type: SourceType::Import,
        subject: subject.name.clone(),
        subject_type: EntityType::try_from(subject.entity_type_id).unwrap_or(EntityType::Concept),
        relationship_type: line.predicate.clone(),
        object: object_name,
        object_is_entity: object_id.is_some(),
        object_type: None,
        valid_from: line.valid_from,
        valid_until: line.valid_until,
        is_sensitive: true, // producer flag; Rust's AND gate decides (narrows only)
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence: line.recurrence,
        requires_user_action,
        raw_reference: Some(format!("obsidian:{relative_path}")),
        extraction_method: Some(ExtractionMethod::StructuredParse),
        event_type: line.event_type,
        location: None,
        confidence: line.confidence,
    }
}

/// The literal object text of a fact line.
fn object_literal_of(line: &ParsedFactLine) -> &str {
    match &line.object {
        ObsidianObject::Literal(value) => value,
        ObsidianObject::Entity(_) => "",
    }
}
