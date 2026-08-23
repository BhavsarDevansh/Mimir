use super::*;

async fn setup_manager() -> (ContextManager, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");
    let mgr = ContextManager::new(&db).await.unwrap();
    (mgr, dir)
}

#[tokio::test]
async fn create_session_returns_i64() {
    let (mgr, _dir) = setup_manager().await;
    let id = mgr
        .create_session("You are a test assistant")
        .await
        .unwrap();
    assert!(id > 0, "expected positive i64 session id, got {id}");
}

#[tokio::test]
async fn ensure_session_exists_populates_cache_and_rejects_unknown() {
    let (mgr, _dir) = setup_manager().await;
    let id = mgr
        .create_session("You are a test assistant")
        .await
        .unwrap();

    // First call should hit the database and populate the cache.
    mgr.ensure_session_exists(id).await.unwrap();
    // Second call should use the cached value without error.
    mgr.ensure_session_exists(id).await.unwrap();

    let unknown_id = i64::MAX;
    let err = mgr.ensure_session_exists(unknown_id).await.unwrap_err();
    assert!(matches!(err, ContextError::SessionNotFound(_)));
}

#[tokio::test]
async fn add_user_and_assistant_messages_persist() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();

    mgr.add_user_message(sid, "hello").await.unwrap();
    mgr.add_assistant_message(sid, "hi there").await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[2].role, "assistant");
}

#[tokio::test]
async fn trim_respects_max_turns() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();

    for i in 0..25 {
        mgr.add_user_message(sid, format!("msg {}", i))
            .await
            .unwrap();
        mgr.add_assistant_message(sid, format!("reply {}", i))
            .await
            .unwrap();
    }

    mgr.trim_to_budget(sid, Some(4096), 20).await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 41);
    assert_eq!(msgs[0].role, "system");
}

#[tokio::test]
async fn trim_respects_max_tokens_after_usage_recorded() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();

    for i in 0..10 {
        mgr.add_user_message(sid, format!("u{}", i)).await.unwrap();
        mgr.add_assistant_message(sid, format!("a{}", i))
            .await
            .unwrap();
        mgr.record_usage(sid, ((i + 1) * 500) as u32, ((i + 1) * 500) as u32)
            .await
            .unwrap();
    }

    mgr.trim_to_budget(sid, Some(2000), 100).await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert!(
        msgs.len() <= 9,
        "expected at most 9 messages, got {}",
        msgs.len()
    );
    assert_eq!(msgs[0].role, "system");
}

#[tokio::test]
async fn system_prompt_never_trimmed() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("precious system prompt").await.unwrap();

    for i in 0..5 {
        mgr.add_user_message(sid, format!("u{}", i)).await.unwrap();
        mgr.add_assistant_message(sid, format!("a{}", i))
            .await
            .unwrap();
    }

    mgr.trim_to_budget(sid, Some(1), 1).await.unwrap();
    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[0].content, "precious system prompt");
}

#[tokio::test]
async fn export_messages_orders_system_first() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "u1").await.unwrap();
    mgr.add_assistant_message(sid, "a1").await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[2].role, "assistant");
}

#[tokio::test]
async fn record_usage_attribution() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();

    mgr.add_user_message(sid, "hello").await.unwrap();
    mgr.add_assistant_message(sid, "world").await.unwrap();
    mgr.record_usage(sid, 10, 5).await.unwrap();

    let conv = mgr.export_conversation(sid).await.unwrap();
    assert_eq!(conv.session.cumulative_prompt_tokens, 10);
    assert_eq!(conv.session.cumulative_completion_tokens, 5);

    mgr.add_user_message(sid, "how?").await.unwrap();
    mgr.add_assistant_message(sid, "fine").await.unwrap();
    // Pass deltas, not cumulative totals.
    mgr.record_usage(sid, 15, 7).await.unwrap();

    let conv2 = mgr.export_conversation(sid).await.unwrap();
    assert_eq!(conv2.session.cumulative_prompt_tokens, 25);
    assert_eq!(conv2.session.cumulative_completion_tokens, 12);

    let rows: Vec<ContextMessage> = sqlx::query_as::<_, ContextMessage>(
        "SELECT * FROM messages WHERE session_id = ?1 AND role = 'user' ORDER BY created_at ASC",
    )
    .bind(sid)
    .fetch_all(mgr.pool.as_ref())
    .await
    .unwrap();

    assert_eq!(rows.len(), 2);
    // First user message got 10 prompt tokens.
    assert_eq!(rows[0].token_count, Some(10));
    // Second user message got 15 prompt tokens (delta).
    assert_eq!(rows[1].token_count, Some(15));
}

#[tokio::test]
async fn unknown_session_returns_error() {
    let (mgr, _dir) = setup_manager().await;
    let result = mgr.add_user_message(999_999, "x").await;
    assert!(matches!(result, Err(ContextError::SessionNotFound(_))));
}

#[tokio::test]
async fn delete_session_cascade_removes_messages() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "u1").await.unwrap();
    mgr.delete_session(sid).await.unwrap();

    let result = mgr.export_messages(sid).await;
    assert!(matches!(result, Err(ContextError::SessionNotFound(_))));
}

#[tokio::test]
async fn reload_from_db_restores_messages() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("persist.db");

    {
        let mgr = ContextManager::new(&db).await.unwrap();
        let sid = mgr.create_session("sys").await.unwrap();
        mgr.add_user_message(sid, "hello").await.unwrap();
        mgr.add_assistant_message(sid, "world").await.unwrap();
    }

    let mgr2 = ContextManager::new(&db).await.unwrap();
    let sids: Vec<i64> = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_all(mgr2.pool.as_ref())
        .await
        .unwrap();
    assert_eq!(sids.len(), 1);

    let msgs = mgr2.export_messages(sids[0]).await.unwrap();
    assert_eq!(msgs.len(), 3);
}

#[tokio::test]
async fn db_path_with_tilde_expanded() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    // Test tilde expansion directly without mutating process env.
    let expanded = expand_tilde_with_home(
        std::path::Path::new("~/nonexistent_test_mimir_ctx.db"),
        home,
    );
    assert_eq!(expanded, home.join("nonexistent_test_mimir_ctx.db"));

    let mgr = ContextManager::new(expanded.to_str().unwrap()).await;
    assert!(
        mgr.is_ok(),
        "ContextManager should succeed with expanded path"
    );
    assert!(
        expanded.exists(),
        "DB file should be created under temp HOME"
    );

    // Clean up session if created (best-effort).
    if let Ok(ref m) = mgr {
        let sessions = m.list_sessions().await.unwrap();
        for s in sessions {
            let _ = m.delete_session(s.id).await;
        }
    }
}

#[tokio::test]
async fn list_sessions_orders_by_updated_at_desc() {
    let (mgr, _dir) = setup_manager().await;
    let sid1 = mgr.create_session("sys1").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let sid2 = mgr.create_session("sys2").await.unwrap();

    mgr.add_user_message(sid1, "first").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    mgr.add_user_message(sid2, "second").await.unwrap();

    let list = mgr.list_sessions().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, sid2);
    assert_eq!(list[1].id, sid1);
}

#[tokio::test]
async fn list_sessions_preview_is_latest_user_message() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "hello").await.unwrap();
    mgr.add_assistant_message(sid, "hi").await.unwrap();
    mgr.add_user_message(sid, "world").await.unwrap();

    let list = mgr.list_sessions().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].preview, Some("world".to_string()));
}

#[tokio::test]
async fn list_sessions_empty_db_returns_empty() {
    let (mgr, _dir) = setup_manager().await;
    let list = mgr.list_sessions().await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn get_messages_after_compaction_returns_all_when_null() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "u1").await.unwrap();
    mgr.add_assistant_message(sid, "a1").await.unwrap();

    let msgs = mgr.get_messages_after_compaction(sid).await.unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[2].role, "assistant");
}

#[tokio::test]
async fn get_messages_after_compaction_returns_only_after_timestamp() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "old").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let mid = Utc::now();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    mgr.add_user_message(sid, "new").await.unwrap();
    mgr.add_assistant_message(sid, "reply").await.unwrap();

    sqlx::query("UPDATE sessions SET compacted_at = ?1 WHERE id = ?2")
        .bind(mid)
        .bind(sid)
        .execute(mgr.pool.as_ref())
        .await
        .unwrap();

    let msgs = mgr.get_messages_after_compaction(sid).await.unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "new");
    assert_eq!(msgs[1].role, "assistant");
}

#[tokio::test]
async fn get_messages_after_compaction_unknown_session_errors() {
    let (mgr, _dir) = setup_manager().await;
    let result = mgr.get_messages_after_compaction(999_999).await;
    assert!(matches!(result, Err(ContextError::SessionNotFound(_))));
}

#[tokio::test]
async fn schema_migration_adds_compacted_at() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("migrate.db");

    // Create an old-style database without compacted_at.
    {
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query(
            r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    system_prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
                    summary TEXT
                )
                "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    token_count INTEGER
                )
                "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    // ContextManager::new should migrate it.
    let mgr = ContextManager::new(&db).await.unwrap();
    let sid = mgr.create_session("sys").await.unwrap();
    let conv = mgr.export_conversation(sid).await.unwrap();
    assert!(conv.session.compacted_at.is_none());
}

#[tokio::test]
async fn schema_migration_text_to_integer() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("migrate_text.db");

    // Create an old-style database with TEXT session IDs.
    {
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query(
            r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    system_prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
                    summary TEXT,
                    compacted_at TEXT
                )
                "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    token_count INTEGER
                )
                "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Seed data with TEXT session IDs.
        sqlx::query(
                "INSERT INTO sessions (id, system_prompt, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)"
            )
            .bind("old-session-uuid")
            .bind("old sys")
            .bind(Utc::now())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
                "INSERT INTO messages (session_id, role, content, created_at) VALUES (?1, 'user', 'hello world', ?2)"
            )
            .bind("old-session-uuid")
            .bind(Utc::now())
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    let mgr = ContextManager::new(&db).await.unwrap();
    let sid = mgr.create_session("sys").await.unwrap();
    assert!(sid > 0);

    // Verify old data was migrated.
    let sessions = mgr.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 2); // old + new
    let old_session = sessions.iter().find(|s| s.id != sid).unwrap();
    let msgs = mgr.export_messages(old_session.id).await.unwrap();
    assert_eq!(msgs.len(), 1); // only user message (old session had no system message)
    assert!(msgs.iter().any(|m| m.content == "hello world"));

    // Verify search works on migrated data.
    let results = mgr.search_messages("hello", 10, None).await.unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.snippet.contains("hello")));
}

#[tokio::test]
async fn search_messages_basic() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "the quick brown fox")
        .await
        .unwrap();
    mgr.add_assistant_message(sid, "jumps over the lazy dog")
        .await
        .unwrap();

    let results = mgr.search_messages("fox", 10, None).await.unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.snippet.contains("<<<fox>>>")));
}

#[tokio::test]
async fn search_messages_session_filter() {
    let (mgr, _dir) = setup_manager().await;
    let sid1 = mgr.create_session("sys1").await.unwrap();
    let sid2 = mgr.create_session("sys2").await.unwrap();

    mgr.add_user_message(sid1, "unique keyword alpha")
        .await
        .unwrap();
    mgr.add_user_message(sid2, "unique keyword beta")
        .await
        .unwrap();

    let results = mgr.search_messages("alpha", 10, Some(sid1)).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].session_id, sid1);
}

#[tokio::test]
async fn search_messages_no_results() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "hello world").await.unwrap();

    let results = mgr
        .search_messages("xyznonsense123", 10, None)
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_context_manager_close() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("close_test.db");
    let mgr = ContextManager::new(&db).await.unwrap();
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "hello").await.unwrap();

    mgr.close().await;

    // After close, any operation should fail because the pool is closed.
    let result = mgr.add_user_message(sid, "world").await;
    assert!(
        matches!(result, Err(ContextError::Database(_))),
        "expected database error after close, got: {:?}",
        result
    );
}

// -- OpenAI provider surface (issue #388) --

#[tokio::test]
async fn resolve_openai_session_creates_and_resumes() {
    let (mgr, _dir) = setup_manager().await;
    let first = mgr
        .resolve_openai_session("phone", "first system prompt")
        .await
        .unwrap();
    let second = mgr
        .resolve_openai_session("phone", "second system prompt")
        .await
        .unwrap();
    assert_eq!(first, second, "same user key must resume one session");

    // First-writer-wins: the stored system prompt is the creation-time one.
    let msgs = mgr.export_messages(first).await.unwrap();
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[0].content, "first system prompt");
}

#[tokio::test]
async fn resolve_openai_session_distinct_user_keys() {
    let (mgr, _dir) = setup_manager().await;
    let phone = mgr.resolve_openai_session("phone", "sys").await.unwrap();
    let laptop = mgr.resolve_openai_session("laptop", "sys").await.unwrap();
    assert_ne!(
        phone, laptop,
        "distinct user keys must map to distinct sessions"
    );
}

#[tokio::test]
async fn resolve_openai_session_concurrent_creates_single_session() {
    let (mgr, _dir) = setup_manager().await;
    let mgr = std::sync::Arc::new(mgr);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let mgr = std::sync::Arc::clone(&mgr);
        handles.push(tokio::spawn(async move {
            mgr.resolve_openai_session("shared", "sys").await.unwrap()
        }));
    }
    let mut ids = Vec::new();
    for handle in handles {
        ids.push(handle.await.unwrap());
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        1,
        "concurrent resolves must converge on one session"
    );
}

#[tokio::test]
async fn tool_messages_roundtrip_through_export() {
    use crate::llm::types::{FunctionCall, ToolCall};

    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();
    mgr.add_user_message(sid, "weather?").await.unwrap();

    let tool_calls = vec![ToolCall {
        index: 0,
        id: "call_1".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "get_weather".to_string(),
            arguments: "{\"location\":\"London\"}".to_string(),
        },
    }];
    mgr.add_assistant_tool_calls_message(sid, "", &tool_calls)
        .await
        .unwrap();
    mgr.add_tool_message(sid, "call_1", "sunny").await.unwrap();
    mgr.add_assistant_message(sid, "It is sunny.")
        .await
        .unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 5);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[2].role, "assistant");
    assert_eq!(msgs[2].tool_calls.as_ref().unwrap(), &tool_calls);
    assert_eq!(msgs[3].role, "tool");
    assert_eq!(msgs[3].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(msgs[3].content, "sunny");
    assert_eq!(msgs[4].role, "assistant");
    assert_eq!(msgs[4].tool_calls, None);
    assert_eq!(msgs[4].tool_call_id, None);
}

#[tokio::test]
async fn trim_removes_whole_turns_including_tool_messages() {
    use crate::llm::types::{FunctionCall, ToolCall};

    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();

    // Turn 1: user -> assistant(tool_calls) -> tool -> assistant.
    mgr.add_user_message(sid, "u1").await.unwrap();
    let calls = vec![ToolCall {
        index: 0,
        id: "c1".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "t".to_string(),
            arguments: "{}".to_string(),
        },
    }];
    mgr.add_assistant_tool_calls_message(sid, "", &calls)
        .await
        .unwrap();
    mgr.add_tool_message(sid, "c1", "r1").await.unwrap();
    mgr.add_assistant_message(sid, "a1").await.unwrap();

    // Turn 2: plain user -> assistant.
    mgr.add_user_message(sid, "u2").await.unwrap();
    mgr.add_assistant_message(sid, "a2").await.unwrap();

    mgr.trim_to_budget(sid, None, 1).await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 3, "oldest turn must be removed whole: {msgs:?}");
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].content, "u2");
    assert_eq!(msgs[2].content, "a2");
}

#[tokio::test]
async fn trim_token_budget_keeps_in_flight_turn() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();

    // Complete turn 1 with known token counts.
    mgr.add_user_message(sid, "u1").await.unwrap();
    mgr.add_assistant_message(sid, "a1").await.unwrap();
    mgr.record_usage(sid, 100, 100).await.unwrap();

    // Turn 2 is in flight: its user message was just persisted and is the
    // message the current request is answering.
    mgr.add_user_message(sid, "u2").await.unwrap();
    mgr.record_usage(sid, 50, 0).await.unwrap();

    // A budget smaller than the in-flight turn alone: the token path would
    // previously delete every turn (including the fresh u2) and leave the
    // LLM call without the user's message.
    mgr.trim_to_budget(sid, Some(1), 20).await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 2, "in-flight turn must survive: {msgs:?}");
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[1].content, "u2");
}

#[tokio::test]
async fn trim_fallback_keeps_in_flight_turn() {
    let (mgr, _dir) = setup_manager().await;
    let sid = mgr.create_session("sys").await.unwrap();

    // Turn 1 complete (u1 has tokens, a1 has none -> unknown-count fallback).
    mgr.add_user_message(sid, "u1").await.unwrap();
    mgr.record_usage(sid, 100, 0).await.unwrap();
    mgr.add_assistant_message(sid, "a1").await.unwrap();

    // Turn 2 in flight (no token counts yet).
    mgr.add_user_message(sid, "u2").await.unwrap();

    // max_turns = 1 forces the fallback target to zero turns; previously the
    // fallback then deleted every turn including the in-flight u2 message.
    mgr.trim_to_budget(sid, Some(1), 1).await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 2, "in-flight turn must survive: {msgs:?}");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[1].content, "u2");
}

#[tokio::test]
async fn schema_migration_adds_user_key_and_tool_columns() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("migrate_openai.db");

    // Create a pre-#388 database: sessions without user_key, messages
    // without tool_calls / tool_call_id.
    {
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query(
            r#"
                CREATE TABLE sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    system_prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    cumulative_prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    cumulative_completion_tokens INTEGER NOT NULL DEFAULT 0,
                    summary TEXT,
                    compacted_at TEXT
                )
                "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    token_count INTEGER
                )
                "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let mgr = ContextManager::new(&db).await.unwrap();
    let sid = mgr.resolve_openai_session("phone", "sys").await.unwrap();
    let resumed = mgr.resolve_openai_session("phone", "sys").await.unwrap();
    assert_eq!(sid, resumed);

    let calls = vec![crate::llm::types::ToolCall {
        index: 0,
        id: "c1".to_string(),
        call_type: "function".to_string(),
        function: crate::llm::types::FunctionCall {
            name: "t".to_string(),
            arguments: "{}".to_string(),
        },
    }];
    mgr.add_assistant_tool_calls_message(sid, "", &calls)
        .await
        .unwrap();
    mgr.add_tool_message(sid, "c1", "r1").await.unwrap();

    let msgs = mgr.export_messages(sid).await.unwrap();
    assert_eq!(msgs[1].tool_calls.as_ref().unwrap(), &calls);
    assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c1"));
}

use super::path::expand_tilde_with_home;
