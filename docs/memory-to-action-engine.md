# Memory-to-Action Engine

## Overview

The Memory-to-Action Engine is Mimir's Rust-owned agency layer. It converts typed memory and connector events into evidence-backed beliefs, versioned rules, scored opportunities, policy decisions, and audited actions. The LLM assists with extraction, explanation, and synthesis, but it never controls workflow or executes actions.

The roadmap source of truth is [`Memory-to-Action-Engine.md`](../VISION/09-Roadmap/Memory-to-Action-Engine.md).

## Implementation Layers

| Layer | Responsibility |
| --- | --- |
| Typed event bus | Durable post-commit delivery of fact, belief, connector, temporal, and action events. |
| Rule and pattern registry | Versioned conditions, evidence requirements, confidence thresholds, cooldowns, permissions, risk classes, and lifecycle state. |
| Scorer and policy engine | Deterministic ranking and a single authorization point for proactive decisions. |
| Action executor | Schema-validated, permission-gated, idempotent execution through connector capabilities. |
| Feedback loop | Bounded learning from accepted, rejected, corrected, failed, and undone outcomes. |

## Contract

- Events carry stable IDs, event type and schema version, actor, source/event time, correlation ID, privacy state, provenance, confidence, and a minimal payload.
- Rules are versioned, inspectable, and stored outside prompts.
- Every action has a stable action type, versioned payload schema, capability, permission, risk class, idempotency strategy, and audit contract.
- The policy engine is the only action authorizer.
- Every decision records rule version, evidence IDs, input snapshot, policy version, decision reason, expiry, result, and feedback.
- LLM output may only supply candidate facts, rules, wording, or research summaries after deterministic validation.
- No sensitive payload is stored or propagated without an explicit privacy decision.

## Planned Issues

| Issue | Role |
| --- | --- |
| [#583](https://github.com/BhavsarDevansh/Mimir/issues/583) | Typed memory event bus. |
| [#584](https://github.com/BhavsarDevansh/Mimir/issues/584) | Typed pattern and rule registry. |
| [#585](https://github.com/BhavsarDevansh/Mimir/issues/585) | Opportunity scorer and policy engine. |
| [#586](https://github.com/BhavsarDevansh/Mimir/issues/586) | Permission-gated action executor. |
| [#587](https://github.com/BhavsarDevansh/Mimir/issues/587) | Feedback loop and preference learning. |
| [#588](https://github.com/BhavsarDevansh/Mimir/issues/588) | Flight preparation reference rule. |
| [#589](https://github.com/BhavsarDevansh/Mimir/issues/589) | Home Assistant state/action rule. |
| [#590](https://github.com/BhavsarDevansh/Mimir/issues/590) | Spatial readiness triggers. |

## Validation

The engine must extend the existing memory benchmark instead of introducing a separate measurement path. Future fixtures should cover event delivery, idempotency, rule evaluation, scoring, policy enforcement, action audit, outcome feedback, privacy gating, and journey-level suppression quality. Until implemented, this document describes the planned architecture rather than a completed subsystem.
