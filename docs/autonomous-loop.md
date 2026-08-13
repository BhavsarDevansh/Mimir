# Autonomous Development Loop

> **Component:** `scripts/autonomous-loop.sh` (+ optional `scripts/systemd/` user timer)
> **Status:** Active

## Purpose

A self-driving orchestration script that advances the Mimir repository toward a fully implemented feature set without human intervention. On each cadence tick (default every 2 hours; `MIMIR_AUTONOMOUS_INTERVAL` seconds) it drives the GitHub pull-request lifecycle and picks up code-quality work from the issue tracker, delegating all coding work to `codex exec` subagents that load the project's `AGENTS.md` and the `gh-issue-tdd` / `gh-review-commit` skills. The script keeps all control flow in deterministic bash (git and `gh` calls) and only delegates open-ended engineering work (implementing, reviewing, addressing comments) to the agent, respecting the project rule that logic must live in deterministic code, not in prompts.

## Design

```text
autonomous-loop.sh
├── run_iteration()                 # one pass, guarded by flock
│   ├── git fetch origin --prune
│   ├── branch == main      -> handle_main_branch()
│   └── branch != main      -> handle_feature_branch()
│       ├── no open PR      -> clean? checkout main + pull
│       ├── draft PR        -> codex review+fix+push -> gh pr ready
│       └── ready PR        -> unresolved comments? codex fix+push
│                              else merge (CodeRabbit-skip aware) + back to main
└── delegates coding to: codex exec (gh-issue-tdd, gh-review-commit skills)
```

### Branch detection

`current_branch()` reads `git rev-parse --abbrev-ref HEAD`. A detached HEAD is treated as "go back to main and pull".

### Cadence and the review loop

With `MIMIR_AUTONOMOUS_INTERVAL=1800` (30 minutes) a typical issue lifecycle is: iteration 1 on main picks the next unblocked issue, implements it with TDD, updates docs, and pushes a DRAFT PR; iteration 2 (30 minutes later) runs a PR-style self-review against main, fixes every finding, and marks the PR ready; iteration 3 checks for unresolved review threads (CodeRabbit and human) and addresses them via the `gh-review-commit` skill; iteration 4 re-checks and merges once nothing is left open, then switches local main and pulls; iteration 5 (on main) picks the next ticket and the cycle repeats.

### PR inspection

- `pr_for_branch` — `gh pr list --head <branch> --state open`.
- `pr_is_draft` — `gh pr view <num> --json isDraft`.
- `count_unresolved_threads` — paginated `gh api graphql` over `reviewThreads`, counting nodes where `isResolved == false` and `isOutdated == false`. This is the source of truth for "are there open review comments" (CodeRabbit findings land here).
- `has_changes_requested` — `gh pr view --json reviews` for any `CHANGES_REQUESTED` state.
- `pr_mergeable` — gates merging on `mergeStateStatus` being `CLEAN`, so the loop does not auto-merge while checks are failing or GitHub reports the branch behind.

### CodeRabbit skip handling

`coderabbit_skipped()` inspects the latest `coderabbitai` review body for documented rate-limit markers such as "Review limit reached" or "Review rate limited". A skipped review produces no threads, so the PR proceeds through the normal "no open comments" path and still must satisfy the merge-state gate; the skip is logged explicitly so the audit trail explains why CodeRabbit did not block the merge.

### Issue selection (code quality only)

`candidate_issues()` lists open issues and filters: issues with the `blocked` label are skipped; `help-wanted` issues are skipped unless the repo owner has commented after the agent's most recent question marker; **feature development is excluded** — an issue must carry at least one quality label (`bug`, `refactor`, `maintenance`, `performance`, `security`, `documentation`, `testing`, `build`) or a quality title prefix (`DRY:`, `Robustness:`, `Refactor:`, `Maintenance:`, `Bug:`, `Fix:`, `Flaky`, `Spec drift`, `E2E test`, `Docs:`, `Security:`, `Perf`, `Cleanup`, `Code quality`), and the `feature` label plus `Implement` / `Future:` titles are hard exclusions. Issues carrying the active phase label (`MIMIR_AUTONOMOUS_PHASE_LABEL`, default `phase-3`) are listed first, then everything else, each group sorted by ascending issue number. The top candidates (up to 5) are handed to `codex exec`, which reads the roadmap and `Mimir-Implementation-Context.md` to confirm the right one and then implements it via the `gh-issue-tdd` skill. The prompt additionally instructs the agent never to implement feature development and to fix missing quality/feature labels it encounters.

### Issue hygiene

The implementation prompt requires the agent to: fetch the full issue (all fields plus every comment via `gh issue view N --comments` and the issues API); verify the spec against the current codebase and update outdated issue bodies with the latest context; read the relevant VISION docs; when genuinely blocked, post the exact human requirements plus the `help-wanted` label and the `<!-- mimir-autonomous-question:N -->` marker instead of guessing; file new GitHub issues for out-of-scope problems (misplaced code, DRY violations, performance, bugs, security) using only existing labels; and keep README.md, docs/wiki/what-works-now.md and AGENTS.md accurate while updating docs.

### Question / answer protocol

When an issue needs clarification, the agent does not implement it. Instead it adds the `help-wanted` label (`gh issue edit <N> --add-label help-wanted`) and posts a comment whose final line is the hidden marker `<!-- mimir-autonomous-question:N -->`. `issue_questions_answered()` finds the latest comment containing that marker, records its timestamp and author, and returns true only if a later comment by the repository owner exists. This lets the loop revisit answered tickets on a future run while skipping tickets whose questions are still outstanding, and keeps the loop moving to a different ticket when a human is needed.

### Delegation to `codex exec`

`run_codex()` writes the prompt to a temp file and pipes it via stdin (`codex exec ... -`). Default sandbox is `workspace-write`. Set `MIMIR_AUTONOMOUS_BYPASS=1` to pass `--dangerously-bypass-approvals-and-sandbox` only when fully unattended full-access operation is explicitly required. Set `MIMIR_AUTONOMOUS_CODEX_ARGS` to a word-split string of extra codex CLI flags that take full control of the provider, sandbox, model and config overrides — for example `"--oss -m deepseek-v4-flash:cloud --yolo --config model_reasoning_effort=max --config model_context_window=1000000"` runs every delegated session on the OSS deepseek backend with full access and a 1M context window. When `MIMIR_AUTONOMOUS_CODEX_ARGS` is set, `MIMIR_AUTONOMOUS_SANDBOX`, `MIMIR_AUTONOMOUS_MODEL` and `MIMIR_AUTONOMOUS_BYPASS` are ignored.

### Conversation-only logging

`run_codex()` invokes codex with `--json` and pipes the JSONL event stream through `log_agent_stream()`, which keeps only the agent's conversational messages (`agent_message` items) and fatal codex errors (`error` / `turn.failed` events) and drops everything else. File contents the agent read, shell commands and their output, patches, web searches and reasoning never reach `autonomous.log` (or the terminal), so the log stays a readable, grep-friendly conversation record and never persists sensitive data. Full raw transcripts remain available in codex's own session files under `~/.codex/sessions/`. The filtering is covered by `scripts/tests/autonomous-loop_test.sh`, which feeds a realistic `codex exec --json` fixture through the filter and asserts transcripts never leak into the log.

## Configuration (environment variables)

| Variable | Default | Meaning |
|---|---|---|
| `MIMIR_AUTONOMOUS_INTERVAL` | `7200` | Seconds between iterations (loop mode); `1800` gives a 30-minute cadence. |
| `MIMIR_AUTONOMOUS_SANDBOX` | `workspace-write` | codex `-s` sandbox mode. |
| `MIMIR_AUTONOMOUS_MODEL` | _unset_ | Override the codex model. |
| `MIMIR_AUTONOMOUS_BYPASS` | `0` | `1` => `--dangerously-bypass-approvals-and-sandbox`. |
| `MIMIR_AUTONOMOUS_CODEX_ARGS` | _unset_ | Extra codex CLI flags (word-split); when set, takes full control of provider/sandbox/model flags and ignores `MIMIR_AUTONOMOUS_SANDBOX`, `MIMIR_AUTONOMOUS_MODEL` and `MIMIR_AUTONOMOUS_BYPASS`. |
| `MIMIR_AUTONOMOUS_LOG` | `${XDG_STATE_HOME:-$HOME/.local/state}/mimir/autonomous.log` | Log file path. |
| `MIMIR_AUTONOMOUS_PHASE_LABEL` | `phase-3` | Issue label prioritised as the active roadmap phase. |
| `MIMIR_AUTONOMOUS_DRY_RUN` | `0` | `1` => dry run (print actions, skip codex). |

## Running

### One-off / loop

```bash
scripts/autonomous-loop.sh --once        # single iteration
scripts/autonomous-loop.sh               # loop forever, 2h cadence
MIMIR_AUTONOMOUS_INTERVAL=1800 scripts/autonomous-loop.sh   # loop forever, 30-min cadence
scripts/autonomous-loop.sh --dry-run     # preview decisions
```

### systemd user timer (recommended for persistence)

```bash
mkdir -p ~/.config/systemd/user
cp scripts/systemd/mimir-autonomous.{service,timer} ~/.config/systemd/user/
# edit the ExecStart/WorkingDirectory paths in the service if your checkout lives elsewhere
systemctl --user daemon-reload
systemctl --user enable --now mimir-autonomous.timer
systemctl --user list-timers mimir-autonomous.timer
```

The timer fires every two hours via `OnCalendar=` with `Persistent=true`, so missed runs while the machine was off are caught up on boot. Each run is a `oneshot` invocation of `--once`, so a crashed iteration cannot stall the cadence.

### Stopping the loop

For a foreground or `setsid`-detached run, kill the process (`pkill -f autonomous-loop.sh`) and any running `codex exec` child. For the systemd timer, run `systemctl --user disable --now mimir-autonomous.timer`. The `flock` on `${XDG_STATE_HOME:-$HOME/.local/state}/mimir/autonomous.lock` prevents overlapping iterations, so a killed run never leaves a stale lock behind.

## Safety

- A `flock` on `${XDG_STATE_HOME:-$HOME/.local/state}/mimir/autonomous.lock` prevents overlapping iterations.
- Merging only happens when GitHub reports the PR merge state as `CLEAN` and no unresolved review threads remain; a CodeRabbit skip is logged and treated as a clear review only when those merge gates are also satisfied.
- The agent is instructed never to co-author commits, to follow `AGENTS.md` (including the no-unsafe policy and semantic versioning), and to only touch files referenced by review comments when addressing PR feedback.
- The orchestrator's actions and the agent's messages are timestamped in `autonomous.log`; the raw codex transcript (file contents, shell commands, patches) is never written to it.

## System connections

- Reads `AGENTS.md`, `Mimir-Implementation-Context.md`, `VISION/09-Roadmap/`.
- Uses the `gh-issue-tdd` and `gh-review-commit` codex skills (from `~/.codex/skills` and the `github@openai-curated` plugin).
- Drives GitHub via `gh` (issues, PRs, review threads, merges, labels).
- Drives git locally (fetch, branch, checkout, commit, push, delete).
