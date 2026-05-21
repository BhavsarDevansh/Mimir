<INSTRUCTIONS>
# Mimir Project Rules

## Context Sources
- Always consult `Mimir-Implementation-Context.md` to understand the project.
- Reference documents in the `VISION/` directory for project goals and task context.

## Development Standards
- Use **Don't Repeat Yourself (DRY)** development.
- Use **Test Driven Development (TDD)** — write failing tests first, then implement.
- Use **Context7** to fetch current documentation for libraries, frameworks, SDKs, APIs, CLI tools, or cloud services before using them — even well-known ones. Use the official library name with proper punctuation (e.g., "tokio" not "tokio-rs", "axum" not "axum-rs").
- Ensure **performance and security** are at the forefront of all decisions.
- Use the **smallest data type needed** for efficient memory utilization. Be smart at initialization (e.g., prefer `u8` over `u16` when sufficient), but do not cast existing values defined by libraries unless absolutely necessary.

## Documentation
- After each set of changes, create/update technical documentation in `docs/`.
- After each set of changes, create/update non-technical (user-facing) documentation in `docs/wiki/`.
- Technical docs should cover: implementation details, rationale, system connections.
- Wiki docs should cover: feature description, how it works, use cases, best practices.
- Create `docs/` and `docs/wiki/` if they do not exist.

## Code Review
- After each set of changes (and after docs are updated), run a code review pass that checks for:
  - Code quality
  - Performance
  - Security
  - Doc comments
  - DRY compliance
  - Modern Design Patterns
  - Guideline compliance

## Commit & Authorship
- Do not co-sign or co-author commits or pull requests.

## Workflow
1. Read `Mimir-Implementation-Context.md` and relevant `VISION/` docs.
2. Use Context7 for any library/framework/API guidance.
3. Write failing tests (TDD).
4. Implement minimally and correctly.
5. Verify tests pass.
6. Update `docs/` and `docs/wiki/`.
7. Run code review.
8. Proceed only after review is clean.
</INSTRUCTIONS>
