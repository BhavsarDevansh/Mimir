#![deny(unsafe_code)]
pub mod error;
pub mod routes;
pub mod state;
pub mod types;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::ConnectInfo,
    http::StatusCode,
    middleware::from_fn,
    response::IntoResponse,
    routing::{get, post},
};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{info, warn};

use mimir_core::config::ReloadableConfig;
use mimir_core::llm::{LlmBackend, LlmClient};

use crate::routes::{
    chat_handler, chat_stream_handler, create_category, delete_category, kb_audit_handler,
    kb_browse_handler, kb_confirm_fact_handler, kb_edit_handler, kb_forget_handler,
    kb_optimization_run_now_handler, kb_optimization_status_handler, kb_pending_handler,
    kb_profile_handler, kb_query_handler, kb_reject_fact_handler, kb_show_handler,
    kb_trash_empty_handler, kb_trash_list_handler, kb_trash_restore_handler, list_categories,
    memory_handler, memory_refresh_handler, session_messages_handler, sessions_handler,
    show_category, status_handler, stop_handler,
};
use crate::state::AppState;

/// Middleware guard that restricts access to loopback addresses.
async fn require_loopback(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if !addr.ip().is_loopback() {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

/// Build the Axum router with all routes and middleware.
pub fn build_app(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin([
            "http://localhost:8080"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://127.0.0.1:8080"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://localhost:3000"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://127.0.0.1:3000"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://localhost:5173"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
            "http://127.0.0.1:5173"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
        ])
        .allow_methods([
            http::Method::GET,
            http::Method::POST,
            http::Method::PATCH,
            http::Method::DELETE,
        ])
        .allow_headers([http::header::CONTENT_TYPE]);

    Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/status", get(status_handler))
        .route("/memory", get(memory_handler))
        .route(
            "/memory/refresh",
            post(memory_refresh_handler).layer(from_fn(require_loopback)),
        )
        .route("/sessions", get(sessions_handler))
        .route("/sessions/{id}/messages", get(session_messages_handler))
        .route("/chat", post(chat_handler))
        .route("/chat/stream", post(chat_stream_handler))
        .route(
            "/kb/optimization/status",
            get(kb_optimization_status_handler),
        )
        .route(
            "/kb/optimization/run-now",
            post(kb_optimization_run_now_handler).layer(from_fn(require_loopback)),
        )
        .route("/kb/categories", get(list_categories).post(create_category))
        .route(
            "/kb/categories/{id}",
            get(show_category).delete(delete_category),
        )
        .route("/kb/query", get(kb_query_handler))
        .route(
            "/kb/facts/{id}",
            get(kb_show_handler).patch(kb_edit_handler),
        )
        .route("/kb/facts/forget", post(kb_forget_handler))
        .route(
            "/kb/facts/{id}/confirm",
            post(kb_confirm_fact_handler).layer(from_fn(require_loopback)),
        )
        .route(
            "/kb/facts/{id}/reject",
            post(kb_reject_fact_handler).layer(from_fn(require_loopback)),
        )
        .route(
            "/kb/pending",
            get(kb_pending_handler).layer(from_fn(require_loopback)),
        )
        .route("/kb/browse", get(kb_browse_handler))
        .route("/kb/profile", get(kb_profile_handler))
        .route("/kb/audit", get(kb_audit_handler))
        .route(
            "/kb/trash",
            get(kb_trash_list_handler).delete(kb_trash_empty_handler),
        )
        .route("/kb/trash/restore", post(kb_trash_restore_handler))
        .route("/stop", post(stop_handler).layer(from_fn(require_loopback)))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(cors),
        )
        .with_state(state)
}

/// Combined shutdown signal that races Ctrl-C, SIGTERM (Unix), and the
/// `/stop` endpoint watch channel.
async fn shutdown_signal(mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = sigterm.recv() => {},
            _ = shutdown_rx.changed() => {},
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = shutdown_rx.changed() => {},
        }
    }
}

/// Maximum time to wait for in-flight connections to finish after a shutdown
/// is requested. Bounds **only** the post-signal drain phase; the server runs
/// indefinitely while no shutdown is requested.
const GRACEFUL_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `app` on `listener` until a shutdown trigger fires, then bound the
/// graceful drain of in-flight connections to `drain_timeout`.
///
/// Extracted from `start_server_with_llm_and_listener` so the drain bound can
/// be unit-tested with a short timeout (see `test_serve_outlives_drain_timeout`).
async fn serve_with_bounded_drain(
    listener: tokio::net::TcpListener,
    app: Router,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    drain_timeout: Duration,
) -> anyhow::Result<()> {
    // One receiver drives axum's graceful-shutdown (stop accepting); the other
    // lets this function detect the trigger independently so it can bound only
    // the *drain* phase rather than the whole serving lifetime.
    let graceful_rx = shutdown_tx.subscribe();
    let trigger_rx = shutdown_tx.subscribe();

    // Wrap axum's `IntoFuture` in an `async` block to obtain a concrete
    // `Future` that can be pinned and polled across two phases below.
    // (axum 0.8's `WithGracefulShutdown` implements `IntoFuture` but not
    // `Future`, so it cannot be `tokio::pin!`-ed or `&mut`-polled directly.)
    let server_fut = async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal(graceful_rx))
        .await
    };

    // Pin the server future so it can be polled across two phases: first
    // serving (unbounded), then draining (bounded by `drain_timeout`).
    tokio::pin!(server_fut);

    // Phase 1 — serve until a shutdown trigger fires (Ctrl-C, SIGTERM, or
    // `/stop`). The server keeps accepting and handling connections
    // throughout; this wait is intentionally unbounded. If the server
    // future resolves first (e.g. a fatal listener error), propagate it.
    tokio::select! {
        biased;
        _ = shutdown_signal(trigger_rx) => {},
        result = &mut server_fut => {
            info!("Server shut down gracefully.");
            return Ok(result?);
        }
    }

    // Phase 2 — trigger fired. axum's own graceful-shutdown signal (wired to
    // the same triggers via `graceful_rx`) has fired too, so it has stopped
    // accepting and is now draining in-flight connections. Bound that drain
    // so a wedged SSE stream cannot keep the process alive past systemd's
    // `TimeoutStopSec`. On timeout, dropping `server_fut` cuts the connections.
    let server_result = match tokio::time::timeout(drain_timeout, &mut server_fut).await {
        Ok(result) => {
            info!("Server shut down gracefully.");
            result
        }
        Err(_) => {
            warn!(
                "Graceful drain timed out after {}s; forcing exit.",
                drain_timeout.as_secs()
            );
            Ok(())
        }
    };
    Ok(server_result?)
}

/// Start the Mimir HTTP server using the provided configuration.
///
/// Loads shared state from `config`, binds to `config.server.bind_addr`,
/// and runs until the process is terminated or a graceful shutdown is
/// triggered via the `/stop` endpoint, Ctrl-C, or SIGTERM.
///
/// If the server does not shut down gracefully within 30 seconds, it is
/// forcefully aborted so that resource cleanup can still run.
pub async fn start_server(config: Arc<ReloadableConfig>) -> anyhow::Result<()> {
    let llm_client: Arc<dyn LlmBackend> =
        Arc::new(LlmClient::new(config.snapshot().await.llm.clone()).await);
    start_server_with_llm(config, llm_client).await
}

/// Start the Mimir HTTP server with an injected LLM backend.
///
/// This is the same as [`start_server`], but allows tests (and future
/// embedders) to supply a custom [`LlmBackend`] implementation without
/// relying on sentinel strings or config hacks.
pub async fn start_server_with_llm(
    config: Arc<ReloadableConfig>,
    llm_client: Arc<dyn LlmBackend>,
) -> anyhow::Result<()> {
    let bind_addr = config.snapshot().await.server.bind_addr.clone();
    let addr: SocketAddr = bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    start_server_with_llm_and_listener(config, llm_client, listener).await
}

/// Start the Mimir HTTP server with an injected LLM backend and a pre-bound listener.
///
/// This is the same as [`start_server_with_llm`], but allows tests to supply
/// a pre-bound [`TcpListener`] so the bound port is known before the server
/// starts accepting connections.
pub async fn start_server_with_llm_and_listener(
    config: Arc<ReloadableConfig>,
    llm_client: Arc<dyn LlmBackend>,
    listener: tokio::net::TcpListener,
) -> anyhow::Result<()> {
    let (app_state, scheduler_shutdown_rx) =
        AppState::from_config_with_llm(Arc::clone(&config), llm_client).await?;
    let state = Arc::new(app_state);

    // ---- Start background scheduler dispatch loop ----
    let sched = Arc::clone(&state.scheduler);
    let sched_shutdown_rx = scheduler_shutdown_rx;
    tokio::spawn(async move {
        sched.start(sched_shutdown_rx).await;
    });

    // ---- Listen for KG dirty signal and submit condensation ----
    if state.user_entity_id.is_some() {
        let notify = state.knowledge_graph.condensation_notify();
        let sched = Arc::clone(&state.scheduler);
        let mut shutdown_rx = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    _ = notify.notified() => {
                        use mimir_core::scheduler::DaemonJob;
                        sched.submit(DaemonJob::MemoryCondensation).await;
                    }
                }
            }
        });
    } else {
        tracing::debug!("Skipping condensation notify listener: no user entity configured");
    }

    // ---- File watcher for config hot-reload ----
    let config_path = config.path().to_path_buf();
    if let Some(parent) = config_path.parent().map(|p| p.to_path_buf()) {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let config_clone = Arc::clone(&config);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let config_filename = config_path.file_name().map(|n| n.to_os_string());
        let meta_path = config_path.clone();

        tokio::task::spawn_blocking(move || {
            let (debounce_tx, debounce_rx) = std::sync::mpsc::channel();
            let mut debouncer = notify_debouncer_full::new_debouncer(
                std::time::Duration::from_secs(1),
                None,
                debounce_tx,
            )
            .expect("debouncer creation");
            if let Err(e) = debouncer.watch(&parent, notify::RecursiveMode::NonRecursive) {
                tracing::warn!("Failed to watch config directory: {}", e);
                return;
            }
            // Signature (mtime, size) of the last file content we asked the
            // async task to reload. Reading the config file generates
            // `Access`/close events; without dedupe these feed a self-reload
            // loop (~1 event per second) and flood the journal.
            let mut last_sig: Option<(std::time::SystemTime, u64)> = None;
            loop {
                match debounce_rx.recv_timeout(std::time::Duration::from_millis(250)) {
                    Ok(Ok(events)) => {
                        // Only react to real content changes on the config
                        // file; ignore pure `Access` events (open/read/close),
                        // which the OS emits when *we* read the file to reload.
                        let relevant = events.iter().any(|e| {
                            if e.event.kind.is_access() {
                                return false;
                            }
                            e.event.paths.iter().any(|p| {
                                config_filename
                                    .as_ref()
                                    .map(|cf| {
                                        p.file_name().map(|n| n == cf.as_os_str()).unwrap_or(false)
                                    })
                                    .unwrap_or(false)
                            })
                        });
                        if !relevant {
                            continue;
                        }
                        // Dedupe by metadata signature so identical content
                        // (repeated byte-identical saves, or repeated
                        // "sensitive field" rejections) is acted on at most once.
                        let Some(meta) = std::fs::metadata(&meta_path).ok() else {
                            continue;
                        };
                        let sig = (
                            meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                            meta.len(),
                        );
                        if last_sig == Some(sig) {
                            continue;
                        }
                        last_sig = Some(sig);
                        if let Err(e) = tx.try_send(()) {
                            tracing::warn!("Config reload event dropped: {}", e);
                        }
                    }
                    Ok(Err(errors)) => {
                        for error in errors {
                            tracing::warn!("File watcher error: {:?}", error);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            drop(debouncer);
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        let mut shutdown_rx_clone = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx_clone.changed() => {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    msg = rx.recv() => {
                        if msg.is_none() { break; }
                        match config_clone.reload().await {
                            Ok(()) => tracing::info!("Config reloaded from file watcher"),
                            Err(e) => tracing::warn!("Config reload failed: {}", e),
                        }
                    }
                }
            }
        });
    }

    // ---- SIGHUP handler for config hot-reload ----
    #[cfg(unix)]
    {
        let config_clone = Arc::clone(&config);
        let mut shutdown_rx_clone = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP handler");
            loop {
                tokio::select! {
                    _ = shutdown_rx_clone.changed() => break,
                    _ = sighup.recv() => {
                        match config_clone.reload().await {
                            Ok(()) => tracing::info!("Config reloaded from SIGHUP"),
                            Err(e) => tracing::warn!("Config reload failed: {}", e),
                        }
                    }
                }
            }
        });
    }

    let app = build_app(Arc::clone(&state));

    info!("Mimir daemon listening on {}", listener.local_addr()?);

    // Serve until a shutdown trigger fires (Ctrl-C, SIGTERM, or `/stop`),
    // then drain in-flight connections for at most `GRACEFUL_DRAIN_TIMEOUT`.
    // Only the drain phase is bounded — the serving lifetime is unbounded.
    let server_result = serve_with_bounded_drain(
        listener,
        app,
        state.shutdown_tx.clone(),
        GRACEFUL_DRAIN_TIMEOUT,
    )
    .await;

    // Broadcast the shutdown watch so every background task spawned from
    // `start_server_with_llm_and_listener` (file watcher, SIGHUP handler,
    // condensation listener) tears down before the runtime drops. Without an
    // explicit broadcast the SIGTERM/Ctrl-C path relied on `AppState` being
    // dropped during runtime teardown to resolve the file-watcher's
    // `shutdown_rx.changed()` (via sender-drop). That is a race: if tokio's
    // `BlockingPool::shutdown` runs before the watcher task is polled, the
    // `spawn_blocking` thread never exits and the process hangs until systemd
    // aborts it with SIGABRT after `TimeoutStopSec`. Sending here, while the
    // runtime is still fully alive, makes teardown deterministic.
    let _ = state.shutdown_tx.send(true);

    state.shutdown().await;
    server_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use dashmap::DashMap;
    use tower::ServiceExt;

    use mimir_api_types::{ChatResponse, StatusResponse};
    use mimir_core::{
        config::{Config, ReloadableConfig},
        context::ContextManager,
        job_queue::JobQueue,
        llm::types::{FunctionCall, LlmError, Message, StreamItem, ToolCall, Usage},
        llm::{LlmBackend, MockLlmClient},
    };

    use crate::state::AppState;

    /// Build an `AppState` suitable for tests, using a temporary directory
    /// for the context database.
    async fn test_state_with_config(
        llm: Arc<dyn LlmBackend>,
        config: Config,
    ) -> (Arc<AppState>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("context.db");

        let context_manager = Arc::new(ContextManager::new(&db_path).await.unwrap());
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let reloadable =
            ReloadableConfig::new(config.clone(), temp.path().join("dummy_config.toml"));

        let tool_registry = mimir_core::tools::ToolRegistry::with_builtins();

        let kg_db_path = temp.path().join("knowledge.db");
        let knowledge_graph = Arc::new(
            mimir_knowledge::KnowledgeGraph::init(&kg_db_path)
                .await
                .unwrap(),
        );
        tool_registry
            .register_native(Arc::new(mimir_knowledge::KgQueryTool::new(Arc::clone(
                &knowledge_graph,
            ))))
            .unwrap();
        tool_registry
            .register_native(Arc::new(mimir_knowledge::KgRelatedTool::new(Arc::clone(
                &knowledge_graph,
            ))))
            .unwrap();
        tool_registry
            .register_native(Arc::new(mimir_knowledge::KgSearchTool::new(Arc::clone(
                &knowledge_graph,
            ))))
            .unwrap();
        tool_registry
            .register_native(Arc::new(mimir_knowledge::KgExpandCatalogueTool::new(
                Arc::clone(&knowledge_graph),
            )))
            .unwrap();
        tool_registry
            .register_native(Arc::new(mimir_knowledge::KgFactsInCatalogueTool::new(
                Arc::clone(&knowledge_graph),
            )))
            .unwrap();
        tool_registry
            .register_native(Arc::new(mimir_knowledge::RememberTool::new(Arc::clone(
                &knowledge_graph,
            ))))
            .unwrap();

        let jobs_db_path = temp.path().join("jobs.db");
        let job_queue = Arc::new(JobQueue::init(&jobs_db_path).await.unwrap());
        let last_user_activity = Arc::new(std::sync::atomic::AtomicU64::new(
            (chrono::Utc::now() - chrono::Duration::minutes(10)).timestamp() as u64,
        ));

        // Register a dummy optimization job so the kb routes work in tests.
        let dummy_job = mimir_core::job_queue::Job::new(
            "knowledge.optimization",
            mimir_core::job_queue::JobPriority::System,
            Some(mimir_core::job_queue::DailySchedule::new(
                chrono::NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
            )),
            false,
            |_ctx: mimir_core::job_queue::JobContext| Box::pin(async move { Ok(()) }),
        );
        job_queue.register(dummy_job).await.unwrap();

        // Dummy scheduler for tests.
        let (scheduler, _sched_rx) = mimir_core::scheduler::BackgroundScheduler::new(
            Arc::clone(&job_queue),
            Arc::clone(&llm),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        );

        // Librarian registered to mirror production, though tests no longer
        // auto-invoke it (issue #137): learning is LLM-orchestrated via
        // `remember`. Kept so the on-demand library API stays exercised.
        let agent_runtime = Arc::new(mimir_core::agents::AgentRuntime::new());
        agent_runtime
            .register::<mimir_knowledge::librarian::LibrarianAgent>(
                mimir_knowledge::librarian::LibrarianAgent::new(),
            )
            .await;

        // Resolve or create the user entity from config identity, mirroring
        // production setup, so background agents like the Librarian can run.
        let user_entity_id = if config.identity.name.trim().is_empty() {
            None
        } else {
            match knowledge_graph
                .search_entities(&config.identity.name, 1)
                .await
            {
                Ok(mut results) if !results.is_empty() => Some(results.remove(0).entity.id),
                _ => match knowledge_graph
                    .create_entity(
                        &config.identity.name,
                        mimir_knowledge::models::entity::EntityType::Person,
                        &[],
                    )
                    .await
                {
                    Ok(entity) => Some(entity.id),
                    Err(e) => {
                        tracing::warn!("Failed to create test user entity: {}", e);
                        None
                    }
                },
            }
        };

        let state = Arc::new(AppState {
            llm_client: llm,
            context_manager,
            config: Arc::new(reloadable),
            session_locks: Arc::new(DashMap::new()),
            start_time: Instant::now(),
            endpoint: "http://localhost:8080".to_string(),
            model: "gpt-4o".to_string(),
            shutdown_tx,
            model_override_cache: Arc::new(DashMap::new()),
            tool_registry: Arc::new(tool_registry),
            knowledge_graph,
            job_queue,
            agent_runtime,
            scheduler,
            user_entity_id,
            last_user_activity,
        });

        (state, temp)
    }

    async fn test_state(llm: Arc<dyn LlmBackend>) -> (Arc<AppState>, tempfile::TempDir) {
        test_state_with_config(llm, Config::default()).await
    }

    #[tokio::test]
    async fn test_status_returns_ok() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_returns_ok_without_llm() {
        // `/health` is the cheap liveness probe used by the daemon guard, so it
        // must never touch the LLM backend (which would make the 500ms probe
        // time out on a healthy-but-slow provider).
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(bytes.is_empty(), "health endpoint should return no body");
    }

    #[tokio::test]
    async fn test_chat_creates_session() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat("Hello!", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat: ChatResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(chat.session_id > 0);
        assert_eq!(chat.response, "Hello!");
    }

    // Issue #137: learning is LLM-orchestrated via the `remember` tool. A
    // chitchat turn where the LLM does not call `remember` must not trigger a
    // background extraction LLM call. The unconditional Librarian has been
    // retired, so the mock should record exactly one LLM call (the main chat
    // completion) and no second extraction call.
    #[tokio::test]
    async fn test_chitchat_does_not_trigger_background_learning() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat("Hi there! How can I help?", Usage::default())
                .build(),
        );
        let mut config = Config::default();
        config.identity.name = "devansh".to_string();
        let (state, _temp) = test_state_with_config(mock.clone(), config).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

        assert_eq!(
            mock.chat_calls().len(),
            1,
            "chitchat must not trigger a background extraction LLM call"
        );
    }

    #[tokio::test]
    async fn test_chat_stream_returns_ok() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::Text("hi".to_string())),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            ct.starts_with("text/event-stream"),
            "expected SSE content-type, got: {}",
            ct
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("data: hi"), "expected text frame in SSE body");
        assert!(
            text.contains("event: usage"),
            "expected usage frame in SSE body"
        );
        assert!(
            text.contains("\n\n"),
            "expected SSE frames terminated with double newline"
        );
    }

    #[tokio::test]
    async fn test_chat_stream_llm_error_sends_error_event() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::Text("partial".to_string())),
                    Err(LlmError::Api {
                        status: 500,
                        body: "boom".to_string(),
                    }),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("partial"));
        assert!(text.contains("error"));
    }

    #[tokio::test]
    async fn test_chat_queue_full_returns_503() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_error(LlmError::QueueFull)
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(retry, "5");
    }

    #[tokio::test]
    async fn test_status_returns_queue_depths() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .user_queue_depth(2)
                .system_queue_depth(1)
                .worker_threads(4)
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: StatusResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status.queue_depth_user, 2);
        assert_eq!(status.queue_depth_system, 1);
        assert_eq!(status.worker_threads, 4);
    }

    #[tokio::test]
    async fn test_chat_forwards_tools_to_llm() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat("Hello!", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Built-in tools should have been forwarded.
        let tools = mock.chat_tools();
        assert_eq!(tools.len(), 1);
        let forwarded = tools[0].as_ref().expect("tools should be forwarded");
        assert!(!forwarded.is_empty(), "at least one tool should be present");
        let names: Vec<String> = forwarded
            .iter()
            .filter_map(|t| t.get("function")?.get("name")?.as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(names.contains(&"get_current_time".to_string()));
        assert!(names.contains(&"echo".to_string()));
    }

    #[tokio::test]
    async fn test_chat_executes_tool_calls_and_returns_final_response() {
        let tool_call = ToolCall {
            index: 0,
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_current_time".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let first_response = Message {
            role: "assistant".to_string(),
            content: "".to_string(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_message(first_response, Usage::default())
                .push_chat("The current time is now.", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
        let app = super::build_app(state);

        let body =
            serde_json::to_string(&serde_json::json!({"message": "What time is it?"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let chat: ChatResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(chat.response, "The current time is now.");

        // Should have made two LLM calls: one for the tool call, one for the final answer.
        let calls = mock.chat_calls();
        assert_eq!(
            calls.len(),
            2,
            "expected two LLM calls (tool request + follow-up)"
        );
    }

    #[tokio::test]
    async fn test_chat_stream_forwards_tools_to_llm() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::Text("Hello!".to_string())),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Consume the SSE body to ensure the spawned task runs.
        let _bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        let tools = mock.stream_tools();
        assert_eq!(tools.len(), 1);
        let forwarded = tools[0].as_ref().expect("tools should be forwarded");
        assert!(!forwarded.is_empty(), "at least one tool should be present");
        let names: Vec<String> = forwarded
            .iter()
            .filter_map(|t| t.get("function")?.get("name")?.as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(names.contains(&"get_current_time".to_string()));
        assert!(names.contains(&"echo".to_string()));
    }

    #[tokio::test]
    async fn test_chat_stream_executes_tool_calls_and_returns_final_response() {
        let tool_call_delta = ToolCall {
            index: 0,
            id: "call_456".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_current_time".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                // First stream: tool call + usage
                .push_stream(vec![
                    Ok(StreamItem::ToolCalls(vec![tool_call_delta])),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                // Second stream (agentic loop): final text + usage
                .push_stream(vec![
                    Ok(StreamItem::Text("The current time is now.".to_string())),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;
        let app = super::build_app(state);

        let body =
            serde_json::to_string(&serde_json::json!({"message": "What time is it?"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        // The follow-up text should be streamed as a data event.
        assert!(
            text.contains("The current time is now."),
            "expected follow-up text in SSE stream, got: {}",
            text
        );

        // The tool_call SSE event should be present.
        assert!(
            text.contains("tool_call"),
            "expected tool_call event in SSE stream, got: {}",
            text
        );

        // The agentic loop should have made two stream calls.
        let calls = mock.stream_calls();
        assert_eq!(
            calls.len(),
            2,
            "expected two LLM stream calls (initial + agentic loop)"
        );
    }

    #[tokio::test]
    async fn test_chat_unknown_session_returns_404() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let body =
            serde_json::to_string(&serde_json::json!({"session_id": 999999, "message": "hello"}))
                .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_memory_returns_condensed_content() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;

        // Seed condensed memory in the knowledge graph
        state
            .knowledge_graph
            .set_condensed_memory("Test memory content from KG.")
            .await
            .unwrap();

        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/memory")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(text.contains("Test memory content from KG."));
    }

    #[tokio::test]
    async fn test_stop_returns_ok() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stop")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_stop_rejects_non_loopback() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/stop")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_server_exits_after_stop() {
        use mimir_core::config::ReloadableConfig;

        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("context.db");

        // Find an available port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let mut config = Config::default();
        config.llm.endpoint = "http://127.0.0.1:1".to_string();
        config.llm.api_key = "test".to_string();
        config.llm.model = "gpt-4o".to_string();
        config.llm.max_tokens = Some(10);
        config.llm.temperature = 0.0;
        config.server.bind_addr = addr.to_string();
        config.memory.char_limit = 10_000;
        config.context.db_path = Some(db_path);

        let config = Arc::new(ReloadableConfig::new(
            config,
            temp.path().join("config.toml"),
        ));
        let handle = tokio::spawn(async move { super::start_server(config).await });

        // Poll until the server accepts a TCP connection (up to 5 s).
        let poll_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut ready = false;
        while tokio::time::Instant::now() < poll_deadline {
            if handle.is_finished() {
                let result = handle.await.unwrap();
                panic!("server exited early: {:?}", result);
            }
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ready, "server did not become reachable within 5 seconds");

        // Send the stop request.
        let client = reqwest::Client::builder().http1_only().build().unwrap();
        let res = client.post(format!("http://{}/stop", addr)).send().await;

        match res {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                assert_eq!(
                    status,
                    reqwest::StatusCode::OK,
                    "unexpected status: {} body: {}",
                    status,
                    body
                );
            }
            Err(e) => {
                // Server may shut down before the response is fully read.
                // As long as the server exits below, the stop signal was received.
                eprintln!(
                    "Stop request got connection error (server shutting down): {}",
                    e
                );
            }
        }

        // The server should exit within 5 seconds.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "server did not exit within 5 seconds");
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
    }

    /// Regression: the drain timeout must bound only the post-signal drain
    /// phase, NOT the serving lifetime. Previously `tokio::time::timeout`
    /// wrapped the entire server future, so the daemon self-terminated
    /// `drain_timeout` after start even with no shutdown requested.
    #[tokio::test]
    async fn test_serve_outlives_drain_timeout() {
        use axum::routing::get;
        use std::time::Duration;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let app = axum::Router::new().route("/health", get(|| async { "ok" }));
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let trigger = shutdown_tx.clone();

        // 100 ms drain bound — far shorter than the 300 ms alive check below.
        let drain_timeout = Duration::from_millis(100);
        let handle = tokio::spawn(async move {
            super::serve_with_bounded_drain(listener, app, shutdown_tx, drain_timeout).await
        });

        // Wait until the server accepts connections (up to 5 s).
        let poll_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while tokio::time::Instant::now() < poll_deadline {
            if handle.is_finished() {
                let result = handle.await.unwrap();
                panic!("server exited early (before alive check): {:?}", result);
            }
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(ready, "server did not become reachable within 5 seconds");

        // Sleep well past the drain timeout. If the timeout bounded the
        // serving lifetime (the bug), the server would already be dead.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !handle.is_finished(),
            "server self-terminated after the drain timeout with no shutdown requested (the bug)"
        );

        // Request shutdown; the server should exit within the drain timeout.
        trigger.send(true).expect("send shutdown trigger");
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "server did not exit within 2s of shutdown");
        assert!(
            result.unwrap().is_ok(),
            "server task panicked or returned error"
        );
    }

    #[tokio::test]
    async fn test_sessions_returns_list() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let list: Vec<mimir_api_types::SessionSummary> =
            serde_json::from_slice(&body_bytes).unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn test_session_messages_returns_messages() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;

        let sid = state
            .context_manager
            .create_session("you are a test assistant")
            .await
            .unwrap();
        state
            .context_manager
            .add_user_message(sid, "hello")
            .await
            .unwrap();

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/sessions/{}/messages", sid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: mimir_api_types::SessionMessagesResponse =
            serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.session_id, sid);
        assert_eq!(resp.messages.len(), 2);
        assert_eq!(resp.messages[0].role, "system");
        assert_eq!(resp.messages[1].role, "user");
        assert_eq!(resp.messages[1].content, "hello");
    }

    #[tokio::test]
    async fn test_session_messages_unknown_session_returns_404() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions/not-a-session/messages")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_kg_tools_registered() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;

        let names: Vec<String> = state
            .tool_registry
            .list()
            .into_iter()
            .map(|m| m.name)
            .collect();

        assert!(names.contains(&"kg_query".to_string()));
        assert!(names.contains(&"kg_related".to_string()));
        assert!(names.contains(&"kg_search".to_string()));
        assert!(names.contains(&"expand_catalogue".to_string()));
        assert!(names.contains(&"get_facts_in_catalogue".to_string()));
        assert!(names.contains(&"remember".to_string()));
    }

    #[tokio::test]
    async fn test_kg_tools_in_openai_export() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;

        let exported = state.tool_registry.export_openai_tools();
        let names: Vec<String> = exported
            .iter()
            .filter_map(|v| {
                v.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        assert!(names.contains(&"kg_query".to_string()));
        assert!(names.contains(&"kg_related".to_string()));
        assert!(names.contains(&"kg_search".to_string()));
        assert!(names.contains(&"expand_catalogue".to_string()));
        assert!(names.contains(&"get_facts_in_catalogue".to_string()));
        assert!(names.contains(&"remember".to_string()));
    }

    #[tokio::test]
    async fn test_kb_optimization_status_returns_job() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/kb/optimization/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: mimir_api_types::OptimizationStatusResponse =
            serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.job_id, "knowledge.optimization");
        assert_eq!(resp.priority, "system");
        assert!(resp.schedule.is_some());
    }

    #[tokio::test]
    async fn test_kb_optimization_run_now_triggers_job() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/kb/optimization/run-now")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: mimir_api_types::OptimizationRunNowResponse =
            serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(resp.status, "succeeded");
    }

    #[tokio::test]
    async fn test_memory_refresh_non_loopback_rejected() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/memory/refresh")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_memory_refresh_not_registered_returns_404() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/memory/refresh")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_memory_refresh_already_running_returns_409() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;

        // Register a slow condensation job so we can race it.
        let slow_job = mimir_core::job_queue::Job::new(
            "memory.condensation",
            mimir_core::job_queue::JobPriority::System,
            None,
            true,
            |_ctx: mimir_core::job_queue::JobContext| {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    Ok(())
                })
            },
        );
        state.job_queue.register(slow_job).await.unwrap();

        let app = super::build_app(Arc::clone(&state));

        // Start a run in the background via the job queue directly.
        let jq = Arc::clone(&state.job_queue);
        let _bg = tokio::spawn(async move {
            let _ = jq.run_now("memory.condensation").await;
        });

        // Give the background task a moment to insert the Running row.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/memory/refresh")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_chat_injects_kg_memory_into_system_prompt() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat("Hello!", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;

        // Seed condensed memory in the knowledge graph
        state
            .knowledge_graph
            .set_condensed_memory("User enjoys hiking and sourdough bread.")
            .await
            .unwrap();

        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let calls = mock.chat_calls();
        assert_eq!(calls.len(), 1, "expected one LLM chat call");
        let messages = &calls[0];
        assert!(!messages.is_empty(), "expected at least one message");
        assert_eq!(messages[0].role, "system");
        let content = &messages[0].content;
        // Issue #138: core-facts framing is third person with operating
        // directives appended; legacy wording is gone.
        assert!(
            content.contains("Core facts about the user"),
            "system prompt should contain the core-facts header"
        );
        assert!(
            content.contains("User enjoys hiking and sourdough bread."),
            "system prompt should contain the seeded KG memory"
        );
        assert!(
            content.contains("retrieve_context"),
            "system prompt should contain the retrieve_context directive"
        );
        assert!(
            !content.contains("Key facts I know about you:"),
            "system prompt must not contain legacy 'Key facts I know about you:'"
        );
        assert!(
            !content.contains("kg_query"),
            "system prompt must not surface internal kg_query tool"
        );
    }

    #[tokio::test]
    async fn test_chat_stream_injects_kg_memory_into_system_prompt() {
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_stream(vec![
                    Ok(StreamItem::Text("Hello!".to_string())),
                    Ok(StreamItem::Usage(Usage::default())),
                ])
                .build(),
        );
        let (state, _temp) = test_state(mock.clone()).await;

        // Seed condensed memory in the knowledge graph
        state
            .knowledge_graph
            .set_condensed_memory("User enjoys hiking and sourdough bread.")
            .await
            .unwrap();

        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({"message": "hello"})).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Consume the SSE body to ensure the spawned task runs.
        let _bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        let calls = mock.stream_calls();
        assert_eq!(calls.len(), 1, "expected one LLM stream call");
        let messages = &calls[0];
        assert!(!messages.is_empty(), "expected at least one message");
        assert_eq!(messages[0].role, "system");
        let content = &messages[0].content;
        // Issue #138: core-facts framing is third person with operating
        // directives appended; legacy wording is gone.
        assert!(
            content.contains("Core facts about the user"),
            "system prompt should contain the core-facts header"
        );
        assert!(
            content.contains("User enjoys hiking and sourdough bread."),
            "system prompt should contain the seeded KG memory"
        );
        assert!(
            content.contains("retrieve_context"),
            "system prompt should contain the retrieve_context directive"
        );
        assert!(
            !content.contains("Key facts I know about you:"),
            "system prompt must not contain legacy 'Key facts I know about you:'"
        );
        assert!(
            !content.contains("kg_query"),
            "system prompt must not surface internal kg_query tool"
        );
    }

    // ------------------------------------------------------------------
    // KG CLI route tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_kb_query_returns_facts() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

        // Seed an entity and a fact
        let entity = state
            .knowledge_graph
            .create_entity(
                "Alice",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();
        let pred_id = state
            .knowledge_graph
            .ensure_relationship_type("works_at")
            .await
            .unwrap();
        let _fact = mimir_knowledge::queries::fact::insert_fact(
            state.knowledge_graph.pool(),
            &mimir_knowledge::models::fact::NewFact {
                subject_id: entity.id,
                relationship_type: "works_at".to_string(),
                object_id: None,
                object_literal: Some("Acme".to_string()),
                valid_from: None,
                valid_until: None,
                source_type: mimir_knowledge::models::source::SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: vec![],
                category_ids: vec![],
            },
            pred_id,
            0.95,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/kb/query?entity=Alice")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!json.get("facts").unwrap().as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_kb_show_returns_fact_detail() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
        let entity = state
            .knowledge_graph
            .create_entity(
                "Bob",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();
        let pred_id = state
            .knowledge_graph
            .ensure_relationship_type("likes")
            .await
            .unwrap();
        let fact = mimir_knowledge::queries::fact::insert_fact(
            state.knowledge_graph.pool(),
            &mimir_knowledge::models::fact::NewFact {
                subject_id: entity.id,
                relationship_type: "likes".to_string(),
                object_id: None,
                object_literal: Some("Pizza".to_string()),
                valid_from: None,
                valid_until: None,
                source_type: mimir_knowledge::models::source::SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: vec![],
                category_ids: vec![],
            },
            pred_id,
            0.88,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/kb/facts/{}", fact.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["fact"]["id"], fact.id);
    }

    #[tokio::test]
    async fn test_kb_browse_returns_edges() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
        let alice = state
            .knowledge_graph
            .create_entity(
                "Alice",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();
        let acme = state
            .knowledge_graph
            .create_entity(
                "Acme",
                mimir_knowledge::models::entity::EntityType::Organization,
                &[],
            )
            .await
            .unwrap();
        let pred_id = state
            .knowledge_graph
            .ensure_relationship_type("works_at")
            .await
            .unwrap();
        let _fact = mimir_knowledge::queries::fact::insert_fact(
            state.knowledge_graph.pool(),
            &mimir_knowledge::models::fact::NewFact {
                subject_id: alice.id,
                relationship_type: "works_at".to_string(),
                object_id: Some(acme.id),
                object_literal: None,
                valid_from: None,
                valid_until: None,
                source_type: mimir_knowledge::models::source::SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: vec![],
                category_ids: vec![],
            },
            pred_id,
            0.92,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/kb/browse?entity=Alice&depth=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!json.get("edges").unwrap().as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_kb_profile_returns_groups() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
        let entity = state
            .knowledge_graph
            .create_entity(
                "Charlie",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();
        let pred_id = state
            .knowledge_graph
            .ensure_relationship_type("enjoys")
            .await
            .unwrap();
        let _fact = mimir_knowledge::queries::fact::insert_fact(
            state.knowledge_graph.pool(),
            &mimir_knowledge::models::fact::NewFact {
                subject_id: entity.id,
                relationship_type: "enjoys".to_string(),
                object_id: None,
                object_literal: Some("Hiking".to_string()),
                valid_from: None,
                valid_until: None,
                source_type: mimir_knowledge::models::source::SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: vec![],
                category_ids: vec![],
            },
            pred_id,
            0.95,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/kb/profile?entity=Charlie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["entity_name"], "Charlie");
    }

    #[tokio::test]
    async fn test_kb_audit_returns_entries() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
        let entity = state
            .knowledge_graph
            .create_entity(
                "Dave",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();
        let pred_id = state
            .knowledge_graph
            .ensure_relationship_type("lives_in")
            .await
            .unwrap();
        let _fact = mimir_knowledge::queries::fact::insert_fact(
            state.knowledge_graph.pool(),
            &mimir_knowledge::models::fact::NewFact {
                subject_id: entity.id,
                relationship_type: "lives_in".to_string(),
                object_id: None,
                object_literal: Some("London".to_string()),
                valid_from: None,
                valid_until: None,
                source_type: mimir_knowledge::models::source::SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: vec![],
                category_ids: vec![],
            },
            pred_id,
            0.90,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/kb/audit?entity=Dave")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["entries"].as_array().unwrap();
        assert!(
            !entries.is_empty(),
            "expected at least one audit entry (Created)"
        );
        assert!(
            entries
                .iter()
                .any(|e| e["change_type"].as_str() == Some("created")),
            "expected a Created audit entry"
        );
    }

    #[tokio::test]
    async fn test_kb_forget_restore_trash_roundtrip() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;
        let entity = state
            .knowledge_graph
            .create_entity(
                "Eve",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();
        let pred_id = state
            .knowledge_graph
            .ensure_relationship_type("has")
            .await
            .unwrap();
        let fact = mimir_knowledge::queries::fact::insert_fact(
            state.knowledge_graph.pool(),
            &mimir_knowledge::models::fact::NewFact {
                subject_id: entity.id,
                relationship_type: "has".to_string(),
                object_id: None,
                object_literal: Some("Cat".to_string()),
                valid_from: None,
                valid_until: None,
                source_type: mimir_knowledge::models::source::SourceType::UserEdit,
                connector_id: None,
                connector_type: None,
                raw_reference: None,
                extraction_method: None,
                inferred: false,
                inference_depth: 0,
                confidence: None,
                parent_fact_ids: vec![],
                category_ids: vec![],
            },
            pred_id,
            0.85,
            chrono::Utc::now(),
        )
        .await
        .unwrap();

        let app = super::build_app(state);

        // Forget
        let _forget_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/kb/facts/forget")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"fact_id": fact.id}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        // List trash
        let trash_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/kb/trash")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(trash_resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(trash_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let trash_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!trash_json["items"].as_array().unwrap().is_empty());

        // Restore
        let _restore_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/kb/trash/restore")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::json!({"all": true}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_seed_identity_facts_creates_name_and_preferred_name() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

        // Create a user entity manually since test_state does not resolve identity.
        let entity = state
            .knowledge_graph
            .create_entity(
                "Alice Smith",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();

        // Seed identity facts
        crate::state::seed_identity_facts(
            &state.knowledge_graph,
            entity.id,
            "Alice Smith",
            "Alice",
        )
        .await
        .unwrap();

        // Verify facts exist
        let facts = state
            .knowledge_graph
            .get_facts_by_subject(entity.id, 1000)
            .await
            .unwrap();

        let mut found_name = false;
        let mut found_preferred = false;
        for fact in &facts {
            let pred = state
                .knowledge_graph
                .relationship_type_name(fact.relationship_type_id)
                .await;
            if pred.as_deref() == Some("has_name")
                && fact.object_literal.as_deref() == Some("Alice Smith")
            {
                found_name = true;
            }
            if pred.as_deref() == Some("preferred_name")
                && fact.object_literal.as_deref() == Some("Alice")
            {
                found_preferred = true;
            }
        }
        assert!(found_name, "expected has_name fact for Alice Smith");
        assert!(found_preferred, "expected preferred_name fact for Alice");
    }

    #[tokio::test]
    async fn test_seed_identity_facts_is_idempotent() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

        let entity = state
            .knowledge_graph
            .create_entity(
                "Bob",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();

        // Call twice with same values
        crate::state::seed_identity_facts(&state.knowledge_graph, entity.id, "Bob", "Bobby")
            .await
            .unwrap();
        crate::state::seed_identity_facts(&state.knowledge_graph, entity.id, "Bob", "Bobby")
            .await
            .unwrap();

        let facts = state
            .knowledge_graph
            .get_facts_by_subject(entity.id, 1000)
            .await
            .unwrap();

        let mut name_count = 0;
        let mut pref_count = 0;
        for f in &facts {
            let pred = state
                .knowledge_graph
                .relationship_type_name(f.relationship_type_id)
                .await;
            if f.status() == Some(mimir_knowledge::models::fact::FactStatus::Active) {
                if pred.as_deref() == Some("has_name") && f.object_literal.as_deref() == Some("Bob")
                {
                    name_count += 1;
                }
                if pred.as_deref() == Some("preferred_name")
                    && f.object_literal.as_deref() == Some("Bobby")
                {
                    pref_count += 1;
                }
            }
        }

        assert_eq!(name_count, 1, "expected exactly one active has_name fact");
        assert_eq!(
            pref_count, 1,
            "expected exactly one active preferred_name fact"
        );
    }

    #[tokio::test]
    async fn test_seed_identity_facts_adds_alias_and_merges_duplicate() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

        // Canonical entity
        let canonical = state
            .knowledge_graph
            .create_entity(
                "Devansh Bhavsar",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();

        // Bare-name duplicate (simulating old bug)
        let duplicate = state
            .knowledge_graph
            .create_entity(
                "Devansh",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();

        // Seed identity facts – should add alias and auto-merge duplicate
        crate::state::seed_identity_facts(
            &state.knowledge_graph,
            canonical.id,
            "Devansh Bhavsar",
            "Devansh",
        )
        .await
        .unwrap();

        // Alias should now exist
        let resolved =
            mimir_knowledge::queries::entity::get_by_name(state.knowledge_graph.pool(), "Devansh")
                .await
                .unwrap();
        assert!(!resolved.is_empty());
        assert_eq!(resolved[0].entity.id, canonical.id);
        assert_eq!(
            resolved[0].match_kind,
            mimir_knowledge::queries::entity::MatchKind::ExactAlias
        );

        // Duplicate entity should have been merged away
        let gone = state
            .knowledge_graph
            .get_entity(duplicate.id)
            .await
            .unwrap();
        assert!(gone.is_none(), "expected duplicate entity to be merged");
    }

    #[tokio::test]
    async fn test_seed_identity_facts_preserves_canonical_when_duplicate_has_more_facts() {
        let (state, _temp) = test_state(Arc::new(MockLlmClient::builder().build())).await;

        // Canonical entity with no facts yet
        let canonical = state
            .knowledge_graph
            .create_entity(
                "Devansh Bhavsar",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();

        // Duplicate entity that already has a couple of facts
        let duplicate = state
            .knowledge_graph
            .create_entity(
                "Devansh",
                mimir_knowledge::models::entity::EntityType::Person,
                &[],
            )
            .await
            .unwrap();

        // Give the duplicate two facts so it outranks the canonical pre-fix
        use mimir_knowledge::models::fact::NewFact;
        use mimir_knowledge::models::source::SourceType;
        let mut f1 = NewFact::new(duplicate.id, "has_name");
        f1.object_literal = Some("Devansh".to_string());
        f1.source_type = SourceType::System;
        let mut f2 = NewFact::new(duplicate.id, "works_at");
        f2.object_literal = Some("Acme".to_string());
        f2.source_type = SourceType::System;
        state
            .knowledge_graph
            .insert_facts_batch(vec![f1, f2])
            .await
            .unwrap();

        // Seed identity facts – canonical should survive because its facts are
        // inserted *before* the auto-merge check.
        crate::state::seed_identity_facts(
            &state.knowledge_graph,
            canonical.id,
            "Devansh Bhavsar",
            "Devansh",
        )
        .await
        .unwrap();

        // Canonical entity must still exist
        let canonical_still = state
            .knowledge_graph
            .get_entity(canonical.id)
            .await
            .unwrap();
        assert!(
            canonical_still.is_some(),
            "canonical entity must survive auto-merge"
        );

        // Duplicate entity should have been merged away
        let gone = state
            .knowledge_graph
            .get_entity(duplicate.id)
            .await
            .unwrap();
        assert!(gone.is_none(), "expected duplicate entity to be merged");
    }

    #[tokio::test]
    async fn test_chat_extracts_facts_after_response() {
        // Inline learning (issue #137): the conversational LLM calls the
        // `remember` tool while composing its reply, so facts are persisted
        // during the chat turn itself — no background Librarian required.
        let remember_output = mimir_knowledge::extract::RememberOutput {
            facts: vec![mimir_knowledge::extract::ExtractedFact {
                classification: mimir_knowledge::extract::Classification::Explicit,
                subject: "Devansh".to_string(),
                subject_type: "Person".to_string(),
                relationship_type: "favourite_colour".to_string(),
                object: "blue".to_string(),
                object_is_entity: false,
                object_type: None,
                temporal: None,
                is_sensitive: false,
                correction_scope: None,
                categories: vec![],
                recurrence: None,
                requires_user_action: None,
            }],
        };
        let extraction_msg = Message {
            role: "assistant".to_string(),
            content: "".to_string(),
            tool_calls: Some(vec![ToolCall {
                index: 0,
                id: "call_remember".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "remember".to_string(),
                    arguments: serde_json::to_string(&remember_output).unwrap(),
                },
            }]),
            tool_call_id: None,
        };

        // The LLM orchestrates learning inline: its first response calls the
        // `remember` tool to persist the fact, then it produces a final
        // acknowledgement. There is no separate background extraction pass.
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_message(extraction_msg, Usage::default())
                .push_chat("Got it!", Usage::default())
                .build(),
        );

        let mut config = Config::default();
        config.identity.name = "Devansh".to_string();
        let (state, _temp) = test_state_with_config(mock, config).await;
        let app = super::build_app(state.clone());

        let body = serde_json::to_string(&serde_json::json!({
            "message": "My favourite colour is blue."
        }))
        .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Poll with timeout so the test is deterministic, not timing-dependent.
        let mut found = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let search = state
                .knowledge_graph
                .search_entities("Devansh", 1)
                .await
                .unwrap();
            if search.is_empty() {
                continue;
            }
            let entity = &search[0].entity;

            let facts = state
                .knowledge_graph
                .get_facts_by_subject(entity.id, 100)
                .await
                .unwrap();

            for f in &facts {
                let pred = state
                    .knowledge_graph
                    .relationship_type_name(f.relationship_type_id)
                    .await;
                if pred.as_deref() == Some("favourite_colour")
                    && f.object_literal.as_deref() == Some("blue")
                {
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }

        assert!(
            found,
            "expected favourite_colour=blue fact to be extracted within 2.5s"
        );
    }

    #[tokio::test]
    async fn test_remember_tool_executes_and_writes_facts() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;

        // Call the remember tool directly through the registry.
        let args = serde_json::json!({
            "facts": [
                {
                    "classification": "Explicit",
                    "subject": "Alice",
                    "subject_type": "Person",
                    "relationship_type": "favourite_colour",
                    "object": "red",
                    "object_is_entity": false,
                    "is_sensitive": false,
                    "categories": []
                }
            ]
        });

        let output = state
            .tool_registry
            .execute("remember", args)
            .await
            .expect("remember tool should succeed");

        let text = output.to_llm_text();
        assert!(
            text.contains("inserted") || text.contains("matched"),
            "expected success text, got: {}",
            text
        );

        // Verify the fact exists.
        let search = state
            .knowledge_graph
            .search_entities("Alice", 1)
            .await
            .unwrap();
        assert!(!search.is_empty(), "expected entity 'Alice' to be created");
        let entity = &search[0].entity;

        let facts = state
            .knowledge_graph
            .get_facts_by_subject(entity.id, 100)
            .await
            .unwrap();

        let mut found = false;
        for f in &facts {
            let pred = state
                .knowledge_graph
                .relationship_type_name(f.relationship_type_id)
                .await;
            if pred.as_deref() == Some("favourite_colour")
                && f.object_literal.as_deref() == Some("red")
            {
                found = true;
                break;
            }
        }

        assert!(
            found,
            "expected favourite_colour=red fact to be written via remember tool"
        );
    }

    // ------------------------------------------------------------------
    // Pending sensitive-fact confirmation lifecycle (issue #141)
    // ------------------------------------------------------------------

    /// Insert a pending sensitive fact directly through the extraction
    /// pipeline and return its id.
    async fn insert_pending_fact(state: &Arc<AppState>, object: &str) -> i32 {
        use mimir_knowledge::extract::{
            Classification, ExtractedFact, RememberOutput, process_remember_output,
        };
        let outcome = process_remember_output(
            &state.knowledge_graph,
            RememberOutput {
                facts: vec![ExtractedFact {
                    classification: Classification::Explicit,
                    subject: "Devansh".to_string(),
                    subject_type: "Person".to_string(),
                    relationship_type: "allergy".to_string(),
                    object: object.to_string(),
                    object_is_entity: false,
                    object_type: None,
                    temporal: None,
                    is_sensitive: true,
                    correction_scope: None,
                    categories: Vec::new(),
                    recurrence: None,
                    requires_user_action: None,
                }],
            },
        )
        .await
        .unwrap();
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        outcome.pending_confirmation[0].fact_id
    }

    #[tokio::test]
    async fn test_kb_pending_lists_pending_facts() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let fact_id = insert_pending_fact(&state, "peanuts").await;

        let app = super::build_app(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/kb/pending")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: mimir_api_types::PendingListResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(resp.total, 1);
        assert_eq!(resp.facts[0].fact_id, fact_id);
        assert_eq!(resp.facts[0].subject, "Devansh");
        assert_eq!(resp.facts[0].predicate, "allergy");
        assert_eq!(resp.facts[0].object.as_deref(), Some("peanuts"));
    }

    #[tokio::test]
    async fn test_kb_confirm_returns_active_fact() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let fact_id = insert_pending_fact(&state, "shellfish").await;

        let app = super::build_app(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/kb/facts/{fact_id}/confirm"))
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let resp: mimir_api_types::ConfirmFactResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(resp.fact.id, fact_id);
        assert_eq!(resp.fact.status, "Active");
        assert!((resp.fact.confidence - 1.0).abs() < f32::EPSILON);

        // No longer pending.
        let pending = state.knowledge_graph.list_pending_facts().await.unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_kb_confirm_non_pending_returns_bad_request() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let fact_id = insert_pending_fact(&state, "pollen").await;
        state.knowledge_graph.confirm_fact(fact_id).await.unwrap();

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/kb/facts/{fact_id}/confirm"))
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_kb_reject_deletes_fact_and_returns_204() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let fact_id = insert_pending_fact(&state, "latex").await;

        let app = super::build_app(state.clone());
        let body = serde_json::to_string(&serde_json::json!({
            "reason": "entered in error"
        }))
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/kb/facts/{fact_id}/reject"))
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Fact hard-deleted.
        assert!(
            state
                .knowledge_graph
                .get_fact(fact_id)
                .await
                .unwrap()
                .is_none()
        );

        // Audit log carries the user reason.
        let audit = state.knowledge_graph.get_audit_log(fact_id).await.unwrap();
        assert!(
            audit
                .iter()
                .any(|a| a.reason.as_deref()
                    == Some("User rejected sensitive fact: entered in error"))
        );
    }

    #[tokio::test]
    async fn test_kb_reject_empty_body_returns_204() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let fact_id = insert_pending_fact(&state, "dust").await;

        let app = super::build_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/kb/facts/{fact_id}/reject"))
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_kb_pending_rejects_non_loopback() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/kb/pending")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_kb_confirm_rejects_non_loopback() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/kb/facts/1/confirm")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_kb_reject_rejects_non_loopback() {
        let mock = Arc::new(MockLlmClient::builder().build());
        let (state, _temp) = test_state(mock).await;
        let app = super::build_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/kb/facts/1/reject")
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [192, 168, 1, 1],
                        0,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_incognito_blocks_remember_tool_and_writes_no_facts() {
        let tool_call = ToolCall {
            index: 0,
            id: "call_remember".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: serde_json::json!({
                    "facts": [{
                        "classification": "Explicit",
                        "subject": "Incognito Test User",
                        "subject_type": "Person",
                        "relationship_type": "based_in",
                        "object": "London",
                        "object_is_entity": false,
                        "is_sensitive": false,
                        "categories": []
                    }]
                })
                .to_string(),
            },
        };
        let first = Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_message(first, Usage::default())
                .push_chat("Noted.", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let kg = Arc::clone(&state.knowledge_graph);
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({
            "message": "remember that I am based in London",
            "incognito": true,
        }))
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // No entity/fact should have been created during the incognito turn.
        let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
        assert!(
            found.is_empty(),
            "incognito turn must not persist entities, got: {found:?}"
        );
    }

    #[tokio::test]
    async fn test_non_incognito_allows_remember_tool_and_persists_fact() {
        // Control: the same tool call persists a fact when not incognito,
        // proving the incognito guard is what prevents writes (issue #155).
        let tool_call = ToolCall {
            index: 0,
            id: "call_remember".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: serde_json::json!({
                    "facts": [{
                        "classification": "Explicit",
                        "subject": "Incognito Test User",
                        "subject_type": "Person",
                        "relationship_type": "based_in",
                        "object": "London",
                        "object_is_entity": false,
                        "is_sensitive": false,
                        "categories": []
                    }]
                })
                .to_string(),
            },
        };
        let first = Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_message(first, Usage::default())
                .push_chat("Noted.", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let kg = Arc::clone(&state.knowledge_graph);
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({
            "message": "remember that I am based in London",
            "incognito": false,
        }))
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
        assert!(
            !found.is_empty(),
            "non-incognito turn should persist the entity/fact"
        );
    }

    #[tokio::test]
    async fn test_incognito_blocks_remember_tool_and_writes_no_facts_stream() {
        let tool_call = ToolCall {
            index: 0,
            id: "call_remember".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: serde_json::json!({
                    "facts": [{
                        "classification": "Explicit",
                        "subject": "Incognito Test User",
                        "subject_type": "Person",
                        "relationship_type": "based_in",
                        "object": "London",
                        "object_is_entity": false,
                        "is_sensitive": false,
                        "categories": []
                    }]
                })
                .to_string(),
            },
        };
        let first = Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_message(first, Usage::default())
                .push_chat("Noted.", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let kg = Arc::clone(&state.knowledge_graph);
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({
            "message": "remember that I am based in London",
            "incognito": true,
        }))
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Drain the SSE response body to ensure stream processing completes.
        let _bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        // No entity/fact should have been created during the incognito turn.
        let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
        assert!(
            found.is_empty(),
            "incognito turn must not persist entities, got: {found:?}"
        );
    }

    #[tokio::test]
    async fn test_non_incognito_allows_remember_tool_and_persists_fact_stream() {
        // Control: the same tool call persists a fact when not incognito,
        // proving the incognito guard is what prevents writes (issue #155).
        let tool_call = ToolCall {
            index: 0,
            id: "call_remember".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "remember".to_string(),
                arguments: serde_json::json!({
                    "facts": [{
                        "classification": "Explicit",
                        "subject": "Incognito Test User",
                        "subject_type": "Person",
                        "relationship_type": "based_in",
                        "object": "London",
                        "object_is_entity": false,
                        "is_sensitive": false,
                        "categories": []
                    }]
                })
                .to_string(),
            },
        };
        let first = Message {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![tool_call]),
            tool_call_id: None,
        };
        let mock = Arc::new(
            MockLlmClient::builder()
                .push_chat_message(first, Usage::default())
                .push_chat("Noted.", Usage::default())
                .build(),
        );
        let (state, _temp) = test_state(mock).await;
        let kg = Arc::clone(&state.knowledge_graph);
        let app = super::build_app(state);

        let body = serde_json::to_string(&serde_json::json!({
            "message": "remember that I am based in London",
            "incognito": false,
        }))
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/chat/stream")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let found = kg.search_entities("Incognito Test User", 10).await.unwrap();
        assert!(
            !found.is_empty(),
            "non-incognito turn should persist the entity/fact"
        );
    }
}
