# Skill Registry

## Overview

The Skill Registry is Mimir's higher-level capability system. While **Tools** are deterministic, single-step operations, **Skills** are multi-step workflows that combine tools, LLM reasoning, and structured prompts.

Skills are registered alongside tools and exposed to the LLM via the same OpenAI-compatible function-calling format. The LLM chooses whether to call a raw tool (novel task) or a skill (well-understood workflow).

## Architecture

```
mimir-core/src/skills/
├── mod.rs                  # Skill trait, SkillContext, SkillInput/Output
├── error.rs                # SkillError
├── registry.rs             # SkillRegistry
├── markdown.rs             # YAML frontmatter parser + MarkdownSkill
├── metrics.rs              # SQLite skill_metrics tracking
├── generated.rs            # System-generated skill scaffolding
└── builtins/
    ├── mod.rs
    ├── research_synthesis.rs
    └── test_driven_development.rs
```

## Skill Trait

```rust
#[async_trait]
pub trait Skill: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn permission(&self) -> ToolPermission;
    async fn execute(&self,
        ctx: SkillContext,
        input: SkillInput,
    ) -> Result<SkillOutput, SkillError>;
}
```

`SkillContext` provides every skill with access to:
- `tool_registry: Arc<ToolRegistry>` — invoke any registered tool
- `llm_client: Arc<LlmClient>` — make LLM calls
- `context_manager: Arc<ContextManager>` — read/write conversation state
- `session_id: Option<String>` — current session, if any

## Skill Types

| Type | Origin | Discovery | Lifecycle |
|------|--------|-----------|-----------|
| **Built-in** | `mimir-core/src/skills/builtins/` | Auto-registered at startup | Versioned with crate |
| **User-added** | `~/.config/mimir/skills/*.md` | Scanned from disk at startup | User-managed |
| **System-generated** | Created by agent after complex tasks | Registered after generation | Auto-managed (Phase B) |

## User Skill Format

User skills are Markdown files with YAML frontmatter:

```markdown
---
name: weekly-summary
version: 1.0.0
description: Summarize my week.
tags: [productivity, summary]
parameters:
  type: object
  properties:
    days:
      type: integer
      description: Number of days to look back.
  required: [days]
---

# Weekly Summary

Summarize the user's week from calendar and knowledge graph.
```

If `parameters` is omitted, defaults to `{"query": string}`.

## Execution Model

**Built-in skills** are native Rust structs implementing `Skill`. They have direct, deterministic control over tools and LLM calls.

**User-added skills** use the **prompt-as-skill** model (Hybrid C):
1. YAML frontmatter is parsed into metadata.
2. Markdown body is sent to the LLM as a system prompt.
3. Input arguments are serialized into the user message.
4. The LLM executes the skill steps and returns the result.

This makes user skills easy to write and share without recompiling Mimir.

## Metrics

Every skill invocation is tracked in `~/.local/share/mimir/skills.db`:

| Column | Type | Description |
|--------|------|-------------|
| `skill_name` | TEXT PRIMARY KEY | Skill identifier |
| `invocation_count` | INTEGER | Total invocations |
| `success_count` | INTEGER | Successful invocations |
| `failure_count` | INTEGER | Failed invocations |
| `avg_latency_ms` | INTEGER | Rolling average latency |
| `last_invoked_at` | DATETIME | Last invocation timestamp |
| `user_correction_count` | INTEGER | Corrections received |
| `avg_token_cost` | INTEGER | Rolling average token cost |
| `utility_score` | REAL | Computed composite score (Phase B) |

## System-Generated Skills (Scaffolded)

Phase A includes the trigger detector only. After a session closes, the agent can call `should_generate_skill(&SessionSummary)` to determine if the four conditions are met:
1. ≥ 3 distinct tools used
2. Task succeeded
3. Novel pattern
4. Confidence > 0.85

The actual Markdown generation and file-writing is stubbed for Phase B.

## CLI

```bash
mimir skill list                    # All skills
mimir skill list --origin builtin   # Filter by origin
mimir skill list --tag travel       # Filter by tag
mimir skill show <name>             # Show metadata
mimir skill add <path>              # Copy .md to ~/.config/mimir/skills/
mimir skill delete <name>           # Delete user skill
mimir skill enable <name>            # Set permission to Auto
mimir skill disable <name>           # Set permission to Disabled
```
