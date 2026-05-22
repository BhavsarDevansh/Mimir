use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json,
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use mimir_core::llm::types::{LlmError, StreamItem};

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

    // The spawned task owns the session lock for the entire stream lifetime.
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Event>(16);
    let state_clone = Arc::clone(&state);
    let session_id_clone = session_id.clone();
    let message_clone = req.message.clone();

    tokio::spawn(async move {
        let sem = state_clone.session_semaphore(&session_id_clone);
        let _permit = match sem.acquire().await {
            Ok(p) => p,
            Err(_) => return,
        };

        if let Err(e) = state_clone
            .context_manager
            .add_user_message(&session_id_clone, &message_clone)
            .await
        {
            let _ = event_tx
                .send(Event::default().event("error").data(e.to_string()))
                .await;
            return;
        }

        let messages = match state_clone
            .context_manager
            .export_messages(&session_id_clone)
            .await
        {
            Ok(msgs) => msgs,
            Err(e) => {
                let _ = event_tx
                    .send(Event::default().event("error").data(e.to_string()))
                    .await;
                return;
            }
        };

        match state_clone
            .llm_client
            .chat_stream_with_usage(messages)
            .await
        {
            Ok(mut stream) => {
                let mut full_response = String::new();
                let mut assistant_persisted = false;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(StreamItem::Text(text)) => {
                            full_response.push_str(&text);
                            let event = Event::default().data(text);
                            if event_tx.send(event).await.is_err() {
                                // Client disconnected.
                                break;
                            }
                        }
                        Ok(StreamItem::Usage(usage)) => {
                            // Persist assistant message before sending usage event.
                            if let Err(e) = state_clone
                                .context_manager
                                .add_assistant_message(&session_id_clone, &full_response)
                                .await
                            {
                                let _ = event_tx
                                    .send(Event::default().event("error").data(e.to_string()))
                                    .await;
                                break;
                            }
                            assistant_persisted = true;
                            let json = serde_json::to_string(&usage).unwrap_or_default();
                            let event = Event::default().event("usage").data(json);
                            let _ = event_tx.send(event).await;
                        }
                        Err(e) => {
                            let event = Event::default().event("error").data(e.to_string());
                            let _ = event_tx.send(event).await;
                            break;
                        }
                    }
                }
                // If the provider never emitted a usage block, persist the
                // accumulated response now so the session is not left incomplete.
                if !assistant_persisted
                    && !full_response.is_empty()
                    && let Err(e) = state_clone
                        .context_manager
                        .add_assistant_message(&session_id_clone, &full_response)
                        .await
                {
                    let _ = event_tx
                        .send(Event::default().event("error").data(e.to_string()))
                        .await;
                }
            }
            Err(LlmError::QueueFull) => {
                let event = Event::default()
                    .event("error")
                    .data("server busy, try again later");
                let _ = event_tx.send(event).await;
            }
            Err(e) => {
                let event = Event::default().event("error").data(e.to_string());
                let _ = event_tx.send(event).await;
            }
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
