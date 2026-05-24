use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json,
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use mimir_api_types::{ChatRequest, ChatResponse, Usage};
use mimir_core::llm::types::StreamItem;
use mimir_core::personality::Personality;

use tracing::error;

use crate::error;
use crate::state::AppState;

/// Resolve the common chat state shared by both the blocking and streaming handlers.
///
/// Returns `(session_id, llm, messages, incognito, permit_option)` where
/// `permit_option` is `Some` only when the session is non-incognito and the
/// caller must hold the permit until after the assistant response is persisted.
async fn resolve_chat_state(
    state: &Arc<AppState>,
    req: &ChatRequest,
) -> Result<
    (
        String,
        Arc<dyn mimir_core::llm::LlmBackend>,
        Vec<mimir_core::llm::types::Message>,
        bool,
        Option<tokio::sync::OwnedSemaphorePermit>,
    ),
    axum::response::Response,
> {
    let memory = tokio::fs::read_to_string(&state.memory_path)
        .await
        .unwrap_or_default();

    let incognito = req.incognito == Some(true);

    let personality = if let Some(ref preset) = req.personality_preset {
        Personality::new(&mimir_core::config::PersonalityConfig {
            preset: preset.clone(),
        })
    } else {
        state.personality.clone()
    };

    let llm = state.resolve_llm(req.model.clone());

    let session_id = if incognito {
        uuid::Uuid::new_v4().to_string()
    } else {
        match &req.session_id {
            Some(id) => match state.context_manager.export_messages(id).await {
                Ok(_) => id.clone(),
                Err(mimir_core::context::ContextError::SessionNotFound(_)) => {
                    return Err(error::session_not_found());
                }
                Err(e) => return Err(error::context_error(e)),
            },
            None => {
                let system_prompt = personality.system_prompt(&memory);
                state
                    .context_manager
                    .create_session(system_prompt)
                    .await
                    .map_err(error::context_error)?
            }
        }
    };

    if incognito {
        let system_prompt = personality.system_prompt(&memory);
        let messages = vec![
            mimir_core::llm::types::Message::system(&system_prompt),
            mimir_core::llm::types::Message::user(&req.message),
        ];
        Ok((session_id, llm, messages, incognito, None))
    } else {
        let permit = state
            .session_semaphore(&session_id)
            .acquire_owned()
            .await
            .map_err(|_| {
                error!("session semaphore closed");
                error::internal("internal server error")
            })?;

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

        Ok((session_id, llm, messages, incognito, Some(permit)))
    }
}

/// Blocking chat completion endpoint.
///
/// 1. Validates or creates a session (unless incognito).
/// 2. Persists the user message (unless incognito).
/// 3. Delegates to the LLM worker pool, with optional model override.
/// 4. Persists the assistant response and returns it (unless incognito).
pub async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, axum::response::Response> {
    let (session_id, llm, messages, incognito, _permit) = resolve_chat_state(&state, &req).await?;

    let (response_text, usage) = llm.chat(messages).await.map_err(error::llm_error)?;

    if !incognito {
        state
            .context_manager
            .add_assistant_message(&session_id, &response_text)
            .await
            .map_err(error::context_error)?;
    }

    Ok(Json(ChatResponse {
        session_id,
        response: response_text,
        usage: Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        },
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
    let (session_id, llm, messages, incognito, permit) = resolve_chat_state(&state, &req).await?;

    let mut stream = llm
        .chat_stream_with_usage(messages)
        .await
        .map_err(error::llm_error)?;

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Event>(16);
    let state_clone = Arc::clone(&state);
    let session_id_clone = session_id.clone();

    tokio::spawn(async move {
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
                    if !incognito && all_sends_ok && !full_response.is_empty() {
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

        if !incognito
            && !assistant_persisted
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
