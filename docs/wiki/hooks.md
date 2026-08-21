# Background Hooks

Mimir learns from your conversations and connected services through **background hooks** — small, typed background tasks that the daemon runs automatically. Hooks replaced the old approach where the chat model decided for itself whether to call a `remember` tool, which made learning depend on the model's behaviour and could be steered by what you said in the conversation.

## How it works

When something happens that Mimir might learn from — you finish a chat turn, a connector stages a new email, or a fact is written to the knowledge graph — the daemon enqueues a hook instance. A single background dispatcher then runs each instance through the durable job queue under rules that keep background work from slowing down interactive chat:

- **Debounce** — a burst of chat messages becomes one extraction instead of many, so Mimir does not fire an LLM call for every single message.
- **Idle gating** — learning and memory condensation wait until you have stopped chatting and the LLM worker pool is idle, so background work never steals capacity from your conversation.
- **Retry** — transient failures are retried with backoff; permanent failures are recorded and dropped, so the same item is not re-processed for its current identity (a new mailbox epoch, e.g. after a `UIDVALIDITY` change, gives the message a fresh attempt).

The pending queue lives in memory, so a daemon restart only loses work that has not started yet. Chat re-triggers on your next turn and memory condensation re-triggers on the next fact write. Connector items whose extraction was still in flight when the daemon stopped are skipped (the sync cursor has already advanced past them), while messages the connector recorded as durable queue-overflow when the hook queue was full are re-staged on the next start; a failed sync cycle re-fetches its window on the next cycle, and a full re-sync re-stages items that were not terminally failed.

## The three hooks

- **`remember.chat`** — after each non-incognito chat turn, Mimir extracts facts from the accumulated conversation and stores them through the same deterministic pipeline as before (confidence, overwrite, sensitive-fact confirmation). Incognito turns never enqueue any hook and never write facts.
- **`connector_item.remember`** — each staged connector item (e.g. an email that needs LLM extraction) is processed individually in order. Structured parsing runs first; the LLM hook only runs when needed.
- **`memory.condensation`** — when facts change, Mimir rebuilds the condensed memory block that is injected into your conversations. The manual "refresh memory" action force-runs this hook immediately.

## What changed for you

- Learning no longer depends on the chat model deciding to call a tool, so it works with any OpenAI-compatible client and cannot be talked out of remembering.
- The `remember` tool is gone from the model's tool set and from the system prompt.
- `GET /status` now reports `hook_queue_depth`, the number of pending hook instances.
- The debounce window for chat learning is configurable via `agent.remember_debounce_seconds` (default 10 seconds).

## See also

- [How Mimir Learns Facts](fact-extraction.md)
- [The Librarian Agent](librarian-agent.md)
- [Knowledge Graph](knowledge-graph.md)
- [Connectors](connectors.md)
