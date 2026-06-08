# Skills

Skills are Mimir's way of performing **multi-step workflows** reliably and repeatedly. Think of a skill as a recipe: instead of the agent improvising with raw tools every time, it can follow a proven script.

## What Is a Skill?

A skill is a named, versioned capability that the LLM can invoke just like a tool. When invoked, it runs a workflow that may:
- Call multiple tools in sequence
- Use the LLM for reasoning or synthesis
- Return structured results

## Types of Skills

### Built-in Skills
These ship with Mimir and are written in Rust for speed and reliability.

Current built-ins:
- **`research_synthesis`** — Takes a topic, breaks it into sub-questions, and synthesizes a narrative.
- **`test_driven_development`** — Takes a programming task and returns a red-green-refactor plan.

### User-Added Skills
You can write your own skills as Markdown files with YAML frontmatter. Drop them into `~/.config/mimir/skills/` and they are loaded automatically.

**Example: `~/.config/mimir/skills/weekly-summary.md`**

```markdown
---
name: weekly-summary
version: 1.0.0
description: Summarize my week from calendar and knowledge graph.
tags: [productivity, summary]
---

# Weekly Summary

1. Look at the last 7 days in the knowledge graph.
2. Check calendar for meetings and travel.
3. Synthesize into a concise narrative.
4. Ask if the user wants it inserted into the knowledge graph via fact extraction.
```

The Markdown body is sent to the LLM as instructions, so you can write skills in plain English.

### System-Generated Skills (Future)
After Mimir completes a complex task successfully, it may automatically create a skill so it can handle similar tasks faster next time. This is planned for a future update.

## How to Add a Skill

1. Create a `.md` file in `~/.config/mimir/skills/`.
2. Start with `---`, write YAML frontmatter, close with `---`.
3. Write the skill instructions in Markdown below.
4. Run `mimir skill list` to confirm it loaded.

Or use the CLI:
```bash
mimir skill add /path/to/my-skill.md
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `mimir skill list` | Show all skills |
| `mimir skill list --origin builtin` | Show only built-in skills |
| `mimir skill list --tag travel` | Show skills tagged "travel" |
| `mimir skill show <name>` | Show skill details |
| `mimir skill add <path>` | Add a user skill from file |
| `mimir skill delete <name>` | Remove a user skill |
| `mimir skill enable <name>` | Allow the skill to run automatically |
| `mimir skill disable <name>` | Prevent the skill from running |
