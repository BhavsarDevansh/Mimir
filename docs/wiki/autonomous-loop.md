# Autonomous Development Loop

## What it is

Mimir can drive its own development. A small script, `scripts/autonomous-loop.sh`, wakes up on a configurable cadence (default every two hours, or every 30 minutes with `MIMIR_AUTONOMOUS_INTERVAL=1800`) and asks one question: "what should I work on next?" It then either finishes off an open pull request or picks up an issue from the GitHub backlog — maintenance, DRY, bug fixes, refactors, robustness, performance, security, documentation, testing, and build work first, then feature-development tickets at lower priority — writing code, running tests, updating docs, and publishing its work as a draft pull request, all on its own.

## How it works

Each cycle the loop checks which git branch you are on:

- **On a feature branch with a draft PR** — it reviews its own diff against `main`, fixes every issue it finds (no matter how small), pushes, and marks the PR ready for review.
- **On a feature branch with a ready PR** — it looks for outstanding review comments (for example from CodeRabbit). If there are any, it fixes them and pushes. If everything is resolved — or CodeRabbit's latest review reports a documented rate-limit marker — and GitHub reports the PR merge state as clean, it merges the PR into `main`, switches the local branch to `main` and pulls the latest changes.
- **On a feature branch with no PR** — if the working tree is clean it switches back to `main` and pulls. If the local branch is not fully merged, or if there is uncommitted work, it leaves things alone for you to handle.
- **On `main`** — it picks the next unblocked issue, checking the roadmap and vision docs to make sure it is the right one. Code-quality work always comes first; feature tickets are only picked up when no quality work is waiting. A feature ticket gets an in-depth requirements analysis first, and is only implemented once its direction is clear. A typical ticket takes a few 30-minute cycles: implement and open the draft PR, self-review and mark ready, address any review comments, then merge and move on to the next ticket.

## Keeping the issue tracker healthy

While working, the loop also keeps GitHub itself in good shape: it refreshes stale issue descriptions with the current state of the codebase, files new issues (using the existing labels) for bugs, DRY violations, misplaced code, or performance problems it finds outside the change it is working on, and keeps `README.md`, `docs/wiki/what-works-now.md` and `AGENTS.md` accurate as part of every documentation update.

It also keeps the repo's own markdown consistent: a review-time regression guard (`scripts/tests/md-reflow_test.sh`) re-checks every `.md` file against the single-line prose rule in `AGENTS.md`, so formatting drift is caught before it lands instead of accumulating.

## When it needs your help

If the loop starts an issue but realises it needs a clarification or a decision that only a human can make, it will not guess. Instead it adds the `help-wanted` label to the issue and posts its questions as a comment on the issue. This is how feature tickets are handled by default: before implementing a feature the loop analyses the requirements in depth, and if anything is missing or ambiguous — scope, design, acceptance criteria, priorities — it asks you and moves on to other work until you reply. When you answer the questions (and remove the `help-wanted` label, if you like), the loop notices on a future run, reads your answers as context, and goes ahead with the implementation. If it still has further questions later, it posts them and puts the `help-wanted` label back, so nothing is ever implemented on guesswork. In the meantime it works on other tickets, so progress never stalls.

## Use cases

- **Unattended progress** — leave it running and the backlog shrinks over time.
- **PR hygiene** — draft PRs get self-reviewed before going ready; ready PRs get review comments addressed and then merged.
- **Conversation trail** — questions and answers accumulate in the GitHub issues, building up context for future work.
- **Issue hygiene** — stale issues get refreshed and newly discovered problems are captured before they are forgotten.

## Best practices

- Run it via the provided systemd timer so it survives reboots, or leave a `setsid`-detached run going for a 30-minute cadence.
- Keep `main` clean; the loop will not start new work on a dirty `main`.
- Review merged PRs occasionally — the loop follows `AGENTS.md` but a human spot-check keeps it honest.
- Watch `MIMIR_AUTONOMOUS_LOG` or `${XDG_STATE_HOME:-$HOME/.local/state}/mimir/autonomous.log` for a timestamped record of the agent's conversations. The loop logs what the agent says (and fatal codex errors) in the standard log colour, and shows the exact prompt text it sent to codex in green. It does not log the raw transcripts of files the agent read or commands it ran — those stay in codex's session files if you ever need them.

## How to start it

```bash
scripts/autonomous-loop.sh --once      # try a single run
scripts/autonomous-loop.sh --dry-run   # preview what it would do
MIMIR_AUTONOMOUS_INTERVAL=1800 scripts/autonomous-loop.sh   # 30-minute cadence
```

The loop delegates all coding to `codex exec`. If your default codex account is unavailable, point the loop at another backend with `MIMIR_AUTONOMOUS_CODEX_ARGS` — for example `MIMIR_AUTONOMOUS_CODEX_ARGS="--oss -m deepseek-v4-flash:cloud --yolo --config model_reasoning_effort=max --config model_context_window=1000000"` runs every delegated session on the OSS deepseek model with full access and a 1M context window. The provided systemd service already ships with this configuration.

For the every-two-hours schedule, install the systemd timer described in `docs/autonomous-loop.md`. To stop a run, kill the `autonomous-loop.sh` process (and any `codex exec` children) or run `systemctl --user disable --now mimir-autonomous.timer`.
