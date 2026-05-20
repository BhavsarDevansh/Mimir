# Phase 1: Core Agent

## Goal
Build the foundational interaction layer: CLI, chat interface, LLM orchestration, and basic tool-calling.

## Duration
4–6 weeks

## Deliverables

### 1.1 CLI Framework
- [ ] `agent start` — daemon launcher
- [ ] `agent ask "..."` — one-shot query
- [ ] `agent chat` — interactive REPL
- [ ] `agent status` — health overview
- [ ] `agent config` — configuration management
- [ ] Command-line argument parsing (clap)
- [ ] Structured logging (tracing)

### 1.2 Chat Interface
- [ ] Local HTTP server (Axum)
- [ ] WebSocket or SSE for streaming responses
- [ ] Simple HTML chat UI (or TUI with ratatui)
- [ ] Conversation history display
- [ ] Markdown rendering for responses

### 1.3 LLM Client
- [ ] OpenAI-compatible HTTP client
- [ ] Streaming support (SSE parsing)
- [ ] Retry with exponential backoff
- [ ] Token usage tracking
- [ ] Configurable endpoint, model, temperature
- [ ] Support for system prompts

### 1.4 Context Manager
- [ ] In-memory conversation history (sliding window)
- [ ] Session management
- [ ] Context injection for multi-turn coherence

### 1.5 Tool Registry
- [ ] Dynamic tool registration
- [ ] JSON Schema generation for LLM function calling
- [ ] Basic built-in tools:
  - `search_knowledge_graph`
  - `get_current_time`
  - `echo` (for testing)

### 1.6 Configuration
- [ ] TOML config file (`~/.config/agent/config.toml`)
- [ ] Environment variable overrides
- [ ] Sensitive value encryption (API keys)
- [ ] Hot-reload for non-sensitive config

### 1.7 Testing
- [ ] Unit tests for CLI parsing
- [ ] Mock LLM client for testing
- [ ] Integration test: full ask → response flow
- [ ] End-to-end test: chat UI interaction

## Success Criteria
- User can start daemon, ask a question, and get a coherent response
- Streaming works without glitches
- Configurable LLM endpoint and model
- Basic tool-calling functional

## Dependencies
- None (this is the foundation)

## Risks
- LLM API latency may make local testing slow
- Streaming SSE parsing edge cases
- Cross-platform config path differences
