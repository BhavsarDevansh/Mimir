# Core Agent — Technical Design

## Architecture

The Core Agent is the orchestration layer. It does not contain business logic for reasoning or connectors — it delegates to the Reasoning Engine and Connector Framework. Its job is:

1. Receive user input (CLI, chat, or proactive trigger)
2. Maintain conversation context
3. Decide which subsystem(s) to invoke
4. Stream responses back to the user
5. Log interactions for learning

## Single Binary Architecture

Mimir is distributed as a single `mimir` binary that operates in two modes:

- **Daemon mode** (`mimir start`): runs a persistent process with the HTTP server, LLM worker pool, context manager, and all subsystems
- **Client mode** (`mimir ask`, `mimir chat`, `mimir status`, `mimir memory`, `mimir stop`): thin HTTP client that talks to the daemon

Library crates provide code organisation without separate binaries:

| Crate | Type | Role |
|-------|------|------|
| `mimir-core` | library | LLM client, config, memory, context, personality, tools, skills, paths |
| `mimir-server` | library | Axum routes, state, middleware |
| `mimir-client` | library | HTTP client for talking to the daemon |
| `mimir` | binary | Single entry point — dispatches daemon or client mode |

### Transport

The daemon exposes its API over two transports simultaneously:

1. **TCP** (`127.0.0.1:8080`) — active transport for all clients (local and remote)
2. **Unix domain socket** (`~/.local/share/mimir/mimir.sock`) — implemented (issue #25); the primary local-CLI transport, offering instant daemon detection (a connection attempt on the socket, so a stale file from a crash is detected as down), filesystem permissions (mode 0600), and lower latency

The CLI prefers the Unix socket and falls back to TCP for remote daemons (`MIMIR_BASE_URL`) and Windows (implemented, see #25).

### Daemon-down Handling

When a CLI command cannot connect to the daemon, the user is prompted:
```
Error: Mimir is not running.
Start the server now? [y/N]:
```
If the user agrees, the daemon is started in-process and the command is retried.

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
- **Unix domain socket** for local CLI (preferred transport)
- **TCP fallback** for remote clients and Windows
- **gRPC or similar** for internal subsystem communication (future)

## Technology Stack
- **Language:** Rust (performance, safety, native async)
- **HTTP Framework:** Axum
- **LLM Client:** Custom async HTTP client with streaming (reqwest)
- **Config:** TOML files
- **Logging:** Structured JSON logging (tracing)

## Dependencies on Other Subsystems
- **Knowledge Graph:** For fetching and updating facts
- **Connectors:** For reading/writing external service data
- **Reasoning Engine:** For complex multi-step queries
