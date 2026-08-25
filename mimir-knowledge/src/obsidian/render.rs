//! Export rendering: Knowledge Graph → Obsidian Markdown documents.

use std::collections::{HashMap, HashSet};

use crate::KnowledgeGraph;
use crate::models::entity::{Entity, EntityType};
use crate::models::event::Event;
use crate::models::fact::FactStatus;
use crate::models::preference::PreferenceCategory;
use crate::queries;

use super::grammar::{
    Frontmatter, ObsidianObject, SECTION_DATES, SECTION_FACTS, SECTION_PREFERENCES,
    SECTION_RELATIONSHIPS, render_date, render_fact_line, render_preference_line,
};

/// One rendered Markdown document in the export bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObsidianExport {
    /// Rendered documents, ordered by their relative path.
    pub files: Vec<super::ObsidianFile>,
    pub entity_count: usize,
    pub fact_count: usize,
    pub preference_count: usize,
    pub event_count: usize,
}

/// One rendered document plus its per-section counts.
struct RenderedDocument {
    relative_path: String,
    content: String,
    facts: usize,
    preferences: usize,
    events: usize,
}

/// Sanitise an entity name into a safe file stem: Obsidian-incompatible
/// characters (`/\:*?"<>|#^[]`) and control characters become `-`, trailing
/// dots/spaces are trimmed, and the stem is capped at 200 chars.
fn sanitize_stem(name: &str) -> String {
    let mut stem = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_control() || "\\/:*?\"<>|#^[]".contains(ch) {
            stem.push('-');
        } else {
            stem.push(ch);
        }
    }
    let stem = stem
        .trim_end_matches(['.', ' ', '-'])
        .trim_start_matches('-');
    let mut stem = stem.to_string();
    if stem.is_empty() {
        stem = "entity".to_string();
    }
    if stem.chars().count() > 200 {
        stem = stem.chars().take(200).collect();
    }
    stem
}

/// Build the export bundle for the whole graph.
pub(crate) async fn render_all(
    kg: &KnowledgeGraph,
) -> Result<ObsidianExport, crate::KnowledgeError> {
    let entities = queries::entity::list_all(kg.pool()).await?;
    let predicate_names = relationship_type_names(kg).await?;
    let mut used_stems: HashSet<String> = HashSet::with_capacity(entities.len());
    let mut files = Vec::with_capacity(entities.len());

    let mut entity_count = 0;
    let mut fact_count = 0;
    let mut preference_count = 0;
    let mut event_count = 0;

    for entity in entities {
        let rendered = render_document(kg, &entity, &predicate_names, &mut used_stems).await?;
        entity_count += 1;
        fact_count += rendered.facts;
        preference_count += rendered.preferences;
        event_count += rendered.events;
        files.push(super::ObsidianFile {
            relative_path: rendered.relative_path,
            content: rendered.content,
        });
    }

    // The doc comment promises relative-path order; entity-list order (name,
    // id) does not match it once stems are sanitised and collisions suffixed.
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    Ok(ObsidianExport {
        files,
        entity_count,
        fact_count,
        preference_count,
        event_count,
    })
}

/// Render one entity document.
///
/// Section split: facts with an event overlay → `Dates`; entity-object facts
/// → `Relationships`; literal-object facts → `Facts`; entity-scoped
/// preferences → `Preferences`.
async fn render_document(
    kg: &KnowledgeGraph,
    entity: &Entity,
    predicate_names: &HashMap<i16, String>,
    used_stems: &mut HashSet<String>,
) -> Result<RenderedDocument, crate::KnowledgeError> {
    let entity_type = EntityType::try_from(entity.entity_type_id)
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|_| "Concept".to_string());
    let aliases: Vec<String> = entity
        .aliases
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();

    let facts = kg.get_facts_by_subject(entity.id, i64::MAX).await?;
    let events: HashMap<i32, Event> = queries::event::get_events_by_entity(kg.pool(), entity.id)
        .await?
        .into_iter()
        .map(|e| (e.fact_id, e))
        .collect();
    let prefs = queries::preference::get_preferences_for_entity(kg.pool(), entity.id).await?;

    let object_ids: Vec<u32> = facts
        .iter()
        .filter_map(|f| f.object_id)
        .map(|id| id as u32)
        .collect();
    let object_names = queries::entity::get_entity_names(kg.pool(), &object_ids).await?;

    let stem = sanitize_stem(&entity.name);
    let relative_path = if used_stems.insert(stem.clone()) {
        format!("{stem}.md")
    } else {
        // Deterministic collision suffix: same stem, different entity.
        format!("{stem}-{}.md", entity.id)
    };

    let frontmatter = Frontmatter {
        entity_id: Some(entity.id),
        entity_type: Some(entity_type),
        aliases,
        created: Some(render_date(entity.created_at)),
        updated: Some(render_date(entity.updated_at)),
    };

    let mut dates = String::new();
    let mut relationships = String::new();
    let mut fact_lines = String::new();
    let mut dates_rendered = 0usize;
    let mut relationships_rendered = 0usize;
    let mut facts_rendered = 0usize;

    for fact in &facts {
        if fact.status() == Some(FactStatus::Forgotten) {
            continue;
        }
        let Some(predicate) = predicate_names.get(&fact.relationship_type_id) else {
            continue;
        };
        let event = events.get(&fact.id);
        let object = if let Some(object_id) = fact.object_id {
            object_names
                .get(&(object_id as u32))
                .map(|name| ObsidianObject::Entity(name.clone()))
                .unwrap_or_else(|| ObsidianObject::Literal(format!("(entity {object_id})")))
        } else {
            ObsidianObject::Literal(fact.object_literal.clone().unwrap_or_default())
        };
        let line = render_fact_line(
            predicate,
            &object,
            fact.valid_from,
            fact.valid_until,
            fact.confidence,
            event
                .and_then(|e| e.recurrence())
                .unwrap_or(crate::models::enums::RecurrenceType::None),
            event.and_then(|e| e.event_type()),
        );
        if event.is_some() {
            dates.push_str(&format!("{line}\n"));
            dates_rendered += 1;
        } else if matches!(object, ObsidianObject::Entity(_)) {
            relationships.push_str(&format!("{line}\n"));
            relationships_rendered += 1;
        } else {
            fact_lines.push_str(&format!("{line}\n"));
            facts_rendered += 1;
        }
    }

    let mut preferences = String::new();
    for pref in &prefs {
        let category =
            PreferenceCategory::try_from(pref.category_id).unwrap_or(PreferenceCategory::General);
        preferences.push_str(&format!(
            "{}\n",
            render_preference_line(category, &pref.key, &pref.value)
        ));
    }

    let mut content = String::new();
    content.push_str("---\n");
    content.push_str(&frontmatter.render());
    content.push_str("---\n\n");
    content.push_str(&format!("# {}\n\n", entity.name));
    if dates_rendered > 0 {
        content.push_str(&format!("## {SECTION_DATES}\n{dates}\n"));
    }
    if relationships_rendered > 0 {
        content.push_str(&format!("## {SECTION_RELATIONSHIPS}\n{relationships}\n"));
    }
    if !prefs.is_empty() {
        content.push_str(&format!("## {SECTION_PREFERENCES}\n{preferences}\n"));
    }
    if facts_rendered > 0 {
        content.push_str(&format!("## {SECTION_FACTS}\n{fact_lines}\n"));
    }

    Ok(RenderedDocument {
        relative_path,
        content,
        facts: dates_rendered + relationships_rendered + facts_rendered,
        preferences: prefs.len(),
        events: dates_rendered,
    })
}

/// Relationship-type id → canonical name map for one export run.
async fn relationship_type_names(
    kg: &KnowledgeGraph,
) -> Result<HashMap<i16, String>, crate::KnowledgeError> {
    let rows: Vec<(i16, String)> = sqlx::query_as("SELECT id, name FROM relationship_types")
        .fetch_all(kg.pool())
        .await?;
    Ok(rows.into_iter().collect())
}
