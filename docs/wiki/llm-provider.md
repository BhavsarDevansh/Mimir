# Using Mimir as Your LLM Provider

Mimir can act as a drop-in OpenAI-compatible LLM provider. Any app or device that speaks the OpenAI chat-completions API — the same contract used by tools like Open WebUI, Continue, or a custom script — can point at your Mimir daemon and get answers that draw on Mimir's memory, personality, and tools.

## How It Works

Point your app at `http://<mimir-host>:8080/v1` and use the daemon's API token as the API key. Two endpoints are available:

- `GET /v1/models` lists the personality presets you can use as model names.
- `POST /v1/chat/completions` answers chat requests, with or without streaming.

Every conversation is stored in Mimir's central profile, so anything you say from any device becomes part of the same memory — and Mimir's learning hooks pick up new facts from those conversations automatically.

## Quick Example

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer $(cat ~/.local/share/mimir/api_token)" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "transparent",
    "messages": [{"role": "user", "content": "What is on my calendar tomorrow?"}],
    "user": "my-phone"
  }'
```

## Choosing a Conversation

The `user` field is your conversation key. Use a fixed value per conversation (for example `"my-phone"` or `"work-laptop"`) and Mimir resumes that conversation on every request. Omit `user` for a one-off incognito-style question: Mimir still uses its memory to answer, but stores nothing and learns nothing from it.

## Model Names

The `model` field accepts two kinds of values:

- A personality preset name (run `mimir personality list` to see them) — Mimir answers in that personality.
- Any other name — passed through to your configured upstream LLM as a model override, with your default personality.

## Tools

Mimir's own tools (time, weather, knowledge-graph search, and more) are always available. You can also send your own tool schemas in the `tools` field: when Mimir calls one of your tools, the response ends with `finish_reason: "tool_calls"` and the tool call details, and you send the result back as a `tool` message to continue the conversation. If one of your tools has the same name as a Mimir tool, Mimir's version wins.

## Streaming

Set `"stream": true` for token-by-token responses. Add `"stream_options": {"include_usage": true}` to receive a final usage chunk before the stream ends.

## Best Practices

- Keep one `user` value per conversation so history and memory stay coherent.
- Use the daemon's API token as the API key; every route except `/health` requires it.
- For remote devices, put the daemon behind a reverse proxy with TLS (Tailscale, WireGuard, or similar) — the daemon itself stays reverse-proxy-first.
- Prefer a preset name as the model unless you specifically want a different upstream model.
