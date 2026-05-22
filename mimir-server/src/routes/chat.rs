use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json,
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use mimir_core::llm::types::StreamItem;

use tracing::error;

use crate::error;
use crate::state::AppState;
use crate::types::{ChatRequest, ChatResponse};

/// Blocking chat completion endpoint.
///
/// 1. Validates or creates a session.
/// 2. Persists the user message.
/// 3. Delegates to the LLM worker pool.
/// 4. Persists the assistant response and returns it.
pub async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, axum::response::Response> {
    let memory = tokio::fs::read_to_string(&state.memory_path)
        .await
        .unwrap_or_default();

    let session_id = match &req.session_id {
        Some(id) => {
            // Verify the session exists, distinguishing not-found from real errors.
            match state.context_manager.export_messages(id).await {
                Ok(_) => id.clone(),
                Err(mimir_core::context::ContextError::SessionNotFound(_)) => {
                    return Err(error::session_not_found());
                }
                Err(e) => return Err(error::context_error(e)),
            }
        }
        None => {
            let system_prompt = state.personality.system_prompt(&memory);
            state
                .context_manager
                .create_session(system_prompt)
                .await
                .map_err(error::context_error)?
        }
    };

    // Serialise per-session access.
    let sem = state.session_semaphore(&session_id);
    let _permit = sem.acquire().await.expect("semaphore never closed");

    state
        .context_manager
        .add_user_message(&session_id, &req.message)
        .await
        .map_err(error::context_error)?;

    let messages = state
        .context_manager
        .export_messages(&session_id)
        .await
        .map_err(error::context_error)?;

    let (response_text, usage) = state
        .llm_client
        .chat(messages)
        .await
        .map_err(error::llm_error)?;

    state
        .context_manager
        .add_assistant_message(&session_id, &response_text)
        .await
        .map_err(error::context_error)?;

    Ok(Json(ChatResponse {
        session_id,
        response: response_text,
        usage,
    }))
}

/// SSE streaming chat completion endpoint.
///
/// Spawns a background task that holds the session lock for the duration of
/// the stream so that concurrent requests for the same session are serialised.
pub async fn chat_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<
    Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>,
    axum::response::Response,
> {
    let memory = tokio::fs::read_to_string(&state.memory_path)
        .await
        .unwrap_or_default();

    let session_id = match &req.session_id {
        Some(id) => {
            // Verify the session exists, distinguishing not-found from real errors.
            match state.context_manager.export_messages(id).await {
                Ok(_) => id.clone(),
                Err(mimir_core::context::ContextError::SessionNotFound(_)) => {
                    return Err(error::session_not_found());
                }
                Err(e) => return Err(error::context_error(e)),
            }
        }
        None => {
            let system_prompt = state.personality.system_prompt(&memory);
            state
                .context_manager
                .create_session(system_prompt)
                .await
                .map_err(error::context_error)?
        }
    };

    // Acquire the session lock synchronously so the enqueue and stream
    // processing are serialised per-session.
    let permit = state
        .session_semaphore(&session_id)
        .acquire_owned()
        .await
        .map_err(|_| {
            error!("session semaphore closed");
            error::internal("internal server error")
        })?;

    // Persist the user message before enqueuing so the 200 response is
    // only committed after both the message is stored and the stream
    // handle is obtained (real enqueue, no TOCTOU race).
    state
        .context_manager
        .add_user_message(&session_id, &req.message)
        .await
        .map_err(error::context_error)?;

    // Export messages and enqueue the stream immediately.
    // If the queue is full we return a proper 503 error response instead
    // of committing a 200 and sending an error event later.
    let messages = state
        .context_manager
        .export_messages(&session_id)
        .await
        .map_err(error::context_error)?;

    let mut stream = state
        .llm_client
        .chat_stream_with_usage(messages)
        .await
        .map_err(error::llm_error)?;

    // Build the SSE channel and spawn the streaming task.
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Event>(16);
    let state_clone = Arc::clone(&state);
    let session_id_clone = session_id.clone();

    tokio::spawn(async move {
        // Keep the session permit alive for the entire stream lifetime.
        let _permit = permit;

        let mut full_response = String::new();
        let mut all_sends_ok = true;
        let mut assistant_persisted = false;

        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamItem::Text(text)) => {
                    full_response.push_str(&text);
                    let event = Event::default().data(text);
                    if event_tx.send(event).await.is_err() {
                        all_sends_ok = false;
                        // Drop the stream immediately so the upstream provider
                        // can be cancelled, instead of silently draining it.
                        drop(stream);
                        break;
                    }
                }
                Ok(StreamItem::Usage(usage)) => {
                    let json = serde_json::to_string(&usage).unwrap_or_default();
                    let event = Event::default().event("usage").data(json);
                    let send_ok = event_tx.send(event).await.is_ok();
                    if !send_ok {
                        all_sends_ok = false;
                    }
                    // Persist the assistant message only when the client
                    // received all data and the context write succeeds.
                    if all_sends_ok && !full_response.is_empty() {
                        match state_clone
                            .context_manager
                            .add_assistant_message(&session_id_clone, &full_response)
                            .await
                        {
                            Ok(()) => {
                                assistant_persisted = true;
                            }
                            Err(e) => {
                                error!("failed to persist assistant message: {e}");
                                let _ = event_tx
                                    .send(
                                        Event::default()
                                            .event("error")
                                            .data("internal server error"),
                                    )
                                    .await;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("LLM stream error: {e}");
                    let event = Event::default()
                        .event("error")
                        .data("internal server error");
                    let _ = event_tx.send(event).await;
                    break;
                }
            }
        }

        // If the provider never emitted a usage block, and all sends
        // succeeded, persist the accumulated response so the session
        // is not left incomplete.
        if !assistant_persisted
            && all_sends_ok
            && !full_response.is_empty()
            && let Err(e) = state_clone
                .context_manager
                .add_assistant_message(&session_id_clone, &full_response)
                .await
        {
            error!("failed to persist assistant message: {e}");
            let _ = event_tx
                .send(
                    Event::default()
                        .event("error")
                        .data("internal server error"),
                )
                .await;
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(event_rx)
        .map(Ok::<_, std::convert::Infallible>);

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keep-alive"),
    ))
}
