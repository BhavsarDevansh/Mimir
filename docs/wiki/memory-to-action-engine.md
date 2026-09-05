# Memory-to-Action Engine

## What It Is

The Memory-to-Action Engine is Mimir's automation layer. It turns what Mimir knows into useful action while keeping decisions in Rust. The LLM helps with language and research, but Rust decides what to do, when to do it, and whether it is allowed.

The roadmap source of truth is [`Memory-to-Action-Engine.md`](../../VISION/09-Roadmap/Memory-to-Action-Engine.md).

## How It Works

Mimir follows one pipeline:

1. Connectors and conversations create typed memory events.
2. Mimir turns related evidence into inspectable beliefs.
3. Versioned rules detect useful opportunities.
4. Mimir scores urgency, relevance, confidence, risk, and user preference.
5. The policy engine checks permissions, privacy, cooldowns, and feedback.
6. The action executor sends a suggestion, notification, or approved action.
7. Outcomes are recorded so future behaviour can improve.

## Planned Use Cases

- Detect an upcoming trip and suggest what to pack or research travel context.
- Detect a social event and warn about train disruption or route problems.
- Detect that you are heading home and pre-cool a hot house through Home Assistant.
- Remind you when you leave home without an important item.
- Tell you where an item was last seen when tracking is explicitly enabled.
- Adjust future suggestions from your accepted, rejected, corrected, or undone feedback.

## Why It Matters

This model keeps Mimir auditable and safe as it becomes more autonomous. You can inspect why Mimir acted, pause a rule, reject a suggestion, or withdraw permission. It also avoids wasting LLM calls on deterministic decisions, so routine behaviour can run like a background automation instead of a prompt-driven chat feature.

## Current Status

This feature is in active planning. The first implementation target is the flight preparation journey after the event bus, rule registry, scorer, policy engine, and action executor are in place.
