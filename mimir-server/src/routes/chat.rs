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

    let cfg = state.config.snapshot().await;

    let personality = if let Some(ref preset) = req.personality_preset {
        Personality::new(&mimir_core::config::PersonalityConfig {
            preset: preset.clone(),
        })
    } else {
        Personality::new(&cfg.personality)
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

        state
            .context_manager
            .trim_to_budget(&session_id, cfg.context.max_tokens, cfg.context.max_turns)
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

    let tools_opt = state.tool_registry.export_openai_tools_for_llm();
    let (assistant_msg, usage) = llm
        .chat_message(messages.clone(), tools_opt)
        .await
        .map_err(error::llm_error)?;

    let (response_text, final_usage) = if let Some(ref tool_calls) = assistant_msg.tool_calls {
        // The model issued tool calls — execute them and ask again.
        let tool_calls = tool_calls.clone();
        let mut follow_up = messages;
        follow_up.push(assistant_msg);

        for tool_call in &tool_calls {
            let result = match state
                .tool_registry
                .execute(
                    &tool_call.function.name,
                    serde_json::from_str(&tool_call.function.arguments)
                        .unwrap_or(serde_json::Value::Null),
                )
                .await
            {
                Ok(output) => output.to_llm_text(),
                Err(e) => format!("Tool error: {e}"),
            };
            follow_up.push(mimir_core::llm::types::Message::tool(&tool_call.id, result));
        }

        let (final_msg, final_usage) = llm
            .chat_message(follow_up, None)
            .await
            .map_err(error::llm_error)?;
        (final_msg.content, final_usage)
    } else {
        (assistant_msg.content, usage)
    };

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
            prompt_tokens: final_usage.prompt_tokens,
            completion_tokens: final_usage.completion_tokens,
            total_tokens: final_usage.total_tokens,
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

    let tools_opt = state.tool_registry.export_openai_tools_for_llm();
    // Keep a clone of messages so we can build the follow-up conversation if the
    // model decides to issue tool calls mid-stream.
    let messages_for_follow_up = messages.clone();
    let mut stream = llm
        .chat_stream_with_usage(messages, tools_opt)
        .await
        .map_err(error::llm_error)?;

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Event>(16);
    let state_clone = Arc::clone(&state);
    let session_id_clone = session_id.clone();
    let llm_clone = Arc::clone(&llm);

    tokio::spawn(async move {
        let _permit = permit;

        let mut full_response = String::new();
        let mut all_sends_ok = true;
        let mut assistant_persisted = false;
        // Accumulate partial tool-call deltas keyed by index.
        let mut tool_calls_acc: std::collections::HashMap<u32, mimir_core::llm::types::ToolCall> =
            std::collections::HashMap::new();

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
                Ok(StreamItem::ToolCalls(deltas)) => {
                    for delta in deltas {
                        let entry = tool_calls_acc.entry(delta.index).or_default();
                        if !delta.id.is_empty() {
                            entry.id = delta.id;
                        }
                        if !delta.call_type.is_empty() {
                            entry.call_type = delta.call_type;
                        }
                        if !delta.function.name.is_empty() {
                            entry.function.name = delta.function.name;
                        }
                        entry.function.arguments.push_str(&delta.function.arguments);
                    }
                }
                Ok(StreamItem::Usage(usage)) => {
                    // If the model issued tool calls during the stream, drain
                    // the accumulated deltas, execute the tools, and make a
                    // follow-up (non-streaming) call to obtain the final text.
                    let final_text = if !tool_calls_acc.is_empty() {
                        let mut follow_up = messages_for_follow_up.clone();
                        let assistant_tool_msg = mimir_core::llm::types::Message {
                            role: "assistant".to_string(),
                            content: full_response.clone(),
                            tool_calls: Some(tool_calls_acc.values().cloned().collect()),
                            tool_call_id: None,
                        };
                        follow_up.push(assistant_tool_msg);

                        for tool_call in tool_calls_acc.values() {
                            let result = match state_clone
                                .tool_registry
                                .execute(
                                    &tool_call.function.name,
                                    serde_json::from_str(&tool_call.function.arguments)
                                        .unwrap_or(serde_json::Value::Null),
                                )
                                .await
                            {
                                Ok(output) => output.to_llm_text(),
                                Err(e) => format!("Tool error: {e}"),
                            };
                            follow_up
                                .push(mimir_core::llm::types::Message::tool(&tool_call.id, result));
                        }

                        match llm_clone.chat(follow_up, None).await {
                            Ok((text, _usage)) => text,
                            Err(e) => {
                                error!("follow-up LLM call after tool calls failed: {e}");
                                let _ = event_tx
                                    .send(
                                        Event::default()
                                            .event("error")
                                            .data("internal server error"),
                                    )
                                    .await;
                                String::new()
                            }
                        }
                    } else {
                        full_response.clone()
                    };

                    full_response = final_text.clone();

                    // If we resolved tool calls and produced final text that
                    // was not already streamed, send it now before the usage event.
                    if !final_text.is_empty() {
                        let event = Event::default().data(final_text.clone());
                        if event_tx.send(event).await.is_err() {
                            all_sends_ok = false;
                        }
                    }

                    let json = serde_json::to_string(&usage).unwrap_or_default();
                    let event = Event::default().event("usage").data(json);
                    let send_ok = event_tx.send(event).await.is_ok();
                    if !send_ok {
                        all_sends_ok = false;
                    }
                    if !incognito && all_sends_ok && !final_text.is_empty() {
                        match state_clone
                            .context_manager
                            .add_assistant_message(&session_id_clone, &final_text)
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

        // If the stream ended without emitting a usage block (some providers
        // do this), but we accumulated tool calls, execute them now.
        if !tool_calls_acc.is_empty() && !assistant_persisted {
            let mut follow_up = messages_for_follow_up.clone();
            let assistant_tool_msg = mimir_core::llm::types::Message {
                role: "assistant".to_string(),
                content: full_response.clone(),
                tool_calls: Some(tool_calls_acc.values().cloned().collect()),
                tool_call_id: None,
            };
            follow_up.push(assistant_tool_msg);

            for tool_call in tool_calls_acc.values() {
                let result = match state_clone
                    .tool_registry
                    .execute(
                        &tool_call.function.name,
                        serde_json::from_str(&tool_call.function.arguments)
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .await
                {
                    Ok(output) => output.to_llm_text(),
                    Err(e) => format!("Tool error: {e}"),
                };
                follow_up.push(mimir_core::llm::types::Message::tool(&tool_call.id, result));
            }

            match llm_clone.chat(follow_up, None).await {
                Ok((text, _usage)) => {
                    full_response.clone_from(&text);
                    let event = Event::default().data(text);
                    if event_tx.send(event).await.is_ok() && !incognito {
                        let _ = state_clone
                            .context_manager
                            .add_assistant_message(&session_id_clone, &full_response)
                            .await;
                    }
                }
                Err(e) => {
                    error!("follow-up LLM call after tool calls failed: {e}");
                    let _ = event_tx
                        .send(
                            Event::default()
                                .event("error")
                                .data("internal server error"),
                        )
                        .await;
                }
            }
        } else if !incognito
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
