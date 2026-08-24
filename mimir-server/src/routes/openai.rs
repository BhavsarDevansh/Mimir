//! OpenAI-compatible provider surface (issue #388): `GET /v1/models` and
//! `POST /v1/chat/completions` (blocking + streaming), mapped onto Mimir's
//! session, personality, and worker-pool infrastructure.
//!
//! Session mapping: Mimir is single-tenant, so the OpenAI `user` field is a
//! conversation key — a fixed `user` resumes one persistent session in the
//! central profile (backed by the `sessions.user_key` column). The client's
//! `messages` array is a stateless echo: only the last user message starts a
//! new turn, and trailing `tool` messages continue an in-flight turn.
//! Requests without `user` (or with a blank one) key the fixed default
//! session, so every request is persistent and every completed turn fires
//! the learning hooks — there is no incognito path on this surface (issue
//! #473).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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
use mimir_core::llm::types::{Message, StreamItem, ToolCall, Usage};
use tracing::error;

use crate::error;
use crate::routes::chat::{build_memory_context, build_system_prompt, execute_tool_call};
use crate::state::AppState;

/// The conversation key used when a request omits (or blanks) the OpenAI
/// `user` field: every unkeyed request resumes one shared persistent session
/// in the central profile (issue #473).
const DEFAULT_OPENAI_SESSION_KEY: &str = "default";

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
    /// Highest persisted message id before this request's writes; a failed
    /// turn rolls back to this baseline so no orphaned messages remain.
    baseline_message_id: i64,
    llm: Arc<dyn mimir_core::llm::LlmBackend>,
    conversation: Vec<Message>,
    max_rounds: u16,
    merged_tools: Option<Vec<serde_json::Value>>,
    client_tool_names: HashSet<String>,
    user_message: String,
    model: String,
    completion_id: String,
    created: u64,
    /// Held until the assistant response is persisted, so concurrent
    /// requests for the same session never interleave writes.
    permit: tokio::sync::OwnedSemaphorePermit,
}

/// The client-message analysis: the turn's user message and the trailing
/// tool results that continue an in-flight turn.
struct TurnInput {
    user_message: String,
    trailing_tools: Vec<(String, String)>,
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
    InvalidTools,
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
            TurnError::InvalidTools => error::openai_error(
                StatusCode::BAD_REQUEST,
                "each tool must be a function tool with a non-empty name and an object parameters schema",
                "invalid_request_error",
                Some("tools"),
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
    })
}

/// Merge server-side tools with client-supplied tools.
///
/// Server tools are always available; on a name collision the server-side
/// tool wins and the client's definition is silently dropped (issue #388).
/// Returns the merged list and the set of client-only tool names.
fn merge_tools(
    server: Option<Vec<serde_json::Value>>,
    client: Option<Vec<serde_json::Value>>,
) -> Result<(Option<Vec<serde_json::Value>>, HashSet<String>), TurnError> {
    let server = server.unwrap_or_default();
    let client = client.unwrap_or_default();
    let server_names: HashSet<String> = server
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
        .collect();
    let mut merged = server;
    let mut client_names = HashSet::new();
    for tool in client {
        let name = validate_client_tool(&tool)?;
        // Server tools win collisions; duplicate client definitions are
        // also dropped (first wins) so the LLM never sees one name twice.
        if server_names.contains(&name) || client_names.contains(&name) {
            continue;
        }
        client_names.insert(name);
        merged.push(tool);
    }
    let merged = if merged.is_empty() {
        None
    } else {
        Some(merged)
    };
    Ok((merged, client_names))
}

/// Validate the minimum OpenAI tool shape a client must send (PR #466 review).
///
/// A tool must be a `function` tool with a non-blank function name, and the
/// `parameters` schema, when present, must be a JSON object. Malformed
/// definitions are rejected with a `400 invalid_request_error` on the
/// `tools` field so the client learns which part of the request is wrong
/// instead of receiving an opaque upstream failure.
fn validate_client_tool(tool: &serde_json::Value) -> Result<String, TurnError> {
    if tool.get("type").and_then(serde_json::Value::as_str) != Some("function") {
        return Err(TurnError::InvalidTools);
    }
    let Some(function) = tool.get("function").filter(|f| f.is_object()) else {
        return Err(TurnError::InvalidTools);
    };
    let Some(name) = function
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
    else {
        return Err(TurnError::InvalidTools);
    };
    if let Some(parameters) = function.get("parameters")
        && !parameters.is_object()
    {
        return Err(TurnError::InvalidTools);
    }
    Ok(name.to_string())
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
    // The preset probe is diagnostic-free, so an upstream model name does not
    // log an unknown-preset warning on every request (PR #466 review).
    let is_preset = state.personality_cache.has_preset(&req.model);
    let personality = if is_preset {
        state
            .personality_cache
            .resolve(&mimir_core::config::PersonalityConfig {
                preset: req.model.clone(),
            })
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

    // A blank `user` is treated as absent; both map to the fixed default
    // session key so no request is ever silently incognito (issue #473).
    let user_key = req
        .user
        .as_deref()
        .filter(|key| !key.trim().is_empty())
        .unwrap_or(DEFAULT_OPENAI_SESSION_KEY);

    // Validate and merge tool definitions before any session creation or
    // persistence, so a rejected request cannot leave a session or an
    // orphaned user message behind. Write-capable tools are always exported:
    // the OpenAI surface has no incognito path (issue #473).
    let (merged_tools, client_tool_names) = merge_tools(
        state
            .tool_registry
            .export_openai_tools_for_llm_with_writes(true),
        req.tools.clone(),
    )
    .map_err(TurnError::into_response)?;

    let system_prompt = build_system_prompt(state, &personality, &memory).await;
    let session_id = state
        .context_manager
        .resolve_openai_session(user_key, system_prompt)
        .await
        .map_err(error::openai_context_error)?;

    // Rollback baseline: a failed turn must not leave its just-persisted
    // user message (or tool results) behind as an orphaned final turn. The
    // per-session permit below guarantees no concurrent request interleaves
    // writes for this session (PR #466 review).
    let baseline_message_id = state
        .context_manager
        .max_message_id(session_id)
        .await
        .map_err(error::openai_context_error)?;

    let permit = state
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
        })?;

    // Stored history is authoritative: append the new user message (or the
    // continuation's tool results), trim, and export.
    let persist = async {
        if input.trailing_tools.is_empty() {
            state
                .context_manager
                .add_user_message(session_id, &input.user_message)
                .await?;
        }
        for (tool_call_id, content) in &input.trailing_tools {
            state
                .context_manager
                .add_tool_message(session_id, tool_call_id, content)
                .await?;
        }
        state
            .context_manager
            .trim_to_budget(session_id, cfg.context.max_tokens, cfg.context.max_turns)
            .await?;
        state.context_manager.export_messages(session_id).await
    };
    let conversation = match persist.await {
        Ok(conversation) => conversation,
        Err(e) => {
            // A persistence failure must not leave the request's writes
            // behind as an orphaned final turn (PR #466 review).
            if let Err(rollback) = state
                .context_manager
                .delete_messages_after(session_id, baseline_message_id)
                .await
            {
                error!("failed to roll back persisted messages after context error: {rollback}");
            }
            return Err(error::openai_context_error(e));
        }
    };

    Ok(OpenAiTurn {
        state: Arc::clone(state),
        session_id,
        baseline_message_id,
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

/// Execute server-side tool calls, persisting results into the session and
/// appending them to the in-memory conversation.
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
            // Every `/v1` turn is persistent; write tools are always allowed
            // (issue #473).
            false,
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

/// Delete every message this request persisted, restoring the session to its
/// pre-request state when a turn fails before completion (PR #466 review).
async fn rollback_persisted_turn(turn: &OpenAiTurn) {
    if let Err(e) = turn
        .state
        .context_manager
        .delete_messages_after(turn.session_id, turn.baseline_message_id)
        .await
    {
        error!("failed to roll back persisted messages after failed turn: {e}");
    }
}

/// Accumulate a per-call usage report into the turn's running total.
///
/// Both the blocking and streaming paths span multiple LLM calls when tools
/// run, so usage must be summed across rounds rather than overwritten
/// (PR #466 review).
fn accumulate_usage(acc: &mut Option<Usage>, usage: Usage) {
    *acc = Some(match acc.take() {
        Some(prev) => Usage {
            prompt_tokens: prev.prompt_tokens + usage.prompt_tokens,
            completion_tokens: prev.completion_tokens + usage.completion_tokens,
            total_tokens: prev.total_tokens + usage.total_tokens,
        },
        None => usage,
    });
}

/// Persist the final assistant response and enqueue the completed-turn
/// learning hook.
async fn finish_turn(turn: &OpenAiTurn, response: &str) {
    if response.is_empty() {
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
    let mut usage_acc: Option<Usage> = None;

    loop {
        let (assistant_msg, usage) = match turn
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
        {
            Ok(result) => result,
            Err(e) => {
                // The turn failed before completing; remove the messages this
                // request persisted so the session keeps no orphaned final
                // turn (PR #466 review).
                rollback_persisted_turn(turn).await;
                return Err(error::openai_llm_error(e));
            }
        };
        accumulate_usage(&mut usage_acc, usage);

        match assistant_msg.tool_calls {
            Some(ref tool_calls) if round < turn.max_rounds => {
                round += 1;
                let (server_calls, client_calls) =
                    split_tool_calls(tool_calls, &turn.client_tool_names);
                conversation.push(assistant_msg.clone());

                turn.state
                    .context_manager
                    .add_assistant_tool_calls_message(
                        turn.session_id,
                        &assistant_msg.content,
                        tool_calls,
                    )
                    .await
                    .map_err(error::openai_context_error)?;

                execute_server_tools(turn, &mut conversation, &server_calls).await;

                if !client_calls.is_empty() {
                    return Ok(tool_calls_response(
                        turn,
                        client_calls,
                        usage_acc.unwrap_or_default(),
                    ));
                }
            }
            _ => {
                finish_turn(turn, &assistant_msg.content).await;
                return Ok(final_response(
                    turn,
                    assistant_msg.content,
                    usage_acc.unwrap_or_default(),
                ));
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

    // Admit the first stream job before the SSE response starts so queue
    // saturation surfaces as 503 + Retry-After instead of an SSE error after
    // `200 OK` (PR #477 review). Later tool-loop rounds enqueue inside the
    // stream, where a mid-stream admission failure can only be an SSE error.
    let mut conversation = turn.conversation.clone();
    let mut round: u16 = 0;
    let first_stream = match turn
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
            // The user message was already persisted by `resolve_openai_turn`;
            // remove it so a rejected turn leaves no orphaned final message.
            rollback_persisted_turn(&turn).await;
            return Err(error::openai_llm_error(e));
        }
    };

    let include_usage = req
        .stream_options
        .as_ref()
        .is_some_and(|options| options.include_usage);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<Event>(16);

    tokio::spawn(async move {
        // Hold the per-session permit until the assistant response is
        // persisted.
        let _permit = &turn.permit;

        let mut usage_acc: Option<Usage> = None;
        let mut sent_role = false;
        let mut stream = Some(first_stream);

        'outer: loop {
            let mut stream = match stream.take() {
                Some(stream) => stream,
                None => match turn
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
                        rollback_persisted_turn(&turn).await;
                        send_error_and_done(&event_tx).await;
                        break 'outer;
                    }
                },
            };

            let mut full_response = String::new();
            let mut tool_calls_acc: HashMap<u32, ToolCall> = HashMap::new();
            // Tool-call deltas are buffered per round and emitted only when
            // the round hands client tools back, so internal Mimir tool
            // calls never reach the client stream (PR #466 review).
            let mut buffered_tool_deltas: Vec<OpenAiToolCallDelta> = Vec::new();

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
                        for delta in &deltas {
                            let entry = tool_calls_acc.entry(delta.index).or_default();
                            // The accumulated call must keep its stream index
                            // so multi-call rounds sort and split correctly
                            // (the `ToolCall::default` index is 0).
                            entry.index = delta.index;
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
                            buffered_tool_deltas.push(OpenAiToolCallDelta {
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
                            });
                        }
                    }
                    Ok(StreamItem::Usage(usage)) => {
                        accumulate_usage(&mut usage_acc, usage);
                    }
                    Err(e) => {
                        error!("LLM stream error: {e}");
                        rollback_persisted_turn(&turn).await;
                        send_error_and_done(&event_tx).await;
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

            if let Err(e) = turn
                .state
                .context_manager
                .add_assistant_tool_calls_message(turn.session_id, &full_response, &tool_calls)
                .await
            {
                error!("failed to persist assistant tool-call message: {e}");
            }

            execute_server_tools(&turn, &mut conversation, &server_calls).await;

            if !client_calls.is_empty() {
                // Client tool calls: stream the buffered deltas for those
                // calls (in index order; server-side deltas were never
                // emitted) and end the stream. The client's follow-up `tool`
                // messages continue the turn.
                if !sent_role
                    && send_chunk(
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
                let client_indices: HashSet<u32> =
                    client_calls.iter().map(|call| call.index).collect();
                buffered_tool_deltas.sort_by_key(|delta| delta.index);
                for delta in buffered_tool_deltas
                    .into_iter()
                    .filter(|delta| client_indices.contains(&delta.index))
                {
                    if send_chunk(
                        &event_tx,
                        &turn,
                        OpenAiDelta {
                            role: None,
                            content: None,
                            tool_calls: vec![delta],
                        },
                        None,
                    )
                    .await
                    .is_err()
                    {
                        break 'outer;
                    }
                }
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

/// Terminate a failed SSE stream: an `error` event followed by `[DONE]`, so
/// clients can distinguish a completed stream from a failed one instead of
/// stalling on a silently closed body (PR #466 review).
async fn send_error_and_done(event_tx: &tokio::sync::mpsc::Sender<Event>) {
    let _ = event_tx
        .send(
            Event::default()
                .event("error")
                .data("internal server error"),
        )
        .await;
    let _ = event_tx.send(Event::default().data("[DONE]")).await;
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
