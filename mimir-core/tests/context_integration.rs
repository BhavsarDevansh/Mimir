use mimir_core::context::ContextManager;

#[tokio::test]
async fn full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("integration.db");

    // 1. Create manager.
    let mgr = ContextManager::new(&db).await.unwrap();

    // 2. Create session.
    let sid = mgr.create_session("You are Mimir").await.unwrap();
    assert!(!sid.is_empty());

    // 3. Add user message.
    mgr.add_user_message(&sid, "What is Rust?").await.unwrap();

    // 4. Export messages for LLM request.
    let exported = mgr.export_messages(&sid).await.unwrap();
    assert_eq!(exported.len(), 2);
    assert_eq!(exported[0].role, "system");
    assert_eq!(exported[1].role, "user");

    // 5. Add assistant message.
    mgr.add_assistant_message(&sid, "Rust is a systems programming language.")
        .await
        .unwrap();

    // 6. Simulate API usage response.
    mgr.record_usage(&sid, 12, 5).await.unwrap();

    // 7. Trim.
    mgr.trim_to_budget(&sid, Some(4096), 20).await.unwrap();

    // 8. Verify DB state.
    let conv = mgr.export_conversation(&sid).await.unwrap();
    assert_eq!(conv.messages.len(), 3);
    assert_eq!(conv.session.cumulative_prompt_tokens, 12);
    assert_eq!(conv.session.cumulative_completion_tokens, 5);

    // 9. Drop manager, recreate (reload test).
    drop(mgr);
    let mgr2 = ContextManager::new(&db).await.unwrap();

    let msgs = mgr2.export_messages(&sid).await.unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[2].role, "assistant");

    // 10. Delete session.
    mgr2.delete_session(&sid).await.unwrap();

    // 11. Verify empty.
    let result = mgr2.export_messages(&sid).await;
    assert!(result.is_err());
}
