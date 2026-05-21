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
  - VISION compliance

## Commit & Authorship
- Do not co-sign or co-author commits or pull requests.

## Branching Workflow
- **Always create a new branch before starting work.** Never commit directly to `main`.
- Use the following branch naming conventions:
  - Features / enhancements: `feat/[task-name]`  
    Example: `feat/configuration-system`, `feat/issue-2-config`
  - Bug fixes: `bugfix/[bug-description]`  
    Example: `bugfix/config-parse-error`, `bugfix/issue-5-env-override`
- Branch names should be lowercase, use hyphens instead of spaces, and be descriptive enough to identify the issue or task.

## Finishing Work
After implementation is complete and all tests pass:
1. Stage the changes (`git add ...`).
2. Commit with a clear, descriptive message summarising what was done.
3. Push the branch to the remote (`git push origin [branch-name]`).
4. Create a Pull Request (PR) that links back to the original issue.
   - The PR description should contain a closing statement such as:  
     `Closes #2` or `Fixes #5`
   - Summarise the key changes and reference any updated documentation.
5. Do not merge the PR yourself unless explicitly asked.

## Workflow
1. Read `Mimir-Implementation-Context.md` and relevant `VISION/` docs.
2. Use Context7 for any library/framework/API guidance.
3. Write failing tests (TDD).
4. Implement minimally and correctly.
5. Verify tests pass.
6. Update `docs/` and `docs/wiki/`.
7. Run code review.
8. Proceed only after review is clean.
