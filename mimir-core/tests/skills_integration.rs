use mimir_core::skills::{MarkdownSkill, Skill, SkillRegistry, SkillSource, parse_skill_file};

#[test]
fn registry_with_builtins_contains_expected_skills() {
    let registry = SkillRegistry::with_builtins();
    let skills = registry.list();
    let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"research_synthesis"));
    assert!(names.contains(&"test_driven_development"));
}

#[tokio::test]
async fn registry_loads_user_skills_from_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skill_path = dir.path().join("hello.md");
    std::fs::write(
        &skill_path,
        r#"---
name: hello-world
version: 1.0.0
description: Says hello.
---

# Hello World

Say hello to the user.
"#,
    )
    .unwrap();

    let registry = SkillRegistry::new();
    let count = registry.load_user_skills(dir.path()).await.unwrap();
    assert_eq!(count, 1);

    let meta = registry.metadata("hello-world").unwrap();
    assert_eq!(meta.name, "hello-world");
    assert_eq!(meta.source, SkillSource::User);
}

#[tokio::test]
async fn registry_skips_invalid_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("valid.md"),
        "---\nname: valid\n---\n\nBody.",
    )
    .unwrap();
    std::fs::write(dir.path().join("invalid.md"), "not frontmatter").unwrap();
    std::fs::write(
        dir.path().join("plain.txt"),
        "---\nname: text\n---\n\nBody.",
    )
    .unwrap();

    let registry = SkillRegistry::new();
    let count = registry.load_user_skills(dir.path()).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn load_user_skills_missing_directory_returns_zero() {
    let registry = SkillRegistry::new();
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist");
    let count = registry.load_user_skills(&missing).await.unwrap();
    assert_eq!(count, 0);
}

#[test]
fn markdown_skill_trait_methods() {
    let def = parse_skill_file(
        r#"---
name: test-skill
version: 2.0.0
description: A test skill.
parameters:
  type: object
  properties:
    input:
      type: string
  required: [input]
---

# Test Skill

Do the thing.
"#,
    )
    .unwrap();

    let skill = MarkdownSkill::from_definition(def).unwrap();
    assert_eq!(skill.name(), "test-skill");
    assert_eq!(skill.description(), "A test skill.");
    let schema = skill.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"].get("input").is_some());
}

#[tokio::test]
async fn openai_export_includes_both_builtin_and_user() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("user.md"),
        "---\nname: user_skill\n---\n\nBody.",
    )
    .unwrap();

    let registry = SkillRegistry::with_builtins();
    registry.load_user_skills(dir.path()).await.unwrap();

    let exported = registry.export_openai_tools();
    let names: Vec<_> = exported
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"research_synthesis"));
    assert!(names.contains(&"test_driven_development"));
    assert!(names.contains(&"user_skill"));
}
