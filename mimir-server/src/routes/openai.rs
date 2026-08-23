//! OpenAI-compatible provider surface (issue #388): `GET /v1/models` and
//! `POST /v1/chat/completions` (blocking + streaming), mapped onto Mimir's
//! session, personality, and worker-pool infrastructure.
//!
//! Session mapping: Mimir is single-tenant, so the OpenAI `user` field is a
//! conversation key — a fixed `user` resumes one persistent session in the
//! central profile (backed by the `sessions.user_key` column). The client's
//! `messages` array is a stateless echo: only the last user message starts a
//! new turn, and trailing `tool` messages continue an in-flight turn.
//! Requests without `user` are incognito-style: memory context is still
//! injected, but nothing is persisted and no learning hooks fire.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::StreamExt;
use mimir_api_types::{
    OpenAiChatMessage, OpenAiChatRequest, OpenAiChatResponse, OpenAiChoice, OpenAiDelta,
    OpenAiFunctionCall, OpenAiModel, OpenAiModelList, OpenAiResponseMessage, OpenAiStreamChoice,
    OpenAiStreamChunk, OpenAiToolCall, OpenAiToolCallDelta, OpenAiUsage,
};
use mimir_core::conversation::ConversationTurn;
use mimir_core::hooks::Trigger;
use mimir_core::llm::types::{FunctionCall, Message, StreamItem, ToolCall, Usage};
use tracing::error;

use crate::error;
use crate::routes::chat::{
    INCOGNITO_COUNTER, build_memory_context, build_system_prompt, execute_tool_call,
};
use crate::state::AppState;

/// `GET /v1/models` — list personality presets as OpenAI models.
///
/// `description` is a Mimir extension carrying the preset description;
/// `created` is always `0` because presets have no upstream creation time.
pub async fn models_handler(State(state): State<Arc<AppState>>) -> Json<OpenAiModelList> {
    let cfg = state.config.snapshot().await;
    let personality = state.personality_cache.resolve(&cfg.personality);
    let data = personality
        .list_presets()
        .into_iter()
        .map(|preset| OpenAiModel {
            id: preset.name,
            object: "model".to_string(),
            created: 0,
            owned_by: "mimir".to_string(),
            description: preset.description,
        })
        .collect();
    Json(OpenAiModelList {
        object: "list".to_string(),
        data,
    })
}

/// `POST /v1/chat/completions` — blocking or streaming, selected by the
/// request's `stream` field.
pub async fn chat_completions_handler(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let req: OpenAiChatRequest =
        serde_json::from_slice(&body).map_err(|_| error::openai_json_rejection())?;

    if req.stream {
        chat_completions_stream(state, req).await
    } else {
        let turn = resolve_openai_turn(&state, &req).await?;
        state.record_user_activity();
        let response = run_blocking_turn(&turn).await?;
        Ok(Json(response).into_response())
    }
}

/// The per-request state shared by the blocking and streaming turn loops.
struct OpenAiTurn {
    state: Arc<AppState>,
    session_id: i64,
    incognito: bool,
    llm: Arc<dyn mimir_core::llm::LlmBackend>,
    conversation: Vec<Message>,
    max_rounds: u16,
    merged_tools: Option<Vec<serde_json::Value>>,
    client_tool_names: HashSet<String>,
    user_message: String,
    model: String,
    completion_id: String,
    created: u64,
    /// Held until the assistant response is persisted (non-incognito only).
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// The client-message analysis: the turn's user message, the trailing tool
/// results that continue an in-flight turn, and the index of the last user
/// message (the start of the trailing segment).
struct TurnInput {
    user_message: String,
    trailing_tools: Vec<(String, String)>,
    last_user_index: usize,
}

/// A request-validation failure for the OpenAI surface.
///
/// Kept small (instead of returning an axum `Response`) so `extract_turn`
/// avoids `clippy::result_large_err`; the caller maps it onto the OpenAI
/// error shape.
enum TurnError {
    MissingUserMessage,
    EmptyUserMessage,
    MissingToolCallId,
}

impl TurnError {
    fn into_response(self) -> Response {
        match self {
            TurnError::MissingUserMessage => error::openai_error(
                StatusCode::BAD_REQUEST,
                "messages must contain at least one user message",
                "invalid_request_error",
                Some("messages"),
                None,
            ),
            TurnError::MissingToolCallId => error::openai_error(
                StatusCode::BAD_REQUEST,
                "tool message is missing tool_call_id",
                "invalid_request_error",
                Some("messages"),
                None,
            ),
            TurnError::EmptyUserMessage => error::openai_error(
                StatusCode::BAD_REQUEST,
                "user message must not be empty",
                "invalid_request_error",
                Some("messages"),
                None,
            ),
        }
    }
}

/// Split the client's stateless `messages` echo into the turn's user message
/// and the trailing tool results that continue an in-flight turn.
///
/// Mimir's stored history is authoritative, so only the last user message
/// starts a new turn; `tool` messages after it are the client's tool results
/// and continue the current turn. Trailing assistant messages are ignored
/// because the server already persisted them.
fn extract_turn(messages: &[OpenAiChatMessage]) -> Result<TurnInput, TurnError> {
    let Some(last_user_index) = messages.iter().rposition(|m| m.role == "user") else {
        return Err(TurnError::MissingUserMessage);
    };
    let user_message = messages[last_user_index]
        .content
        .clone()
        .unwrap_or_default();
    if user_message.trim().is_empty() {
        return Err(TurnError::EmptyUserMessage);
    }
    let mut trailing_tools = Vec::new();
    for msg in &messages[last_user_index + 1..] {
        if msg.role == "tool" {
            let Some(tool_call_id) = msg.tool_call_id.clone() else {
                return Err(TurnError::MissingToolCallId);
            };
            trailing_tools.push((tool_call_id, msg.content.clone().unwrap_or_default()));
        }
    }
    Ok(TurnInput {
        user_message,
        trailing_tools,
        last_user_index,
    })
}

/// Convert a client message into the internal LLM message shape.
fn convert_message(msg: &OpenAiChatMessage) -> Message {
    Message {
        role: msg.role.clone(),
        content: msg.content.clone().unwrap_or_default(),
        tool_calls: msg.tool_calls.as_ref().map(|calls| {
            calls
                .iter()
                .map(|call| ToolCall {
                    index: 0,
                    id: call.id.clone(),
                    call_type: call.call_type.clone(),
                    function: FunctionCall {
                        name: call.function.name.clone(),
                        arguments: call.function.arguments.clone(),
                    },
                })
                .collect()
        }),
        tool_call_id: msg.tool_call_id.clone(),
    }
}

/// Merge server-side tools with client-supplied tools.
///
/// Server tools are always available; on a name collision the server-side
/// tool wins and the client's definition is silently dropped (issue #388).
/// Returns the merged list and the set of client-only tool names.
fn merge_tools(
    server: Option<Vec<serde_json::Value>>,
    client: Option<Vec<serde_json::Value>>,
) -> (Option<Vec<serde_json::Value>>, HashSet<String>) {
    let server = server.unwrap_or_default();
    let client = client.unwrap_or_default();
    let server_names: HashSet<String> = server
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
        .collect();
    let mut merged = server;
    let mut client_names = HashSet::new();
    for tool in client {
        let Some(name) = tool["function"]["name"].as_str() else {
            continue;
        };
        // Server tools win collisions; duplicate client definitions are
        // also dropped (first wins) so the LLM never sees one name twice.
        if server_names.contains(name) || client_names.contains(name) {
            continue;
        }
        client_names.insert(name.to_string());
        merged.push(tool);
    }
    let merged = if merged.is_empty() {
        None
    } else {
        Some(merged)
    };
    (merged, client_names)
}

/// Split tool calls into server-side (executed here) and client-side
/// (returned to the client) sets.
fn split_tool_calls(
    tool_calls: &[ToolCall],
    client_tool_names: &HashSet<String>,
) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let mut server = Vec::new();
    let mut client = Vec::new();
    for call in tool_calls {
        if client_tool_names.contains(&call.function.name) {
            client.push(call.clone());
        } else {
            server.push(call.clone());
        }
    }
    (server, client)
}

/// Resolve the session, personality, LLM backend, and conversation for one
/// OpenAI chat completion request.
async fn resolve_openai_turn(
    state: &Arc<AppState>,
    req: &OpenAiChatRequest,
) -> Result<OpenAiTurn, Response> {
    if req.model.trim().is_empty() {
        return Err(error::openai_error(
            StatusCode::BAD_REQUEST,
            "model is required",
            "invalid_request_error",
            Some("model"),
            None,
        ));
    }
    let input = extract_turn(&req.messages).map_err(TurnError::into_response)?;

    let memory = build_memory_context(state).await;
    let cfg = state.config.snapshot().await;

    // Model mapping: preset names select a personality; unknown names pass
    // through as upstream model overrides with the configured personality.
    let candidate = state
        .personality_cache
        .resolve(&mimir_core::config::PersonalityConfig {
            preset: req.model.clone(),
        });
    let is_preset = candidate.has_preset(&req.model);
    let personality = if is_preset {
        candidate
    } else {
        state.personality_cache.resolve(&cfg.personality)
    };
    let llm = state.resolve_llm(if is_preset {
        None
    } else {
        Some(req.model.clone())
    });

    // Sampling: per-request temperature wins over config; max_tokens applies
    // only when the client sends it — no default cap (issue #388).
    let temperature = req.temperature.unwrap_or(cfg.llm.temperature);
    let llm = llm.with_temperature_override(temperature).unwrap_or(llm);
    let llm = match req.max_tokens.or(req.max_completion_tokens) {
        Some(max_tokens) => llm.with_max_tokens_override(max_tokens).unwrap_or(llm),
        None => llm,
    };

    // A blank `user` is treated as absent so an empty conversation key
    // cannot silently create a session keyed on "".
    let user_key = req.user.as_deref().filter(|key| !key.trim().is_empty());
    let incognito = user_key.is_none();
    let session_id = if let Some(user_key) = user_key {
        let system_prompt = build_system_prompt(state, &personality, &memory).await;
        state
            .context_manager
            .resolve_openai_session(user_key, system_prompt)
            .await
            .map_err(error::openai_context_error)?
    } else {
        INCOGNITO_COUNTER.fetch_sub(1, Ordering::SeqCst)
    };

    let permit = if incognito {
        None
    } else {
        Some(
            state
                .session_semaphore(session_id)
                .acquire_owned()
                .await
                .map_err(|_| {
                    error!("session semaphore closed");
                    error::openai_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal server error",
                        "server_error",
                        None,
                        None,
                    )
                })?,
        )
    };

    let conversation = if incognito {
        // No stored history: the client's trailing segment (from the last
        // user message onward) is the conversation, prefixed with the
        // system prompt.
        let system_prompt = build_system_prompt(state, &personality, &memory).await;
        let mut conversation = vec![Message::system(&system_prompt)];
        for msg in &req.messages[input.last_user_index..] {
            conversation.push(convert_message(msg));
        }
        conversation
    } else {
        // Stored history is authoritative: append the new user message (or
        // the continuation's tool results), trim, and export.
        if input.trailing_tools.is_empty() {
            state
                .context_manager
                .add_user_message(session_id, &input.user_message)
                .await
                .map_err(error::openai_context_error)?;
        }
        for (tool_call_id, content) in &input.trailing_tools {
            state
                .context_manager
                .add_tool_message(session_id, tool_call_id, content)
                .await
                .map_err(error::openai_context_error)?;
        }
        state
            .context_manager
            .trim_to_budget(session_id, cfg.context.max_tokens, cfg.context.max_turns)
            .await
            .map_err(error::openai_context_error)?;
        state
            .context_manager
            .export_messages(session_id)
            .await
            .map_err(error::openai_context_error)?
    };

    let (merged_tools, client_tool_names) = merge_tools(
        state
            .tool_registry
            .export_openai_tools_for_llm_with_writes(!incognito),
        req.tools.clone(),
    );

    Ok(OpenAiTurn {
        state: Arc::clone(state),
        session_id,
        incognito,
        llm,
        conversation,
        max_rounds: cfg.agent.max_tool_rounds,
        merged_tools,
        client_tool_names,
        user_message: input.user_message,
        model: req.model.clone(),
        completion_id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        created: chrono::Utc::now().timestamp() as u64,
        permit,
    })
}

/// Execute server-side tool calls, persisting results for non-incognito
/// sessions and appending them to the in-memory conversation.
async fn execute_server_tools(
    turn: &OpenAiTurn,
    conversation: &mut Vec<Message>,
    server_calls: &[ToolCall],
) {
    for tool_call in server_calls {
        let llm_text = match execute_tool_call(
            &turn.state.tool_registry,
            &tool_call.function.name,
            &tool_call.function.arguments,
            Arc::clone(&turn.llm),
            turn.incognito,
        )
        .await
        {
            Ok(output) => output.to_llm_text(),
            Err(e) => {
                error!("tool '{}' execution failed: {e}", tool_call.function.name);
                format!("Tool error: {e}")
            }
        };
        conversation.push(Message::tool(&tool_call.id, &llm_text));
        if !turn.incognito {
            if let Err(e) = turn
                .state
                .context_manager
                .add_tool_message(turn.session_id, &tool_call.id, &llm_text)
                .await
            {
                error!("failed to persist tool message: {e}");
            }
        }
    }
}

/// Persist the final assistant response and enqueue the completed-turn
/// learning hook (non-incognito only).
async fn finish_turn(turn: &OpenAiTurn, response: &str) {
    if turn.incognito || response.is_empty() {
        return;
    }
    if let Err(e) = turn
        .state
        .context_manager
        .add_assistant_message(turn.session_id, response)
        .await
    {
        error!("failed to persist assistant message: {e}");
    }
    // Issue #386: learning is hook-driven — enqueue the completed turn for
    // the debounced `remember.chat` extraction.
    turn.state
        .hook_engine
        .trigger(Trigger::TurnCompleted {
            session_id: turn.session_id,
            payload: Arc::new(vec![ConversationTurn::new(
                turn.session_id,
                turn.user_message.clone(),
                response.to_string(),
            )]),
        })
        .await;
}

fn final_response(turn: &OpenAiTurn, content: String, usage: Usage) -> OpenAiChatResponse {
    OpenAiChatResponse {
        id: turn.completion_id.clone(),
        object: "chat.completion".to_string(),
        created: turn.created,
        model: turn.model.clone(),
        choices: vec![OpenAiChoice {
            index: 0,
            message: OpenAiResponseMessage {
                role: "assistant".to_string(),
                content: Some(content),
                tool_calls: Vec::new(),
            },
            finish_reason: "stop".to_string(),
        }],
        usage: OpenAiUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        },
    }
}

fn tool_calls_response(
    turn: &OpenAiTurn,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
) -> OpenAiChatResponse {
    OpenAiChatResponse {
        id: turn.completion_id.clone(),
        object: "chat.completion".to_string(),
        created: turn.created,
        model: turn.model.clone(),
        choices: vec![OpenAiChoice {
            index: 0,
            message: OpenAiResponseMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: tool_calls
                    .into_iter()
                    .map(|call| OpenAiToolCall {
                        id: call.id,
                        call_type: call.call_type,
                        function: OpenAiFunctionCall {
                            name: call.function.name,
                            arguments: call.function.arguments,
                        },
                    })
                    .collect(),
            },
            finish_reason: "tool_calls".to_string(),
        }],
        usage: OpenAiUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        },
    }
}

/// Blocking completion with the agentic tool loop.
async fn run_blocking_turn(turn: &OpenAiTurn) -> Result<OpenAiChatResponse, Response> {
    let mut conversation = turn.conversation.clone();
    let mut round: u16 = 0;

    loop {
        let (assistant_msg, usage) = turn
            .llm
            .chat_message(
                conversation.clone(),
                if round < turn.max_rounds {
                    turn.merged_tools.clone()
                } else {
                    None
                },
            )
            .await
            .map_err(error::openai_llm_error)?;

        match assistant_msg.tool_calls {
            Some(ref tool_calls) if round < turn.max_rounds => {
                round += 1;
                let (server_calls, client_calls) =
                    split_tool_calls(tool_calls, &turn.client_tool_names);
                conversation.push(assistant_msg.clone());

                if !turn.incognito {
                    turn.state
                        .context_manager
                        .add_assistant_tool_calls_message(
                            turn.session_id,
                            &assistant_msg.content,
                            tool_calls,
                        )
                        .await
                        .map_err(error::openai_context_error)?;
                }

                execute_server_tools(turn, &mut conversation, &server_calls).await;

                if !client_calls.is_empty() {
                    return Ok(tool_calls_response(turn, client_calls, usage));
                }
            }
            _ => {
                finish_turn(turn, &assistant_msg.content).await;
                return Ok(final_response(turn, assistant_msg.content, usage));
            }
        }
    }
}

/// Streaming completion with the agentic tool loop, framed as OpenAI SSE
/// chunks (`chat.completion.chunk`) terminated by `data: [DONE]`.
async fn chat_completions_stream(
    state: Arc<AppState>,
    req: OpenAiChatRequest,
) -> Result<Response, Response> {
    let turn = resolve_openai_turn(&state, &req).await?;
    state.record_user_activity();

    // Defensive pre-flight: the worker-pool bypass (#465) makes this a no-op
    // on the hot path today, but once the bypass is fixed a full queue must
    // surface as 503 before the SSE response starts.
    if !turn.llm.user_queue_has_capacity().await {
        return Err(error::openai_llm_error(
            mimir_core::llm::types::LlmError::QueueFull,
        ));
    }

    let include_usage = req
        .stream_options
        .as_ref()
        .is_some_and(|options| options.include_usage);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Event>(16);

    tokio::spawn(async move {
        let mut turn = turn;
        // Hold the per-session permit until the assistant response is
        // persisted (non-incognito only).
        let _permit = turn.permit.take();

        let mut conversation = turn.conversation.clone();
        let mut round: u16 = 0;
        let mut usage_acc: Option<Usage> = None;
        let mut sent_role = false;

        'outer: loop {
            let mut stream = match turn
                .llm
                .chat_stream_with_usage(
                    conversation.clone(),
                    if round < turn.max_rounds {
                        turn.merged_tools.clone()
                    } else {
                        None
                    },
                )
                .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    error!("LLM stream error: {e}");
                    break 'outer;
                }
            };

            let mut full_response = String::new();
            let mut tool_calls_acc: HashMap<u32, ToolCall> = HashMap::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(StreamItem::Text(text)) => {
                        full_response.push_str(&text);
                        if !sent_role {
                            sent_role = true;
                            if send_chunk(
                                &event_tx,
                                &turn,
                                OpenAiDelta {
                                    role: Some("assistant".to_string()),
                                    content: Some(String::new()),
                                    tool_calls: Vec::new(),
                                },
                                None,
                            )
                            .await
                            .is_err()
                            {
                                break 'outer;
                            }
                        }
                        if send_chunk(
                            &event_tx,
                            &turn,
                            OpenAiDelta {
                                role: None,
                                content: Some(text),
                                tool_calls: Vec::new(),
                            },
                            None,
                        )
                        .await
                        .is_err()
                        {
                            break 'outer;
                        }
                    }
                    Ok(StreamItem::ToolCalls(deltas)) => {
                        if !sent_role {
                            sent_role = true;
                            if send_chunk(
                                &event_tx,
                                &turn,
                                OpenAiDelta {
                                    role: Some("assistant".to_string()),
                                    content: Some(String::new()),
                                    tool_calls: Vec::new(),
                                },
                                None,
                            )
                            .await
                            .is_err()
                            {
                                break 'outer;
                            }
                        }
                        let converted: Vec<OpenAiToolCallDelta> = deltas
                            .iter()
                            .map(|delta| OpenAiToolCallDelta {
                                index: delta.index,
                                id: if delta.id.is_empty() {
                                    None
                                } else {
                                    Some(delta.id.clone())
                                },
                                call_type: if delta.call_type.is_empty() {
                                    None
                                } else {
                                    Some(delta.call_type.clone())
                                },
                                function: Some(OpenAiFunctionCall {
                                    name: delta.function.name.clone(),
                                    arguments: delta.function.arguments.clone(),
                                }),
                            })
                            .collect();
                        for delta in &deltas {
                            let entry = tool_calls_acc.entry(delta.index).or_default();
                            if !delta.id.is_empty() {
                                entry.id = delta.id.clone();
                            }
                            if !delta.call_type.is_empty() {
                                entry.call_type = delta.call_type.clone();
                            }
                            if !delta.function.name.is_empty() {
                                entry.function.name = delta.function.name.clone();
                            }
                            entry.function.arguments.push_str(&delta.function.arguments);
                        }
                        if send_chunk(
                            &event_tx,
                            &turn,
                            OpenAiDelta {
                                role: None,
                                content: None,
                                tool_calls: converted,
                            },
                            None,
                        )
                        .await
                        .is_err()
                        {
                            break 'outer;
                        }
                    }
                    Ok(StreamItem::Usage(usage)) => {
                        usage_acc = Some(match usage_acc {
                            Some(prev) => Usage {
                                prompt_tokens: prev.prompt_tokens + usage.prompt_tokens,
                                completion_tokens: prev.completion_tokens + usage.completion_tokens,
                                total_tokens: prev.total_tokens + usage.total_tokens,
                            },
                            None => usage,
                        });
                    }
                    Err(e) => {
                        error!("LLM stream error: {e}");
                        break 'outer;
                    }
                }
            }

            if tool_calls_acc.is_empty() || round >= turn.max_rounds {
                // Final answer: persist, fire the learning hook, and close
                // the stream with the finish chunk.
                finish_turn(&turn, &full_response).await;
                let _ = send_chunk(&event_tx, &turn, OpenAiDelta::default(), Some("stop")).await;
                if include_usage {
                    if let Some(usage) = usage_acc {
                        let _ = send_usage_chunk(&event_tx, &turn, usage).await;
                    }
                }
                let _ = event_tx.send(Event::default().data("[DONE]")).await;
                break 'outer;
            }

            round += 1;
            let mut tool_calls: Vec<ToolCall> = tool_calls_acc.into_values().collect();
            tool_calls.sort_by_key(|call| call.index);
            let (server_calls, client_calls) =
                split_tool_calls(&tool_calls, &turn.client_tool_names);

            let assistant_tool_msg = Message {
                role: "assistant".to_string(),
                content: full_response.clone(),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
            };
            conversation.push(assistant_tool_msg);

            if !turn.incognito {
                if let Err(e) = turn
                    .state
                    .context_manager
                    .add_assistant_tool_calls_message(turn.session_id, &full_response, &tool_calls)
                    .await
                {
                    error!("failed to persist assistant tool-call message: {e}");
                }
            }

            execute_server_tools(&turn, &mut conversation, &server_calls).await;

            if !client_calls.is_empty() {
                // Client tool calls: hand them back and end the stream; the
                // client's follow-up `tool` messages continue the turn.
                let _ =
                    send_chunk(&event_tx, &turn, OpenAiDelta::default(), Some("tool_calls")).await;
                if include_usage {
                    if let Some(usage) = usage_acc {
                        let _ = send_usage_chunk(&event_tx, &turn, usage).await;
                    }
                }
                let _ = event_tx.send(Event::default().data("[DONE]")).await;
                break 'outer;
            }
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(event_rx)
        .map(Ok::<_, std::convert::Infallible>);

    Ok(Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(10))
                .text("keep-alive"),
        )
        .into_response())
}

/// Send one OpenAI stream chunk as an SSE `data:` event.
async fn send_chunk(
    event_tx: &tokio::sync::mpsc::Sender<Event>,
    turn: &OpenAiTurn,
    delta: OpenAiDelta,
    finish_reason: Option<&str>,
) -> Result<(), ()> {
    let chunk = OpenAiStreamChunk {
        id: turn.completion_id.clone(),
        object: "chat.completion.chunk".to_string(),
        created: turn.created,
        model: turn.model.clone(),
        choices: vec![OpenAiStreamChoice {
            index: 0,
            delta,
            finish_reason: finish_reason.map(String::from),
        }],
        usage: None,
    };
    let json = serde_json::to_string(&chunk).unwrap_or_default();
    event_tx
        .send(Event::default().data(json))
        .await
        .map_err(|_| ())
}

/// Send the final usage chunk (`choices: []` + `usage`), emitted only when
/// the client requested `stream_options.include_usage`.
async fn send_usage_chunk(
    event_tx: &tokio::sync::mpsc::Sender<Event>,
    turn: &OpenAiTurn,
    usage: Usage,
) -> Result<(), ()> {
    let chunk = OpenAiStreamChunk {
        id: turn.completion_id.clone(),
        object: "chat.completion.chunk".to_string(),
        created: turn.created,
        model: turn.model.clone(),
        choices: Vec::new(),
        usage: Some(OpenAiUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }),
    };
    let json = serde_json::to_string(&chunk).unwrap_or_default();
    event_tx
        .send(Event::default().data(json))
        .await
        .map_err(|_| ())
}
