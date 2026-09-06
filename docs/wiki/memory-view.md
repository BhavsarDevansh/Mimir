# Composed Memory View

Mimir now uses one shared memory view when composing what the agent knows and what is coming up. Chat, `/memory`, `/status`, and the CLI memory and status commands all read from that same view, so they show the same memory ordering, temporal context, and budget accounting.

The view contains two parts: the condensed stable memory and the freshly rendered upcoming events. It also records whether either part was unavailable, the request-local UTC time, the configured temporal horizon, character and approximate token usage, and any warnings. Fact-level confidence, provenance, privacy, and controls remain attached to the facts themselves; view-level states provide the stable seam for richer controls in #581, #582, and #284. If memory is disabled or Mimir cannot resolve your user identity for upcoming events, that state is explicit instead of being quietly omitted.

Chat prompts use the budgeted rendering policy. It always keeps the condensed core memory and trims only the upcoming section if the combined memory would exceed the configured character limit; the system prompt then adds one request-local UTC time anchor. `/memory` uses full rendering so you can inspect all available memory content.

This improves consistency without changing how you use Mimir. You can still call `mimir memory` to see the live memory block, `mimir status` to see memory size and usage, and chat normally to receive memory in the system prompt. More detailed provenance, citations, privacy controls, and pin/deprioritization controls will build on this shared view in their own follow-up issues.
