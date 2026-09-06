# Composed Memory View

## Purpose

The daemon previously assembled condensed memory and upcoming events separately in the chat routes, the `/memory` endpoint, and the `/status` endpoint. That duplication made it easy for temporal anchors, degradation handling, content ordering, and budget accounting to drift as the memory system gained capabilities.

`mimir-server/src/memory_view.rs` now owns one `ComposedMemoryView` builder. The builder reads the condensation cache and upcoming section from the knowledge graph, applies the configured temporal horizon and character limit, and returns structured data plus a deterministic rendering policy. Native chat, the OpenAI-compatible provider surface, `/memory`, `/status`, and the CLI through those APIs all consume the same view.

## Structured Output

The view carries the condensed core memory, the freshly rendered upcoming section, the request-local UTC `now` timestamp, the temporal horizon, availability and degradation flags, and warnings. It also carries explicit status, confidence, provenance, privacy, and user-control states. Confidence, provenance, privacy, and control are currently explicit `Unknown`, `Unavailable`, `NotEvaluated`, and `NotConfigured` values because their fact-level controls land in #581, #582, and #284; the builder now gives those subsystems one stable place to attach richer state without changing every consumer again.

`MemoryViewUsage` records the character count, configured limit, percentage, approximate token count, and whether the composed content is within budget. The budgeted rendering policy preserves the condensed core and truncates only the upcoming section when the combined content exceeds the configured limit; it omits the `Now:` anchor because the system-prompt composer supplies exactly one request-local anchor.

## Degradation And Rendering Policies

`Full` rendering preserves all content for user inspection, including the normal `No stable memory yet.` fallback. `Budgeted` rendering returns an empty prompt-memory block when there is no core memory and truncates upcoming content to fit the configured character limit. A disabled memory subsystem or an unresolved user identity is represented with warnings and degraded upcoming availability rather than being silently folded into an empty string. Condensed-memory database failures remain an error at the `/memory` transport boundary while `/status` continues to report any available upcoming memory consistently with its usage metrics.

## System Connections

The condensation pipeline remains responsible for producing and caching the stable core memory. The knowledge graph remains responsible for deterministic upcoming rendering. The shared server builder is the presentation seam that combines those sources, records their quality, and keeps route-specific transport and error behaviour separate from memory assembly.
