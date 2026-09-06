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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::models::audit_log::ChangedBy;
use crate::models::entity::{Entity, EntityType};
use crate::models::enums::EventType;
use crate::models::preference::{
    NewPreference, PreferenceSourceType, UpsertAction, UpsertPreferenceInput,
};
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

#[derive(Default)]
struct ObjectResolutionCache {
    resolved: HashMap<String, ResolvedEntity>,
    allowed_object_types: HashMap<(i16, i16), EntityType>,
}

struct ResolvedEntity {
    id: i32,
    name: String,
}

impl ObjectResolutionCache {
    fn get_resolved(&self, name: &str) -> Option<(String, i32)> {
        self.resolved
            .get(&name.to_ascii_lowercase())
            .map(|entity| (entity.name.clone(), entity.id))
    }

    /// Only cache canonical names. Alias matches can depend on the
    /// relationship-derived object type, so they must not be reused for a
    /// different object-type constraint with the same alias text.
    fn remember(&mut self, name: &str, entity: &Entity) {
        if entity.name.eq_ignore_ascii_case(name) {
            self.resolved.insert(
                name.to_ascii_lowercase(),
                ResolvedEntity {
                    id: entity.id,
                    name: entity.name.clone(),
                },
            );
        }
    }

    fn remove_entity(&mut self, entity_id: i32) {
        self.resolved.retain(|_, entity| entity.id != entity_id);
    }
}

async fn allowed_object_type(
    kg: &KnowledgeGraph,
    cache: &mut ObjectResolutionCache,
    relationship_type_id: i16,
    subject_type: EntityType,
) -> Result<EntityType, KnowledgeError> {
    if let Some(entity_type) = cache
        .allowed_object_types
        .get(&(relationship_type_id, subject_type as i16))
    {
        return Ok(*entity_type);
    }

    let allowed_type: Option<i16> = sqlx::query_scalar(
        "SELECT allowed_object_type_id \
         FROM relationship_constraints \
         WHERE relationship_type_id = ? AND allowed_subject_type_id = ? \
         ORDER BY allowed_object_type_id LIMIT 1",
    )
    .bind(relationship_type_id)
    .bind(subject_type as i16)
    .fetch_optional(kg.pool())
    .await?;
    let entity_type = allowed_type
        .and_then(|type_id| EntityType::try_from(type_id).ok())
        .unwrap_or(EntityType::Concept);
    cache
        .allowed_object_types
        .insert((relationship_type_id, subject_type as i16), entity_type);
    Ok(entity_type)
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
/// Hidden entries are skipped; non-markdown files are ignored; symlinked
/// directories are followed but already-visited directories are pruned so a
/// cycle (e.g. `vault/loop -> vault`) terminates instead of recursing forever.
pub fn scan_markdown_files(dir: &Path) -> Result<Vec<PathBuf>, KnowledgeError> {
    let mut out = Vec::new();
    // Canonicalised visited directories: a symlinked subdirectory that points
    // at an ancestor (e.g. `vault/loop -> vault`) must not recurse forever.
    let mut visited_dirs: HashSet<PathBuf> = HashSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let canonical = std::fs::canonicalize(&current).unwrap_or_else(|_| current.clone());
        if !visited_dirs.insert(canonical) {
            continue;
        }
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
    // Entity names planned to be created, so a name referenced from several
    // files/lines is counted once — the exact set an apply would create.
    let mut planned_new_entities: HashSet<String> = HashSet::new();
    let mut object_cache = ObjectResolutionCache::default();

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

        if let Err(error) = import_document(
            kg,
            file,
            document,
            dry_run,
            &mut outcome,
            &mut planned_new_entities,
            &mut object_cache,
        )
        .await
        {
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
    /// `Some` only when the document has an explicit `# heading`; a
    /// heading-less note falls back to the file stem for resolution/creation
    /// but never renames an existing entity.
    heading: Option<String>,
    /// `None` when the frontmatter omitted `type` — only an explicitly
    /// declared type is applied to an existing entity.
    entity_type: Option<EntityType>,
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
        .transpose()?;

    let mut heading: Option<String> = None;
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
        if let Some(raw_heading) = trimmed.strip_prefix("# ") {
            if !raw_heading.starts_with('#') {
                let trimmed_heading = raw_heading.trim();
                if !trimmed_heading.is_empty() {
                    heading = Some(trimmed_heading.to_string());
                }
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

    let name = heading
        .clone()
        .unwrap_or_else(|| entity_name_from_path(&file.relative_path));

    Ok(ParsedDocument {
        frontmatter,
        name,
        heading,
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
    planned_new_entities: &mut HashSet<String>,
    object_cache: &mut ObjectResolutionCache,
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
                    object_cache.remove_entity(entity.id);
                    outcome.counts.entities_updated += 1;
                }
                Some(entity)
            }
            None => {
                if planned_new_entities.insert(document.name.to_lowercase()) {
                    outcome.counts.entities_new += 1;
                }
                if dry_run {
                    None
                } else {
                    Some(
                        kg.create_entity(
                            &document.name,
                            document.entity_type.unwrap_or(EntityType::Concept),
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
        let (resolved, created) = resolve_subject(
            kg,
            &document.name,
            document.entity_type.unwrap_or(EntityType::Concept),
            dry_run,
        )
        .await?;
        if created {
            if planned_new_entities.insert(document.name.to_lowercase()) {
                outcome.counts.entities_new += 1;
            }
            resolved
        } else if let Some(entity) = resolved {
            let (entity, changed) = apply_entity_updates(kg, &entity, &document, dry_run).await?;
            if changed {
                object_cache.remove_entity(entity.id);
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
        let relationship_type_id = match kg
            .resolve_emit_eligible_relationship_type(&line.predicate)
            .await
        {
            Ok(Some(relationship_type_id)) => relationship_type_id,
            Ok(None) => {
                outcome.errors.push(format!(
                    "{}: predicate '{}' is not an emit-eligible taxonomy leaf",
                    file.relative_path, line.predicate
                ));
                continue;
            }
            Err(error) => {
                outcome
                    .errors
                    .push(format!("{}: {error}", file.relative_path));
                continue;
            }
        };
        let mut context = ObjectPlanningContext {
            relationship_type_id,
            subject_type: subject.as_ref().map_or(
                document.entity_type.unwrap_or(EntityType::Concept),
                |entity| EntityType::try_from(entity.entity_type_id).unwrap_or(EntityType::Concept),
            ),
            dry_run,
            counts: &mut outcome.counts,
            planned_new_entities,
            object_cache,
        };
        let (object_name, object_id) = plan_object(kg, &line.object, &mut context).await?;
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
        let Some(entity) = &subject else {
            // A brand-new subject has no preferences to compare against.
            outcome.counts.preferences_new += 1;
            continue;
        };
        let existing = kg
            .get_preference(Some(entity.id), &preference.key, &[])
            .await?;
        let mut upsert = true;
        let mut pre_counted = false;
        match existing {
            None => {
                outcome.counts.preferences_new += 1;
                pre_counted = true;
            }
            Some(existing) => {
                // The upsert matches on an identical (empty) context set, so a
                // context-scoped existing preference is no conflict — the
                // import inserts a new default row.
                let contexts =
                    queries::preference::get_contexts_for_preference(kg.pool(), existing.id)
                        .await?;
                if !contexts.is_empty() {
                    outcome.counts.preferences_new += 1;
                    pre_counted = true;
                } else if existing.value == preference.value {
                    // Unchanged value: idempotent re-import — nothing to
                    // overwrite, no audit-log churn.
                    upsert = false;
                } else if dry_run {
                    // Mirror the upsert conflict rules (1/3/4) so dry-run
                    // counts and reports match what an apply would do: a
                    // user-set or equal/higher-confidence preference keeps
                    // its value, and the changed vault value is reported.
                    if existing.overridden_by_user || existing.confidence >= 0.80 {
                        outcome.errors.push(format!(
                            "{}: preference {} not applied — kept existing value {:?}",
                            file.relative_path, preference.key, existing.value
                        ));
                    } else {
                        outcome.counts.preferences_updated += 1;
                    }
                    upsert = false;
                }
                // Apply mode with a changed value falls through to the upsert,
                // which reports the real outcome below.
            }
        }
        if dry_run || !upsert {
            continue;
        }
        let (_, action) = kg
            .upsert_preference(UpsertPreferenceInput {
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
        match action {
            UpsertAction::Overwritten => outcome.counts.preferences_updated += 1,
            UpsertAction::Created if !pre_counted => outcome.counts.preferences_new += 1,
            UpsertAction::Created => {}
            UpsertAction::Rejected | UpsertAction::KeptAsPrimary => {
                // The conflict policy kept the stored value; surface the
                // changed vault value instead of skipping it silently.
                outcome.errors.push(format!(
                    "{}: preference {} not applied — kept existing value (upsert {action:?})",
                    file.relative_path, preference.key
                ));
            }
        }
    }

    Ok(())
}

/// Per-fact planning inputs and mutable accounting state for object lookup.
struct ObjectPlanningContext<'a> {
    relationship_type_id: i16,
    subject_type: EntityType,
    dry_run: bool,
    counts: &'a mut ObsidianImportCounts,
    planned_new_entities: &'a mut HashSet<String>,
    object_cache: &'a mut ObjectResolutionCache,
}

/// Resolve a fact line's object entity and update the entity-creation count.
///
/// Returns the object's canonical name and id (`None` id when the object is a
/// literal, or when the entity would be created in dry-run mode).
async fn plan_object(
    kg: &KnowledgeGraph,
    object: &ObsidianObject,
    context: &mut ObjectPlanningContext<'_>,
) -> Result<(String, Option<i32>), KnowledgeError> {
    match object {
        ObsidianObject::Literal(value) => Ok((value.clone(), None)),
        ObsidianObject::Entity(name) => {
            // Canonical resolution is type-filtered (issue #182); the object
            // type is not recorded in the file, so the Concept filter can
            // miss a same-name entity of another type (e.g. a `Person`
            // `[[Alice]]`). `create_entity`'s upsert reuses that entity, so
            // the exact same-name fallback keeps dry-run and apply identical.
            if let Some((canonical_name, id)) = context.object_cache.get_resolved(name) {
                return Ok((canonical_name, Some(id)));
            }
            let results =
                queries::entity::get_by_name_typed(kg.pool(), name, EntityType::Concept).await?;
            if let Some(entity) = pick_resolution(&results) {
                context.object_cache.remember(name, entity);
                return Ok((entity.name.clone(), Some(entity.id)));
            }
            if let Some(entity) = queries::entity::get_exact_name(kg.pool(), name).await? {
                context.object_cache.remember(name, &entity);
                return Ok((entity.name.clone(), Some(entity.id)));
            }
            let object_type = allowed_object_type(
                kg,
                context.object_cache,
                context.relationship_type_id,
                context.subject_type,
            )
            .await?;

            if context.dry_run && object_type != EntityType::Concept {
                let results =
                    queries::entity::get_by_name_typed(kg.pool(), name, object_type).await?;
                if let Some(entity) = pick_resolution(&results) {
                    context.object_cache.remember(name, entity);
                    return Ok((entity.name.clone(), Some(entity.id)));
                }
            }

            if context.dry_run {
                if context.planned_new_entities.insert(name.to_lowercase()) {
                    context.counts.entities_new += 1;
                }
                Ok((name.clone(), None))
            } else {
                let (entity, created) = resolve_or_create(kg, name, object_type).await?;
                if context.planned_new_entities.insert(name.to_lowercase()) && created {
                    context.counts.entities_new += 1;
                }
                context.object_cache.remember(name, &entity);
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
    // Only an explicitly declared frontmatter `type` is an instruction:
    // a note without `type:` retains the stored entity type.
    let type_changed = document
        .entity_type
        .is_some_and(|declared| EntityType::try_from(entity.entity_type_id).ok() != Some(declared));
    // Only an explicit `# heading` renames: a heading-less note (file-stem
    // fallback) never renames the stored entity.
    let name_changed = document
        .heading
        .as_ref()
        .is_some_and(|heading| entity.name != *heading);
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
            let name = document.heading.as_deref().unwrap_or(&entity.name);
            let entity_type = document.entity_type.unwrap_or_else(|| {
                EntityType::try_from(entity.entity_type_id).unwrap_or(EntityType::Concept)
            });
            kg.update_entity(entity.id, name, entity_type).await?;
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
        // The Obsidian format carries the recurrence kind only.
        recurrence_rule: None,
        recurrence_interval: 1,
        recurrence_until: None,
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
