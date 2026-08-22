use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use axum::{
    Json,
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use mimir_api_types::{ChatRequest, ChatResponse, Usage};
use mimir_core::conversation::ConversationTurn;
use mimir_core::hooks::Trigger;
use mimir_core::llm::types::StreamItem;
use mimir_core::personality::Personality;
use tracing::error;

use crate::error;
use crate::state::AppState;

static INCOGNITO_COUNTER: AtomicI64 = AtomicI64::new(-1);

/// Build a catalogue appendix for the system prompt.
async fn build_catalogue(knowledge_graph: &mimir_knowledge::KnowledgeGraph) -> String {
    match knowledge_graph.get_top_level_catalogue().await {
        Ok(cats) if !cats.is_empty() => {
            let mut lines = vec!["## Knowledge Catalogue".to_string()];
            for cat in cats {
                lines.push(format!("{} {}", cat.id, cat.name));
            }
            lines.join("\n")
        }
        _ => String::new(),
    }
}

/// Execute a single tool call through the registry with the per-request
/// context, so every tool — including `retrieve_context` — flows through the
/// same dispatch path with uniform permission checks (issue #441).
async fn execute_tool_call(
    registry: &mimir_core::tools::ToolRegistry,
    tool_name: &str,
    tool_arguments: &str,
    llm: Arc<dyn mimir_core::llm::LlmBackend>,
    incognito: bool,
) -> Result<mimir_core::tools::ToolOutput, mimir_core::tools::ToolError> {
    let args = serde_json::from_str(tool_arguments).unwrap_or(serde_json::Value::Null);
    let ctx = mimir_core::tools::ToolContext::new(llm, !incognito);
    registry.execute(tool_name, args, &ctx).await
}

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
        i64,
        Arc<dyn mimir_core::llm::LlmBackend>,
        Vec<mimir_core::llm::types::Message>,
        bool,
        Option<tokio::sync::OwnedSemaphorePermit>,
    ),
    axum::response::Response,
> {
    let condensed = match state.knowledge_graph.get_condensed_memory().await {
        Ok(Some(text)) => text,
        Ok(None) => String::new(),
        Err(e) => {
            tracing::warn!("Failed to read condensed memory for chat: {}", e);
            String::new()
        }
    };
    let cfg = state.config.snapshot().await;
    let upcoming = if let Some(uid) = state.user_entity_id {
        match state
            .knowledge_graph
            .render_upcoming_section(uid, cfg.memory.temporal_horizon as i64, 10)
            .await
        {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("Failed to render upcoming section for chat: {}", e);
                String::new()
            }
        }
    } else {
        String::new()
    };
    let memory = if upcoming.is_empty() {
        condensed
    } else {
        format!(
            "{}

{}",
            condensed, upcoming
        )
    };

    let incognito = req.incognito == Some(true);

    let personality = if let Some(ref preset) = req.personality_preset {
        Personality::new(&mimir_core::config::PersonalityConfig {
            preset: preset.clone(),
        })
    } else {
        Personality::new(&cfg.personality)
    };

    let llm = state.resolve_llm(req.model.clone());
    // Apply the live configuration temperature so hot-reloaded changes take
    // effect without restarting the daemon (issue #80).
    let llm = llm
        .with_temperature_override(cfg.llm.temperature)
        .unwrap_or(llm);

    let session_id = if incognito {
        INCOGNITO_COUNTER.fetch_sub(1, Ordering::SeqCst)
    } else {
        match &req.session_id {
            Some(id) => match state.context_manager.export_messages(*id).await {
                Ok(_) => *id,
                Err(mimir_core::context::ContextError::SessionNotFound(_)) => {
                    return Err(error::session_not_found());
                }
                Err(e) => return Err(error::context_error(e)),
            },
            None => {
                let catalogue = build_catalogue(&state.knowledge_graph).await;
                let system_prompt = if catalogue.is_empty() {
                    personality.system_prompt(&memory)
                } else {
                    format!(
                        "{}

{}",
                        personality.system_prompt(&memory),
                        catalogue
                    )
                };
                state
                    .context_manager
                    .create_session(system_prompt)
                    .await
                    .map_err(error::context_error)?
            }
        }
    };

    if incognito {
        let catalogue = build_catalogue(&state.knowledge_graph).await;
        let system_prompt = if catalogue.is_empty() {
            personality.system_prompt(&memory)
        } else {
            format!(
                "{}

{}",
                personality.system_prompt(&memory),
                catalogue
            )
        };
        let messages = vec![
            mimir_core::llm::types::Message::system(&system_prompt),
            mimir_core::llm::types::Message::user(&req.message),
        ];
        Ok((session_id, llm, messages, incognito, None))
    } else {
        let permit = state
            .session_semaphore(session_id)
            .acquire_owned()
            .await
            .map_err(|_| {
                error!("session semaphore closed");
                error::internal("internal server error")
            })?;

        state
            .context_manager
            .add_user_message(session_id, &req.message)
            .await
            .map_err(error::context_error)?;

        state
            .context_manager
            .trim_to_budget(session_id, cfg.context.max_tokens, cfg.context.max_turns)
            .await
            .map_err(error::context_error)?;

        let messages = state
            .context_manager
            .export_messages(session_id)
            .await
            .map_err(error::context_error)?;

        Ok((session_id, llm, messages, incognito, Some(permit)))
    }
}

/// Blocking chat completion endpoint with agentic tool loop.
///
/// 1. Validates or creates a session (unless incognito).
/// 2. Persists the user message (unless incognito).
/// 3. Delegates to the LLM, executing tool calls in a loop up to
///    `max_tool_rounds` rounds.
/// 4. Persists the final assistant response and returns it (unless incognito).
pub async fn chat_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, axum::response::Response> {
    let (session_id, llm, messages, incognito, _permit) = resolve_chat_state(&state, &req).await?;
    state.record_user_activity();

    let max_rounds = state.config.snapshot().await.agent.max_tool_rounds;
    let tools_opt = state
        .tool_registry
        .export_openai_tools_for_llm_with_writes(!incognito);

    let mut conversation = messages;
    let mut tool_call_info: Vec<mimir_api_types::ToolCallInfo> = Vec::new();
    let mut round: u16 = 0;

    let (response_text, final_usage) = loop {
        let (assistant_msg, usage) = llm
            .chat_message(
                conversation.clone(),
                if round < max_rounds {
                    tools_opt.clone()
                } else {
                    None
                },
            )
            .await
            .map_err(error::llm_error)?;

        match assistant_msg.tool_calls {
            Some(ref tool_calls) if round < max_rounds => {
                round += 1;
                let tool_calls = tool_calls.clone();
                conversation.push(assistant_msg);

                for tool_call in &tool_calls {
                    let display_name = state
                        .tool_registry
                        .get_display_name(&tool_call.function.name)
                        .unwrap_or_else(|| {
                            mimir_core::tools::snake_to_title_case(&tool_call.function.name)
                        });

                    let output = match execute_tool_call(
                        &state.tool_registry,
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                        Arc::clone(&llm),
                        incognito,
                    )
                    .await
                    {
                        Ok(output) => output,
                        Err(e) => {
                            error!("tool '{}' execution failed: {e}", tool_call.function.name);
                            tool_call_info.push(mimir_api_types::ToolCallInfo {
                                name: tool_call.function.name.clone(),
                                display_name,
                                result: mimir_api_types::ToolCallInfo::truncate_result(&format!(
                                    "Tool error: {e}"
                                )),
                            });
                            conversation.push(mimir_core::llm::types::Message::tool(
                                &tool_call.id,
                                format!("Tool error: {e}"),
                            ));
                            continue;
                        }
                    };

                    let llm_text = output.to_llm_text();
                    let display_text = output.to_display_text();

                    tool_call_info.push(mimir_api_types::ToolCallInfo {
                        name: tool_call.function.name.clone(),
                        display_name,
                        result: mimir_api_types::ToolCallInfo::truncate_result(&display_text),
                    });

                    conversation.push(mimir_core::llm::types::Message::tool(
                        &tool_call.id,
                        llm_text,
                    ));
                }
            }
            _ => break (assistant_msg.content, usage),
        }
    };

    if !incognito {
        state
            .context_manager
            .add_assistant_message(session_id, &response_text)
            .await
            .map_err(error::context_error)?;
        // Issue #386: learning is hook-driven — enqueue the completed turn
        // for the debounced `remember.chat` extraction instead of relying on
        // the conversational LLM to call the `remember` tool.
        state
            .hook_engine
            .trigger(Trigger::TurnCompleted {
                session_id,
                payload: Arc::new(vec![ConversationTurn::new(
                    session_id,
                    req.message.clone(),
                    response_text.clone(),
                )]),
            })
            .await;
    }

    Ok(Json(ChatResponse {
        session_id,
        response: response_text,
        usage: Usage {
            prompt_tokens: final_usage.prompt_tokens,
            completion_tokens: final_usage.completion_tokens,
            total_tokens: final_usage.total_tokens,
        },
        tool_calls: tool_call_info,
    }))
}

/// SSE streaming chat completion endpoint with agentic tool loop.
///
/// Streams LLM text as default SSE events. When tool calls are detected,
/// emits `event: tool_call` SSE events with `ToolCallInfo` JSON, executes
/// the tools, and re-opens the LLM stream in the same SSE connection.
/// Repeats until the LLM responds without tool calls or `max_tool_rounds`
/// is reached.
pub async fn chat_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<
    Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>,
    axum::response::Response,
> {
    let (session_id, llm, messages, incognito, permit) = resolve_chat_state(&state, &req).await?;
    state.record_user_activity();

    let max_rounds = state.config.snapshot().await.agent.max_tool_rounds;
    let tools_opt = state
        .tool_registry
        .export_openai_tools_for_llm_with_writes(!incognito);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Event>(16);

    let state_clone = Arc::clone(&state);
    let session_id_clone = session_id;
    let user_message_clone = req.message.clone();
    let llm_clone = Arc::clone(&llm);
    let tool_registry_clone = Arc::clone(&state.tool_registry);

    tokio::spawn(async move {
        let _permit = permit;

        let mut conversation = messages;
        let mut round: u16 = 0;

        let mut usage_acc: Option<mimir_core::llm::types::Usage> = None;

        // Emit session_id so the client can capture it for subsequent turns.
        {
            let event = Event::default()
                .event("session_id")
                .json_data(serde_json::json!({"session_id": session_id_clone}))
                .expect("serializing session_id should not fail");
            if event_tx.send(event).await.is_err() {
                return;
            }
        }

        'outer: loop {
            let mut stream = match llm_clone
                .chat_stream_with_usage(
                    conversation.clone(),
                    if round < max_rounds {
                        tools_opt.clone()
                    } else {
                        None
                    },
                )
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    error!("LLM stream error: {e}");
                    let _ = event_tx
                        .send(
                            Event::default()
                                .event("error")
                                .data("internal server error"),
                        )
                        .await;
                    break 'outer;
                }
            };

            let mut full_response = String::new();
            let mut tool_calls_acc: std::collections::HashMap<
                u32,
                mimir_core::llm::types::ToolCall,
            > = std::collections::HashMap::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(StreamItem::Text(text)) => {
                        full_response.push_str(&text);
                        let event = Event::default().data(text);
                        if event_tx.send(event).await.is_err() {
                            break 'outer;
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
                        usage_acc = Some(match usage_acc {
                            Some(prev) => mimir_core::llm::types::Usage {
                                prompt_tokens: prev.prompt_tokens + usage.prompt_tokens,
                                completion_tokens: prev.completion_tokens + usage.completion_tokens,
                                total_tokens: prev.total_tokens + usage.total_tokens,
                            },
                            None => usage,
                        });
                    }
                    Err(e) => {
                        error!("LLM stream error: {e}");
                        let _ = event_tx
                            .send(
                                Event::default()
                                    .event("error")
                                    .data("internal server error"),
                            )
                            .await;
                        break 'outer;
                    }
                }
            }

            if tool_calls_acc.is_empty() || round >= max_rounds {
                // No tool calls (or max rounds reached) — emit usage and persist.
                if let Some(usage) = usage_acc {
                    let json = serde_json::to_string(&mimir_api_types::Usage {
                        prompt_tokens: usage.prompt_tokens,
                        completion_tokens: usage.completion_tokens,
                        total_tokens: usage.total_tokens,
                    })
                    .unwrap_or_default();
                    let _ = event_tx
                        .send(Event::default().event("usage").data(json))
                        .await;
                }

                if !incognito && !full_response.is_empty() {
                    if let Err(e) = state_clone
                        .context_manager
                        .add_assistant_message(session_id_clone, &full_response)
                        .await
                    {
                        error!("failed to persist assistant message: {e}");
                    }
                    // Issue #386: enqueue the completed turn for the
                    // debounced `remember.chat` extraction.
                    state_clone
                        .hook_engine
                        .trigger(Trigger::TurnCompleted {
                            session_id: session_id_clone,
                            payload: Arc::new(vec![ConversationTurn::new(
                                session_id_clone,
                                user_message_clone.clone(),
                                full_response.clone(),
                            )]),
                        })
                        .await;
                }
                break 'outer;
            }

            // Tool calls accumulated — execute them, emit tool_call events,
            // build follow-up, and loop.
            round += 1;

            let assistant_tool_msg = mimir_core::llm::types::Message {
                role: "assistant".to_string(),
                content: full_response.clone(),
                tool_calls: Some(tool_calls_acc.values().cloned().collect()),
                tool_call_id: None,
            };
            conversation.push(assistant_tool_msg);

            for tool_call in tool_calls_acc.values() {
                let display_name = tool_registry_clone
                    .get_display_name(&tool_call.function.name)
                    .unwrap_or_else(|| {
                        mimir_core::tools::snake_to_title_case(&tool_call.function.name)
                    });

                // Emit tool_call_start so the client sees Mimir is working.
                let start_info = serde_json::json!({
                    "name": tool_call.function.name,
                    "display_name": display_name,
                });
                let start_json = serde_json::to_string(&start_info).unwrap_or_default();
                if event_tx
                    .send(Event::default().event("tool_call_start").data(start_json))
                    .await
                    .is_err()
                {
                    break 'outer;
                }

                let (llm_text, display_text) = match execute_tool_call(
                    &tool_registry_clone,
                    &tool_call.function.name,
                    &tool_call.function.arguments,
                    Arc::clone(&llm_clone),
                    incognito,
                )
                .await
                {
                    Ok(output) => (output.to_llm_text(), output.to_display_text()),
                    Err(e) => {
                        error!("tool '{}' execution failed: {e}", tool_call.function.name);
                        (format!("Tool error: {e}"), format!("Tool error: {e}"))
                    }
                };

                // Emit tool_call SSE event for client visibility.
                let info = mimir_api_types::ToolCallInfo {
                    name: tool_call.function.name.clone(),
                    display_name,
                    result: mimir_api_types::ToolCallInfo::truncate_result(&display_text),
                };
                let json = serde_json::to_string(&info).unwrap_or_default();
                if event_tx
                    .send(Event::default().event("tool_call").data(json))
                    .await
                    .is_err()
                {
                    break 'outer;
                }

                conversation.push(mimir_core::llm::types::Message::tool(
                    &tool_call.id,
                    llm_text,
                ));
            }

            // Loop back — the next iteration will open a fresh LLM stream.
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
