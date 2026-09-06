//! Integration tests for the Obsidian export/import subsystem (issue #62).
//!
//! Covers the canonical Markdown format (YAML frontmatter + wiki-links +
//! section grammar), export rendering, import planning/application, dry-run
//! semantics, entity-id round-tripping, sensitivity gating, and the
//! export → import round trip.

use chrono::{DateTime, Utc};

use mimir_knowledge::KnowledgeGraph;
use mimir_knowledge::models::audit_log::ChangedBy;
use mimir_knowledge::models::entity::EntityType;
use mimir_knowledge::models::enums::{EventType, RecurrenceType};
use mimir_knowledge::models::preference::{
    NewPreference, PreferenceSourceType, UpsertPreferenceInput,
};
use mimir_knowledge::models::source::{ExtractionMethod, SourceType};
use mimir_knowledge::normalize::{NormalizedFact, Provenance, normalize_and_insert};
use mimir_knowledge::obsidian::{ObsidianFile, scan_markdown_files};

/// Fresh KnowledgeGraph in a temp dir.
async fn fresh_kg() -> (KnowledgeGraph, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kg = KnowledgeGraph::init(&dir.path().join("obsidian_test.db"))
        .await
        .unwrap();
    (kg, dir)
}

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone::<Utc>(&Utc)
}

/// Build a normalized fact with the shared defaults for seeding.
#[allow(clippy::too_many_arguments)]
fn seed_fact(
    subject: &str,
    relationship_type: &str,
    object: &str,
    object_is_entity: bool,
    object_type: EntityType,
    recurrence: RecurrenceType,
    event_type: Option<EventType>,
    valid_from: Option<DateTime<Utc>>,
    source_type: SourceType,
) -> NormalizedFact {
    NormalizedFact {
        source_type,
        subject: subject.to_string(),
        subject_type: EntityType::Person,
        relationship_type: relationship_type.to_string(),
        object: object.to_string(),
        object_is_entity,
        object_type: Some(object_type),
        valid_from,
        valid_until: None,
        is_sensitive: false,
        is_correction: false,
        correction_scope: None,
        category_ids: Vec::new(),
        recurrence,
        recurrence_rule: None,
        recurrence_interval: 1,
        recurrence_until: None,
        requires_user_action: false,
        raw_reference: None,
        extraction_method: Some(ExtractionMethod::UserInput),
        event_type,
        location: None,
        confidence: None,
    }
}

#[tokio::test]
async fn export_renders_frontmatter_sections_wiki_links_and_attributes() {
    let (kg, _dir) = fresh_kg().await;
    let facts = vec![
        seed_fact(
            "Devansh",
            "has_partner",
            "Alice",
            true,
            EntityType::Person,
            RecurrenceType::None,
            None,
            Some(dt("2022-01-01T00:00:00Z")),
            SourceType::UserEdit,
        ),
        seed_fact(
            "Devansh",
            "visited",
            "Rome",
            true,
            EntityType::Place,
            RecurrenceType::None,
            None,
            Some(dt("2025-05-03T00:00:00Z")),
            SourceType::UserEdit,
        ),
        seed_fact(
            "Devansh",
            "allergy",
            "peanuts",
            false,
            EntityType::Concept,
            RecurrenceType::None,
            None,
            None,
            SourceType::UserEdit,
        ),
        seed_fact(
            "Devansh",
            "born_on",
            "1995-08-20",
            false,
            EntityType::Concept,
            RecurrenceType::Yearly,
            Some(EventType::Birthday),
            Some(dt("1995-08-20T00:00:00Z")),
            SourceType::UserEdit,
        ),
    ];
    let outcome = normalize_and_insert(&kg, facts, Provenance::chat(ExtractionMethod::UserInput))
        .await
        .unwrap();
    assert_eq!(outcome.errors.len(), 0);
    // Give Devansh an alias so the frontmatter renders it.
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.entity.name == "Devansh")
        .map(|e| e.entity)
        .unwrap();
    kg.add_alias(devansh.id, "Dev").await.unwrap();

    let export = kg.export_obsidian().await.unwrap();
    let file = export
        .files
        .iter()
        .find(|f| f.relative_path == "Devansh.md")
        .expect("Devansh.md in export");

    let content = &file.content;
    assert!(content.contains("entity_id: "), "frontmatter entity_id");
    assert!(content.contains("type: Person"), "frontmatter type");
    assert!(content.contains("aliases:"), "frontmatter aliases");
    assert!(content.contains("Dev"), "frontmatter alias value");
    assert!(content.contains("# Devansh\n"), "H1 heading");

    assert!(content.contains("## Dates"), "Dates section");
    assert!(
        content.contains("## Relationships"),
        "Relationships section"
    );
    assert!(content.contains("## Facts"), "Facts section");

    // Dates: event overlay facts carry event type + recurrence.
    let dates_line = content
        .lines()
        .find(|l| l.contains("born_on") && l.contains("1995-08-20"))
        .expect("born_on date line");
    assert!(dates_line.contains("Birthday"), "event type: {dates_line}");
    assert!(dates_line.contains("yearly"), "recurrence: {dates_line}");

    // Relationships: entity objects render as wiki-links with bounds.
    let rel_line = content
        .lines()
        .find(|l| l.contains("has_partner") && l.contains("[[Alice]]"))
        .expect("has_partner wiki-link line");
    assert!(rel_line.contains("since 2022-01-01"), "bounds: {rel_line}");

    // Facts: literal objects stay plain, confidence rendered.
    let fact_line = content
        .lines()
        .find(|l| l.contains("allergy") && l.contains("peanuts"))
        .expect("allergy line");
    assert!(fact_line.contains("confidence:"), "confidence: {fact_line}");

    assert_eq!(export.entity_count, 3, "Devansh, Alice, Rome");
    assert_eq!(export.preference_count, 0);
    assert_eq!(export.event_count, 1);
}

#[tokio::test]
async fn export_sanitizes_unsafe_file_names_and_deduplicates_collisions() {
    let (kg, _dir) = fresh_kg().await;
    let facts = vec![
        seed_fact(
            "Dev/ansh",
            "has_name",
            "x",
            false,
            EntityType::Concept,
            RecurrenceType::None,
            None,
            None,
            SourceType::UserEdit,
        ),
        seed_fact(
            "Dev?ansh",
            "has_name",
            "y",
            false,
            EntityType::Concept,
            RecurrenceType::None,
            None,
            None,
            SourceType::UserEdit,
        ),
    ];
    let _ = normalize_and_insert(&kg, facts, Provenance::chat(ExtractionMethod::UserInput))
        .await
        .unwrap();

    let export = kg.export_obsidian().await.unwrap();
    let names: Vec<&str> = export
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    // Both names sanitise to `Dev-ansh.md`, so the collision suffix keeps them distinct.
    assert!(
        names.contains(&"Dev-ansh.md"),
        "plain sanitised name: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.starts_with("Dev-ansh-")),
        "collision-suffixed name: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains('/') || n.contains('?')),
        "no raw separators"
    );
}

#[tokio::test]
async fn import_creates_entities_facts_events_and_preferences() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
aliases: ["Dev"]
---

# Devansh

## Dates
- birthday → 1995-08-20 (1995-08-20, Birthday, yearly)

## Relationships
- has_partner → Alice (since 2022-01-01)

## Preferences
- FoodPreference: favourite = Italian

## Facts
- allergic_to → peanuts (confidence: 1.0)
"#
        .to_string(),
    };

    let outcome = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(outcome.errors.len(), 0, "errors: {:?}", outcome.errors);
    assert!(!outcome.dry_run);
    assert_eq!(outcome.counts.entities_new, 2, "Devansh + Alice");
    assert_eq!(outcome.counts.facts_new, 3);
    assert_eq!(outcome.counts.dates_new, 1);
    assert_eq!(outcome.counts.preferences_new, 1);

    // Devansh exists with alias + Import facts.
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.entity.name == "Devansh")
        .map(|e| e.entity)
        .expect("Devansh entity");
    let facts = kg.get_facts_by_subject(devansh.id, 100).await.unwrap();
    assert_eq!(facts.len(), 3, "facts: {facts:?}");

    let married = facts.iter().find(|f| f.object_id.is_some()).unwrap();
    let sources = kg.get_sources_for_fact(married.id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_type_id, SourceType::Import as i16);
    assert!(
        (married.confidence - 0.80).abs() < 1e-6,
        "default import confidence"
    );

    let allergic = facts
        .iter()
        .find(|f| f.object_literal.as_deref() == Some("peanuts"))
        .unwrap();
    assert!(
        (allergic.confidence - 1.0).abs() < 1e-6,
        "explicit confidence honoured"
    );

    // Birthday fact has an event overlay.
    let birthday = facts
        .iter()
        .find(|f| f.object_literal.as_deref() == Some("1995-08-20"))
        .unwrap();
    let event = kg
        .get_event_by_fact(birthday.id)
        .await
        .unwrap()
        .expect("birthday overlay");
    assert_eq!(event.event_type(), Some(EventType::Birthday));
    assert_eq!(event.recurrence(), Some(RecurrenceType::Yearly));

    // Preference upserted scoped to the entity.
    let prefs =
        mimir_knowledge::queries::preference::get_preferences_for_entity(kg.pool(), devansh.id)
            .await
            .unwrap();
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0].key, "favourite");
    assert_eq!(prefs[0].value, "Italian");
}

#[tokio::test]
async fn import_dry_run_reports_counts_without_writing() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Facts
- allergic_to → peanuts
"#
        .to_string(),
    };

    let outcome = kg.import_obsidian(&[file], true).await.unwrap();
    assert!(outcome.dry_run);
    assert_eq!(outcome.counts.entities_new, 1);
    assert_eq!(outcome.counts.facts_new, 1);
    assert_eq!(outcome.errors.len(), 0);

    // Nothing was written.
    let entities = kg.search_entities("Devansh", 10).await.unwrap();
    assert!(entities.is_empty(), "dry-run must not create entities");
}

#[tokio::test]
async fn import_round_trips_an_export() {
    let (kg, _dir) = fresh_kg().await;
    let facts = vec![
        seed_fact(
            "Devansh",
            "has_partner",
            "Alice",
            true,
            EntityType::Person,
            RecurrenceType::None,
            None,
            Some(dt("2022-01-01T00:00:00Z")),
            SourceType::UserEdit,
        ),
        // Non-explicit confidence must survive the round trip.
        seed_fact(
            "Devansh",
            "allergic_to",
            "peanuts",
            false,
            EntityType::Concept,
            RecurrenceType::None,
            None,
            None,
            SourceType::Interaction,
        ),
        seed_fact(
            "Devansh",
            "birthday",
            "1995-08-20",
            false,
            EntityType::Concept,
            RecurrenceType::Yearly,
            Some(EventType::Birthday),
            Some(dt("1995-08-20T00:00:00Z")),
            SourceType::UserEdit,
        ),
    ];
    let _ = normalize_and_insert(&kg, facts, Provenance::chat(ExtractionMethod::UserInput))
        .await
        .unwrap();

    let export = kg.export_obsidian().await.unwrap();
    let files: Vec<ObsidianFile> = export
        .files
        .into_iter()
        .map(|f| ObsidianFile {
            relative_path: f.relative_path,
            content: f.content,
        })
        .collect();

    let (kg2, _dir2) = fresh_kg().await;
    let outcome = kg2.import_obsidian(&files, false).await.unwrap();
    assert_eq!(outcome.errors.len(), 0, "errors: {:?}", outcome.errors);

    let devansh = kg2
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.entity.name == "Devansh")
        .map(|e| e.entity)
        .unwrap();
    let facts2 = kg2.get_facts_by_subject(devansh.id, 100).await.unwrap();
    assert_eq!(facts2.len(), 3, "facts: {facts2:?}");

    let married = facts2.iter().find(|f| f.object_id.is_some()).unwrap();
    assert_eq!(married.confidence, 1.0, "explicit confidence preserved");
    let allergic = facts2
        .iter()
        .find(|f| f.object_literal.as_deref() == Some("peanuts"))
        .unwrap();
    assert!(
        (allergic.confidence - 0.30).abs() < 1e-6,
        "non-explicit confidence preserved: {}",
        allergic.confidence
    );
    let birthday = facts2
        .iter()
        .find(|f| f.object_literal.as_deref() == Some("1995-08-20"))
        .unwrap();
    assert_eq!(
        kg2.get_event_by_fact(birthday.id)
            .await
            .unwrap()
            .unwrap()
            .event_type(),
        Some(EventType::Birthday)
    );
}

#[tokio::test]
async fn import_uses_entity_id_and_updates_changed_entities() {
    let (kg, _dir) = fresh_kg().await;
    let first = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh
"#
        .to_string(),
    };
    let outcome = kg.import_obsidian(&[first], false).await.unwrap();
    assert_eq!(outcome.counts.entities_new, 1);
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;

    let second = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: format!(
            "---\nentity_id: {}\ntype: Person\n---\n\n# Devansh Bhavsar\n",
            devansh.id
        ),
    };
    let outcome = kg.import_obsidian(&[second], false).await.unwrap();
    assert_eq!(outcome.counts.entities_new, 0);
    assert_eq!(outcome.counts.entities_updated, 1);
    let renamed = kg.get_entity(devansh.id).await.unwrap().unwrap();
    assert_eq!(renamed.name, "Devansh Bhavsar");
}

#[tokio::test]
async fn import_skips_existing_triples() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Facts
- allergic_to → peanuts (confidence: 1.0)
"#
        .to_string(),
    };
    let first = kg
        .import_obsidian(std::slice::from_ref(&file), false)
        .await
        .unwrap();
    assert_eq!(first.counts.facts_new, 1);

    let second = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(second.counts.entities_new, 0);
    assert_eq!(second.counts.facts_new, 0);
    assert_eq!(second.counts.facts_existing, 1, "existing triple skipped");
}

#[tokio::test]
async fn import_flags_sensitive_facts_for_confirmation() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Facts
- health_condition → diabetes (confidence: 0.9)
"#
        .to_string(),
    };
    let outcome = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(outcome.errors.len(), 0);
    let pending = kg.list_pending_facts().await.unwrap();
    assert_eq!(pending.len(), 1, "sensitive import must await confirmation");
}

#[tokio::test]
async fn import_accepts_em_dash_relationship_form() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Relationships
- [[Alice]] — has_partner (since 2022-01-01)
"#
        .to_string(),
    };
    let outcome = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(outcome.errors.len(), 0, "errors: {:?}", outcome.errors);
    assert_eq!(outcome.counts.facts_new, 1);
}

#[tokio::test]
async fn scan_markdown_files_collects_md_recursively() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("a.md"), "# A").unwrap();
    std::fs::write(dir.path().join("sub/b.md"), "# B").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not markdown").unwrap();

    let files = scan_markdown_files(dir.path()).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(dir.path()).unwrap().display().to_string())
        .collect();
    assert!(names.contains(&"a.md".to_string()));
    assert!(names.contains(&"sub/b.md".to_string()));
    assert!(!names.iter().any(|n| n.ends_with(".txt")));
}

#[tokio::test]
async fn import_dry_run_matches_apply_for_entity_objects() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Relationships
- has_partner → [[Alice]] (since 2022-01-01)
"#
        .to_string(),
    };

    // First apply creates Devansh + Alice (a Person — the object type is not
    // recorded in the file, so the Concept-filtered chain falls back to the
    // exact same-name entity instead of double-counting a phantom "new").
    let first = kg
        .import_obsidian(std::slice::from_ref(&file), false)
        .await
        .unwrap();
    assert_eq!(first.counts.entities_new, 2, "{:?}", first.counts);
    assert_eq!(first.counts.facts_new, 1);

    // Dry-run of the same files must report exactly what an apply would do.
    let dry = kg
        .import_obsidian(std::slice::from_ref(&file), true)
        .await
        .unwrap();
    assert_eq!(dry.counts.entities_new, 0, "{:?}", dry.counts);
    assert_eq!(dry.counts.entities_updated, 0, "{:?}", dry.counts);
    assert_eq!(dry.counts.facts_new, 0, "{:?}", dry.counts);
    assert_eq!(dry.counts.facts_existing, 1, "{:?}", dry.counts);

    // Apply agrees with the dry-run.
    let second = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(second.counts.entities_new, 0, "{:?}", second.counts);
    assert_eq!(second.counts.facts_new, 0);
    assert_eq!(second.counts.facts_existing, 1);
}

#[tokio::test]
async fn import_dry_run_counts_each_new_entity_once() {
    let (kg, _dir) = fresh_kg().await;
    // The same new subject spans two files and the same new object is
    // referenced from two lines: dry-run must report each distinct new
    // entity exactly once, matching what an apply would create.
    let first = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Relationships
- has_partner → [[Alice]] (since 2022-01-01)
- has_sibling → [[Alice]]
"#
        .to_string(),
    };
    let second = ObsidianFile {
        relative_path: "sub/Devansh.md".to_string(),
        content: "---\ntype: Person\n---\n\n# Devansh\n".to_string(),
    };

    let dry = kg
        .import_obsidian(&[first.clone(), second.clone()], true)
        .await
        .unwrap();
    assert_eq!(
        dry.counts.entities_new, 2,
        "Devansh + Alice once each: {:?}",
        dry.counts
    );
    assert_eq!(dry.counts.facts_new, 2, "{:?}", dry.counts);

    let applied = kg.import_obsidian(&[first, second], false).await.unwrap();
    assert_eq!(
        applied.counts.entities_new, 2,
        "apply agrees: {:?}",
        applied.counts
    );
    assert_eq!(applied.counts.facts_new, 2, "{:?}", applied.counts);
}

#[tokio::test]
async fn import_reimport_unchanged_preferences_is_idempotent() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Preferences
- FoodPreference: favourite = Italian
"#
        .to_string(),
    };

    let first = kg
        .import_obsidian(std::slice::from_ref(&file), false)
        .await
        .unwrap();
    assert_eq!(first.counts.preferences_new, 1, "{:?}", first.counts);

    // Re-importing the untouched file changes nothing: no phantom "updated",
    // no duplicate row, no audit-log churn.
    let second = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(second.counts.preferences_new, 0, "{:?}", second.counts);
    assert_eq!(second.counts.preferences_updated, 0, "{:?}", second.counts);

    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;
    let prefs =
        mimir_knowledge::queries::preference::get_preferences_for_entity(kg.pool(), devansh.id)
            .await
            .unwrap();
    assert_eq!(prefs.len(), 1, "no duplicate preference rows");
    assert_eq!(prefs[0].value, "Italian");
}

#[tokio::test]
async fn import_updates_preference_when_import_confidence_wins() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Preferences
- FoodPreference: favourite = Italian
"#
        .to_string(),
    };
    // Seed Devansh with a lower-confidence preference so the import's 0.80
    // overwrites it (upsert rule 3).
    kg.import_obsidian(
        &[ObsidianFile {
            relative_path: "Devansh.md".to_string(),
            content: "---\ntype: Person\n---\n\n# Devansh\n".to_string(),
        }],
        false,
    )
    .await
    .unwrap();
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;
    kg.upsert_preference(UpsertPreferenceInput {
        preference: NewPreference {
            entity_id: Some(devansh.id),
            category: mimir_knowledge::models::preference::PreferenceCategory::FoodPreference,
            key: "favourite".to_string(),
            value: "French".to_string(),
            confidence: 0.50,
            overridden_by_user: false,
            source_fact_id: None,
        },
        changed_by: ChangedBy::User,
        contexts: Vec::new(),
        sources: vec![(PreferenceSourceType::UserEdit, "seed".to_string())],
    })
    .await
    .unwrap();

    let outcome = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(outcome.counts.preferences_new, 0, "{:?}", outcome.counts);
    assert_eq!(
        outcome.counts.preferences_updated, 1,
        "{:?}",
        outcome.counts
    );
    let prefs =
        mimir_knowledge::queries::preference::get_preferences_for_entity(kg.pool(), devansh.id)
            .await
            .unwrap();
    assert_eq!(prefs.len(), 1, "updated in place, no duplicate row");
    assert_eq!(prefs[0].value, "Italian");
}

#[cfg(unix)]
#[tokio::test]
async fn scan_markdown_files_avoids_symlink_cycles() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "# A").unwrap();
    std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();

    let files = scan_markdown_files(dir.path()).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(dir.path()).unwrap().display().to_string())
        .collect();
    assert_eq!(names, vec!["a.md".to_string()], "cycle must terminate");
}

#[tokio::test]
async fn export_files_are_ordered_by_relative_path() {
    let (kg, _dir) = fresh_kg().await;
    // `a#` sanitises to `a-` and then trims to `a`, colliding with the plain
    // `a`; name order (`a`, `a#`) differs from path order (`a-<id>.md`, `a.md`).
    let facts = vec![
        seed_fact(
            "a",
            "has_name",
            "x",
            false,
            EntityType::Concept,
            RecurrenceType::None,
            None,
            None,
            SourceType::UserEdit,
        ),
        seed_fact(
            "a#",
            "has_name",
            "y",
            false,
            EntityType::Concept,
            RecurrenceType::None,
            None,
            None,
            SourceType::UserEdit,
        ),
    ];
    let _ = normalize_and_insert(&kg, facts, Provenance::chat(ExtractionMethod::UserInput))
        .await
        .unwrap();

    let export = kg.export_obsidian().await.unwrap();
    let names: Vec<String> = export
        .files
        .iter()
        .map(|f| f.relative_path.clone())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "files must be ordered by relative path");
}

#[tokio::test]
async fn import_without_frontmatter_type_keeps_existing_entity_type() {
    let (kg, _dir) = fresh_kg().await;
    let first = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: "---\ntype: Person\n---\n\n# Devansh\n".to_string(),
    };
    kg.import_obsidian(&[first], false).await.unwrap();

    // A hand-written note with no frontmatter `type` must not retype the
    // stored Person to the Concept default.
    let second = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: "# Devansh\n\n## Facts\n- likes → coffee\n".to_string(),
    };
    let outcome = kg.import_obsidian(&[second], false).await.unwrap();
    assert_eq!(outcome.counts.entities_updated, 0, "{:?}", outcome.counts);
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;
    assert_eq!(
        EntityType::try_from(devansh.entity_type_id).unwrap(),
        EntityType::Person,
        "absent frontmatter type must not retype the entity"
    );
}

#[tokio::test]
async fn import_without_heading_does_not_rename_existing_entity() {
    let (kg, _dir) = fresh_kg().await;
    let first = ObsidianFile {
        relative_path: "Alice.md".to_string(),
        content: "---\ntype: Person\n---\n\n# Alice\n".to_string(),
    };
    kg.import_obsidian(&[first], false).await.unwrap();
    let alice = kg
        .search_entities("Alice", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;

    // The heading is removed; the file stem (`alice`) must not rename the
    // stored `Alice` entity.
    let second = ObsidianFile {
        relative_path: "alice.md".to_string(),
        content: "---\ntype: Person\n---\n\n## Facts\n- likes → coffee\n".to_string(),
    };
    let outcome = kg.import_obsidian(&[second], false).await.unwrap();
    assert_eq!(outcome.counts.entities_updated, 0, "{:?}", outcome.counts);
    let after = kg.get_entity(alice.id).await.unwrap().unwrap();
    assert_eq!(after.name, "Alice");
}

#[tokio::test]
async fn import_reports_conflict_when_changed_value_loses_to_stored_preference() {
    let (kg, _dir) = fresh_kg().await;
    kg.import_obsidian(
        &[ObsidianFile {
            relative_path: "Devansh.md".to_string(),
            content: "---\ntype: Person\n---\n\n# Devansh\n".to_string(),
        }],
        false,
    )
    .await
    .unwrap();
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;
    kg.upsert_preference(UpsertPreferenceInput {
        preference: NewPreference {
            entity_id: Some(devansh.id),
            category: mimir_knowledge::models::preference::PreferenceCategory::FoodPreference,
            key: "favourite".to_string(),
            value: "French".to_string(),
            confidence: 0.90,
            overridden_by_user: false,
            source_fact_id: None,
        },
        changed_by: ChangedBy::User,
        contexts: Vec::new(),
        sources: vec![(PreferenceSourceType::UserEdit, "seed".to_string())],
    })
    .await
    .unwrap();

    // The vault value changed to Italian, but the stored 0.90 preference
    // wins: the conflict must be reported, not silently skipped.
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Preferences
- FoodPreference: favourite = Italian
"#
        .to_string(),
    };
    let applied = kg
        .import_obsidian(std::slice::from_ref(&file), false)
        .await
        .unwrap();
    assert_eq!(
        applied.counts.preferences_updated, 0,
        "{:?}",
        applied.counts
    );
    assert!(
        applied
            .errors
            .iter()
            .any(|e| e.contains("favourite") && e.contains("not applied")),
        "conflict surfaced: {:?}",
        applied.errors
    );

    let dry = kg.import_obsidian(&[file], true).await.unwrap();
    assert_eq!(dry.counts.preferences_updated, 0, "{:?}", dry.counts);
    assert!(
        dry.errors
            .iter()
            .any(|e| e.contains("favourite") && e.contains("not applied")),
        "dry-run must predict the conflict: {:?}",
        dry.errors
    );

    let prefs =
        mimir_knowledge::queries::preference::get_preferences_for_entity(kg.pool(), devansh.id)
            .await
            .unwrap();
    assert_eq!(prefs[0].value, "French", "stored value kept");
}

#[tokio::test]
async fn import_reports_rejection_when_user_owns_the_preference() {
    let (kg, _dir) = fresh_kg().await;
    kg.import_obsidian(
        &[ObsidianFile {
            relative_path: "Devansh.md".to_string(),
            content: "---\ntype: Person\n---\n\n# Devansh\n".to_string(),
        }],
        false,
    )
    .await
    .unwrap();
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;
    kg.upsert_preference(UpsertPreferenceInput {
        preference: NewPreference {
            entity_id: Some(devansh.id),
            category: mimir_knowledge::models::preference::PreferenceCategory::FoodPreference,
            key: "favourite".to_string(),
            value: "French".to_string(),
            confidence: 1.0,
            overridden_by_user: true,
            source_fact_id: None,
        },
        changed_by: ChangedBy::User,
        contexts: Vec::new(),
        sources: vec![(PreferenceSourceType::UserEdit, "seed".to_string())],
    })
    .await
    .unwrap();

    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Preferences
- FoodPreference: favourite = Italian
"#
        .to_string(),
    };
    let outcome = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(
        outcome.counts.preferences_updated, 0,
        "{:?}",
        outcome.counts
    );
    assert!(
        outcome
            .errors
            .iter()
            .any(|e| e.contains("favourite") && e.contains("not applied")),
        "rejection surfaced: {:?}",
        outcome.errors
    );
    let prefs =
        mimir_knowledge::queries::preference::get_preferences_for_entity(kg.pool(), devansh.id)
            .await
            .unwrap();
    assert_eq!(prefs[0].value, "French", "user-set value kept");
}

#[tokio::test]
async fn import_does_not_create_entities_for_invalid_predicates() {
    let (kg, _dir) = fresh_kg().await;
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Facts
- owes → [[Dan]]
"#
        .to_string(),
    };

    let outcome = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(outcome.counts.facts_new, 0);
    assert!(
        outcome
            .errors
            .iter()
            .any(|error| error.contains("owes") && error.contains("emit-eligible")),
        "invalid predicate surfaced: {:?}",
        outcome.errors
    );
    assert!(
        kg.search_entities("Dan", 10).await.unwrap().is_empty(),
        "invalid predicate must not create an object entity"
    );
}

#[tokio::test]
async fn import_unchanged_low_confidence_preference_is_idempotent() {
    let (kg, _dir) = fresh_kg().await;
    kg.import_obsidian(
        &[ObsidianFile {
            relative_path: "Devansh.md".to_string(),
            content: "---\ntype: Person\n---\n\n# Devansh\n".to_string(),
        }],
        false,
    )
    .await
    .unwrap();
    let devansh = kg
        .search_entities("Devansh", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .entity;
    kg.upsert_preference(UpsertPreferenceInput {
        preference: NewPreference {
            entity_id: Some(devansh.id),
            category: mimir_knowledge::models::preference::PreferenceCategory::FoodPreference,
            key: "favourite".to_string(),
            value: "Italian".to_string(),
            confidence: 0.50,
            overridden_by_user: false,
            source_fact_id: None,
        },
        changed_by: ChangedBy::User,
        contexts: Vec::new(),
        sources: vec![(PreferenceSourceType::UserEdit, "seed".to_string())],
    })
    .await
    .unwrap();

    // Same value, even though the import's 0.80 would win: idempotent skip.
    let file = ObsidianFile {
        relative_path: "Devansh.md".to_string(),
        content: r#"---
type: Person
---

# Devansh

## Preferences
- FoodPreference: favourite = Italian
"#
        .to_string(),
    };
    let outcome = kg.import_obsidian(&[file], false).await.unwrap();
    assert_eq!(outcome.counts.preferences_new, 0, "{:?}", outcome.counts);
    assert_eq!(
        outcome.counts.preferences_updated, 0,
        "{:?}",
        outcome.counts
    );
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let prefs =
        mimir_knowledge::queries::preference::get_preferences_for_entity(kg.pool(), devansh.id)
            .await
            .unwrap();
    assert_eq!(prefs.len(), 1, "no duplicate preference rows");
    assert_eq!(prefs[0].value, "Italian");
}
