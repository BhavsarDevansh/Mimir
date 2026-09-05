# Memory-to-Action Engine

> **Status:** Active
>
> **Parent Epic:** #567
>
> **Primary Issues:** #583, #584, #585, #586, #587, #588, #589, #590
>
> **Last Updated:** 2026-09-05

## Purpose

Mimir should become a Memory-to-Action Engine rather than only a chat assistant with memory. The system must detect changes, derive evidence-backed beliefs, score opportunities, enforce policy, execute or suggest actions, and learn from feedback. This is the product definition that turns Mimir from a memory-backed chat interface into an autonomous personal assistant.

The LLM is a bounded component for extraction, explanation, research synthesis, and candidate generation. Rust owns event delivery, inference, scoring, policy, authorization, action execution, audit, and feedback. No LLM output may directly execute an action, alter permissions, or bypass privacy controls.

## Architecture

```text
connectors / calendar / chat / vision / spatial input
  -> typed memory events
  -> evidence-backed beliefs
  -> versioned rules
  -> scored opportunities
  -> policy decisions
  -> permission-gated actions
  -> audit + outcome events + feedback
```

The architecture has one control plane and one action plane. The control plane turns evidence into ranked opportunities and policy decisions. The action plane validates and executes actions through connector capabilities. Neither plane reads prompts as executable workflow logic.

## Direction

1. Promote the Memory-to-Action Engine ahead of the broad Phase 4 reasoning framework.
2. Build the deterministic event, rule, policy, and action layers first.
3. Prove the system with one complete travel journey before adding more autonomous domains.
4. Keep memory work as the foundation, but do not require perfect recall, rendering, or consolidation before the first useful journey.
5. Keep broad reasoning and web research behind bounded, audited services invoked by deterministic rules.
6. Treat Home Assistant as the first high-risk external action surface; defer broad Matter support until this works.
7. Keep spatial and vision intelligence behind explicit permissions and privacy controls.

## Milestones

### M0 — Direction Reset

Create this roadmap as the planning source of truth, update the implementation context and user documentation, and ensure all issue relationships point to the new architecture. Phase 5 remains the implementation home for proactive intelligence, but its framing is the Memory-to-Action Engine.

**Outcome:** The repository, GitHub issues, and documentation all describe the same Rust-owned agency model.

### M1 — Typed Memory Event Bus (#583)

Build a durable typed event/outbox layer. Events must be emitted after the source transaction commits and must carry stable event IDs, event type and schema versions, actor, source/event time, correlation ID, privacy state, provenance, confidence, and a minimal payload. Delivery must be at-least-once, idempotent, ordered where required, bounded, observable, and coalescible.

The first supported event families should include fact changes, observations, mental-model updates, connector state, calendar changes, upcoming temporal events, and action outcomes. Existing hooks and jobs should integrate with the event bus rather than creating another queueing mechanism.

**Outcome:** Memory and connector changes can safely trigger deterministic rules.

### M2 — Typed Rule and Pattern Registry (#584)

Create a versioned registry for proactive rules and learned patterns. A rule must declare trigger events, conditions, evidence requirements, confidence and freshness thresholds, cooldowns, rate limits, permissions, risk class, action templates, explanation templates, lifecycle state, and audit metadata.

Rules are data and configuration, not prompt logic. LLM-proposed rules may exist only as candidate structures and must be validated by Rust before activation. Learned patterns remain evidence-backed and cannot become active rules without deterministic confidence and lifecycle checks.

**Outcome:** Behaviour is inspectable, testable, versioned, and auditable.

### M3 — Scorer and Policy Engine (#585, #587)

Build deterministic scoring and a single policy authorization point. The scorer evaluates urgency, relevance, confidence, corroboration, connector reliability, novelty, intrusiveness, action risk, user preferences, and freshness. The policy engine decides to ignore, queue, notify, suggest, require confirmation, execute, or remain read-only.

Feedback from accepted, ignored, expired, rejected, corrected, snoozed, undone, failed, and disabled outcomes updates bounded preferences, confidence, cooldowns, confirmation requirements, and autonomy. Explicit user feedback outranks inferred feedback.

**Outcome:** Every proactive decision has an explainable score, persisted policy decision, evidence trail, and auditable feedback path.

### M4 — Typed Action Executor (#586)

Create a capability registry and action executor. Each action must have a stable action type, versioned payload schema, connector capability contract, permission, risk class, idempotency strategy, timeout and retry policy, audit record, and outcome event. The executor distinguishes suggest, confirm-then-act, autonomous, and read-only behaviour.

The executor is the only path to external effects. It validates action schema and connector capability, checks auth, health, rate limits, and authorization, executes with bounded retries, records success or failure, emits outcome events, and attempts undo or compensation where supported.

**Outcome:** External actions are safe, inspectable, typed, permission-gated, and auditable.

### M5 — Flight Reference Journey (#588)

Implement the first end-to-end journey with upcoming travel. Rust consumes flight or upcoming-event evidence, resolves departure, destination, return, and travel context, checks existing packing evidence and preferences, scores the opportunity, applies policy, emits a notification or suggestion, and records feedback.

The first implementation may use deterministic packing guidance from destination, season, trip duration, user preferences, and available connector data. Weather and travel advisories are optional context and may be added only when enabled.

**Outcome:** The first complete memory-to-action journey works and is benchmarked.

### M6 — Home Assistant Reference Journey (#589)

Implement typed Home Assistant state ingestion and a first controlled action, such as pre-cooling when the user is heading home to a hot house. State events must be typed and freshness-aware, actions must use validated capabilities, and sensor/action loops must be prevented.

Use explicit permissions, quiet hours, cooldowns, confirmation rules, and audit. Home Assistant is the first external action connector because it already provides a broad device gateway without requiring Mimir to implement Matter directly.

**Outcome:** The first high-risk external action is safe, audited, and user-controlled.

### M7 — Spatial and Vision Triggers (#590)

Prepare typed spatial readiness rules for object presence, departure, arrival, item location, and list mutations. This must require explicit camera and object-tracking permissions, store only bounded typed observations by default, and exclude raw frames from normal event payloads.

Rules may support cases such as reminding the user when leaving without an item, locating an item, or adding a missing item to a shopping list.

**Outcome:** Spatial intelligence reaches proactive actions without sacrificing privacy or auditability.

### M8 — Bounded Research Service

Reframe the Phase 4 reasoning engine as a bounded research and investigation service. The deterministic planner gathers and ranks evidence from the knowledge graph, connectors, and enabled external sources. The LLM may synthesize prose only after Rust has validated evidence and enforced budgets.

This service may be invoked by proactive rules, but it must not become the agent controller.

**Outcome:** Complex research remains useful without displacing deterministic agency.

## Implementation Rules

- Rust owns triggers, inference, scoring, policy, execution, audit, and feedback.
- The LLM never directly executes actions, schedules jobs, changes permissions, or decides control flow.
- Inference is typed and domain-specific; arbitrary missing-middle reasoning is a bounded candidate-belief process, not an automatic fact insertion.
- Events are typed, versioned, privacy-aware, idempotent, and durable.
- Actions are typed, schema-validated, permission-gated, idempotent, and reversible where possible.
- Every decision must be traceable to rule version, evidence IDs, input snapshot, policy version, decision reason, and outcome.
- Sensitive data is excluded from event and audit payloads unless policy explicitly permits it.
- No implementation should bypass the existing hooks, jobs, connector supervisor, or knowledge-graph facade.

## Testing And Benchmarking

Extend issue #568's memory benchmark rather than creating another benchmark system. Add fixtures and metrics for event throughput, handler latency, rule evaluation latency, opportunity scoring, policy correctness, action audit coverage, outcome feedback learning, privacy false-allow/false-block rates, and journey-level suppression quality.

At minimum, test:

- event delivery after commit;
- idempotent delivery under retry;
- rule activation, cooldown, and lifecycle controls;
- scoring and policy determinism;
- permission and privacy enforcement;
- typed action validation and execution;
- action outcome and undo behaviour;
- feedback-driven confidence and cooldown updates;
- flight preparation, Home Assistant, and spatial readiness journeys.

## Current Issue Relationships

```text
#567 (parent epic)
  ├─ #583 typed memory event bus
  │     └─ blocks #584
  ├─ #584 typed rule and pattern registry
  │     └─ blocks #585
  ├─ #585 opportunity scorer and policy engine
  │     └─ blocks #586 and #587
  ├─ #586 permission-gated action executor
  │     └─ blocks #587, #588, #589, #590
  ├─ #587 feedback loop and preference learning
  │     └─ blocks #588, #589, #590
  ├─ #588 flight preparation reference rule
  ├─ #589 Home Assistant state/action rule
  └─ #590 spatial readiness triggers
```

These relationships should be retained in issue metadata so implementation order and blockers remain visible in GitHub.
