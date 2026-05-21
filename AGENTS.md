# Mimir Project Rules

## Context Sources
- Always consult `Mimir-Implementation-Context.md` to understand the project.
- Reference documents in the `VISION/` directory for project goals and task context.

## Development Standards
- Use **Don't Repeat Yourself (DRY)** development.
- Use **Test Driven Development (TDD)** — write failing tests first, then implement.
- Use **Context7** (and crates.io if needed) to fetch current documentation for libraries, frameworks, SDKs, APIs, CLI tools, or cloud services **before designing or planning** — even well-known ones. Use the official library name with proper punctuation (e.g., "tokio" not "tokio-rs", "axum" not "axum-rs").
  - **Verify versions:** Check the latest stable version and correct feature flags on crates.io before adding any dependency. Do not rely on training-data version knowledge.
  - **Verify patterns:** Confirm current best practices, recommended APIs, and common pitfalls from Context7 docs before writing implementation code or plans.
  - **Verify alternatives:** Check if simpler or more modern alternatives exist before committing to a library or pattern.
- Ensure **performance and security** are at the forefront of all decisions.
- Use the **smallest data type needed** for efficient memory utilization. Be smart at initialization (e.g., prefer `u8` over `u16` when sufficient), but do not cast existing values defined by libraries unless absolutely necessary.

## Documentation
- After each set of changes, create/update technical documentation in `docs/`.
- After each set of changes, create/update non-technical (user-facing) documentation in `docs/wiki/`.
- Technical docs should cover: implementation details, rationale, system connections.
- Wiki docs should cover: feature description, how it works, use cases, best practices.
- Create `docs/` and `docs/wiki/` if they do not exist.

## Code Review
**Code review is mandatory and non-negotiable.** It must be run after every set of changes, after documentation is updated, and before any commit is made.

**Process:**
1. Run the code review pass against every file touched in the change set.
2. Produce findings in a structured table (dimension, finding, severity).
3. **All findings must be actioned, no matter how trivial.** There are no "optional" or "minor" exceptions. If a finding exists, fix it before proceeding.
4. Re-run tests, clippy, and fmt after every fix.
5. Only proceed to commit when the review returns zero findings.

**Checklist:**
- Code quality
- Performance
- Security
- Doc comments
- DRY compliance
- Modern Design Patterns
- Guideline compliance
- VISION compliance
- Type consistency across the workspace
- Public API surface changes documented

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
1. **Run code review** and action **all** findings.
2. Stage the changes (`git add ...`).
3. Commit with a clear, descriptive message summarising what was done.
4. Push the branch to the remote (`git push origin [branch-name]`).
5. Create a Pull Request (PR) that links back to the original issue.
   - The PR description should contain a closing statement such as:  
     `Closes #2` or `Fixes #5`
   - Summarise the key changes and reference any updated documentation.
6. Do not merge the PR yourself unless explicitly asked.

## Semantic Versioning

- After any work is done on the project — whether a feature, bugfix, refactor, or documentation update — bump the semantic version number in **all** workspace member `Cargo.toml` files (and the workspace root `Cargo.toml` if it declares a version) before committing.
- Follow [Semantic Versioning 2.0.0](https://semver.org/):
  - **PATCH** (`0.1.0` → `0.1.1`) for backwards-compatible bug fixes and minor documentation updates.
  - **MINOR** (`0.1.0` → `0.2.0`) for backwards-compatible new features, refactors, or subsystem additions.
  - **MAJOR** (`0.1.0` → `1.0.0`) for breaking changes to public APIs, configuration formats, or data models.
- Keep all crate versions in the workspace in sync unless there is an explicit, documented reason to diverge.
- Update `CHANGELOG.md` (or create it at the workspace root if absent) with a brief entry summarising the change for the new version.
- If the change set includes multiple logical changes (e.g., a feature plus a bugfix), bump the highest applicable version component once for the entire change set.

## Breaking Changes

Mimir is **not a public library** — it is a personal, self-hosted application. Breaking changes to internal APIs, configuration formats, or data models are **fully acceptable** when they improve code quality, correctness, or maintainability. Do not preserve backwards compatibility at the expense of better design. Public-facing interfaces (e.g., the OpenAI-compatible chat endpoint) are the only surfaces where stability matters.

## Planning Standards
- Every plan that introduces new dependencies must include version-checked dependency specifications.
- Every plan that uses a library API must cite the current best-practice pattern (e.g., "per reqwest 0.13 docs, use `bytes_stream()` with `stream` feature").
- If a library is well-known (tokio, axum, serde), still verify the latest guidance — training data may be stale.

## Workflow
1. Read `Mimir-Implementation-Context.md` and relevant `VISION/` docs.
2. Use Context7 (and crates.io if applicable) to verify:
   - Latest versions, feature flags, and compatibility of all proposed dependencies.
   - Current best practices and patterns for every library, framework, or API being introduced.
   - Do not proceed to planning until this check is complete and reflected in the plan.
3. Write failing tests (TDD).
4. Implement minimally and correctly.
5. Verify tests pass.
6. Update `docs/` and `docs/wiki/`.
7. Run code review and action every finding.
8. Proceed only after review returns **zero** findings.
