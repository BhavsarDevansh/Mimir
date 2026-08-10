//! HTTP app assembly: the axum router, middleware, and loopback guard.
#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

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

use crate::routes::{
    chat_handler, chat_stream_handler, connector_actions_handler, connector_add_handler,
    connector_forget_handler, connector_pause_handler, connector_remove_handler,
    connector_resume_handler, connector_show_handler, connector_sync_handler,
    connector_tokens_handler, connectors_list_handler, create_category, delete_category,
    kb_audit_handler, kb_browse_handler, kb_confirm_fact_handler, kb_edit_handler,
    kb_forget_handler, kb_optimization_run_now_handler, kb_optimization_status_handler,
    kb_pending_handler, kb_profile_handler, kb_query_handler, kb_reject_fact_handler,
    kb_show_handler, kb_trash_empty_handler, kb_trash_list_handler, kb_trash_restore_handler,
    list_categories, memory_handler, memory_refresh_handler, session_messages_handler,
    sessions_handler, show_category, status_handler, stop_handler,
};
use crate::state::AppState;

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
        .route(
            "/connectors",
            get(connectors_list_handler).post(connector_add_handler),
        )
        .route(
            "/connectors/{id}",
            get(connector_show_handler).delete(connector_remove_handler),
        )
        .route("/connectors/{id}/sync", post(connector_sync_handler))
        .route("/connectors/{id}/pause", post(connector_pause_handler))
        .route("/connectors/{id}/resume", post(connector_resume_handler))
        .route(
            "/connectors/{id}/tokens",
            post(connector_tokens_handler).layer(from_fn(require_loopback)),
        )
        .route("/connectors/{id}/actions", post(connector_actions_handler))
        .route(
            "/connectors/{id}/forget",
            post(connector_forget_handler).layer(from_fn(require_loopback)),
        )
        .route("/stop", post(stop_handler).layer(from_fn(require_loopback)))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(cors),
        )
        .with_state(state)
}
