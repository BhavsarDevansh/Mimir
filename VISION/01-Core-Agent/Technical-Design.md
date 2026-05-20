# Core Agent — Technical Design

## Architecture

The Core Agent is the orchestration layer. It does not contain business logic for reasoning or connectors — it delegates to the Reasoning Engine and Connector Framework. Its job is:

1. Receive user input (CLI, chat, or proactive trigger)
2. Maintain conversation context
3. Decide which subsystem(s) to invoke
4. Stream responses back to the user
5. Log interactions for learning

## Components

### 1. Input Router
Parses incoming messages and classifies intent:
- **Direct question** → Route to Reasoning Engine
- **Command** (`/config`, `/forget`, `/audit`) → Handle internally
- **Casual chat** → Route to LLM with context
- **Proactive trigger** → Initiated by event monitor, routed to LLM with context

### 2. Context Manager
Maintains a sliding window of recent conversation, plus references to persistent facts from the Knowledge Graph relevant to the current topic.
- Conversation history: last N turns (configurable)
- Working memory: facts fetched from Knowledge Graph for this session
- Session ID for correlation

### 3. LLM Client
OpenAI-compatible HTTP client with:
- Streaming support (SSE)
- Retry with exponential backoff
- Token usage tracking
- Support for tool-calling (function calling)

### 4. Tool Registry
Dynamic registry of available tools:
- Knowledge Graph query tools
- Connector read/write tools
- Reasoning Engine invoke tool
- Calendar/event creation tools

Each tool has a JSON Schema description consumed by the LLM.

### 5. Response Synthesizer
Takes raw output from LLM or Reasoning Engine and formats it for the user. In verbose mode, prepends a reasoning summary.

### 6. Learning Logger
Records every interaction (input, tools used, output, user feedback if any) to the Learning Store for pattern extraction.

## Data Flow

```
User Input → Input Router → Context Manager → LLM Client
                                        ↓
                              Tool Calls (if any)
                                        ↓
                              Tool Registry → Subsystems
                                        ↓
                              Response Synthesizer → User
                                        ↓
                              Learning Logger
```

## API Surface

The Core Agent exposes:
- **HTTP API** (localhost only by default) for chat UI
- **Unix socket / named pipe** for CLI
- **gRPC or similar** for internal subsystem communication

## Technology Stack
- **Language:** Rust (performance, safety, native async)
- **HTTP Framework:** Axum or Actix-web
- **LLM Client:** Custom async HTTP client with streaming
- **Config:** TOML files
- **Logging:** Structured JSON logging (tracing)

## Dependencies on Other Subsystems
- **Knowledge Graph:** For fetching and updating facts
- **Connectors:** For reading/writing external service data
- **Reasoning Engine:** For complex multi-step queries
