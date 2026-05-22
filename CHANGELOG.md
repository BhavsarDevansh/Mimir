# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] - 2026-05-22

### Added

- **Skill Registry** (`mimir-core/src/skills/`): higher-level workflows registered alongside tools.
  - `Skill` trait with `SkillContext` providing access to `ToolRegistry`, `LlmClient`, and `ContextManager`.
  - `SkillRegistry` supporting built-in, user-added, and system-generated skill origins.
  - `SkillInput` / `SkillOutput` / `SkillError` types mirroring the tool layer.
  - Built-in skills: `research_synthesis` and `test_driven_development`.
  - User skill loading from `~/.config/mimir/skills/*.md` with YAML frontmatter parser (`serde_yaml`).
  - `MarkdownSkill` execution model: body is sent as a system prompt to the LLM with input arguments.
  - SQLite `skill_metrics` table for invocation tracking (`skill_metrics.db`).
  - System-generated skill scaffolding: `SessionSummary` and `should_generate_skill()` trigger detector.
  - CLI commands: `mimir skill list`, `show`, `add`, `delete`, `enable`, `disable`.
  - `SkillRegistry::export_openai_tools()` for OpenAI-compatible function-calling exposure.
