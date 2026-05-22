use std::path::Path;

use mimir_core::context::ContextManager;
use mimir_core::personality::Personality;

#[tokio::test]
async fn personality_system_prompt_injected_into_session() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("integration.db");

    // 1. Create a personality.
    let personality = Personality::from_path(Path::new("/nonexistent"), "transparent");
    let system_prompt = personality.system_prompt("User likes Rust and coffee.");

    // 2. Create manager and session with composed system prompt.
    let mgr = ContextManager::new(&db).await.unwrap();
    let sid = mgr.create_session(&system_prompt).await.unwrap();

    // 3. Export messages and assert system prompt is first.
    let exported = mgr.export_messages(&sid).await.unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].role, "system");
    assert!(exported[0].content.contains("transparent"));
    assert!(exported[0].content.contains("## Persistent Memory Context"));
    assert!(exported[0].content.contains("User likes Rust and coffee."));
}

#[tokio::test]
async fn personality_empty_memory_omits_section_in_session() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("integration.db");

    let personality = Personality::from_path(Path::new("/nonexistent"), "concise");
    let system_prompt = personality.system_prompt("");

    let mgr = ContextManager::new(&db).await.unwrap();
    let sid = mgr.create_session(&system_prompt).await.unwrap();

    let exported = mgr.export_messages(&sid).await.unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].role, "system");
    assert!(!exported[0].content.contains("## Persistent Memory Context"));
    assert!(exported[0].content.contains("bullet points"));
}
