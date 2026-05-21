# LLM Client

## What It Does

Mimir's LLM client connects to any OpenAI-compatible API — OpenAI, Anthropic (via compatibility layer), local models (Ollama, LM Studio), Azure OpenAI, or custom endpoints — and handles both quick replies and real-time streaming responses.

## Supported Providers

Any endpoint that implements the OpenAI `/v1/chat/completions` API:
- **OpenAI** — GPT-4o, GPT-5, etc.
- **Anthropic** — Claude via compatibility layers
- **Local** — Ollama, LM Studio, llama.cpp
- **Azure** — Azure OpenAI Service
- **Custom** — Any self-hosted endpoint

## Configuration

Set in `~/.config/mimir/config.toml`:

```toml
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-4o"
max_tokens = 4096
temperature = 0.2
```

Or override with environment variables:

```bash
export MIMIR_LLM_API_KEY="sk-..."
export MIMIR_LLM_MODEL="gpt-5"
export MIMIR_LLM_ENDPOINT="http://localhost:11434/v1"
```

## How Streaming Works

When you ask Mimir something in chat mode, the client opens an SSE (Server-Sent Events) connection to the LLM. Tokens arrive one by one and are displayed immediately, so you see the response being typed out rather than waiting for the full answer.

## Error Handling

The client distinguishes between different failure types:
- **Network error** — your machine can't reach the server. Check your internet or endpoint URL.
- **API error** — the server returned an error (e.g., 401 invalid key, 429 rate limit).
- **Retry exhausted** — the server was temporarily unavailable and all retry attempts failed.
- **Parse error** — the response was not valid JSON. Usually indicates a misbehaving proxy or non-OpenAI endpoint.

## Best Practices

- **Keep `temperature` low (0.0–0.3)** for factual tasks; raise it for creative writing.
- **Set `max_tokens`** to avoid accidentally burning through large token budgets.
- **Use a local endpoint** if you want full privacy — no data leaves your machine.
- **Check `docs/llm-client.md`** for implementation details if you're contributing.
