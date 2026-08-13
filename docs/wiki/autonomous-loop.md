# Autonomous Development Loop

## What it is

Mimir can drive its own development. A small script, `scripts/autonomous-loop.sh`, wakes up on a configurable cadence (default every two hours, or every 30 minutes with `MIMIR_AUTONOMOUS_INTERVAL=1800`) and asks one question: "what should I work on next?" It then either finishes off an open pull request or picks up a new issue from the GitHub backlog — writing code, running tests, updating docs, and publishing its work as a draft pull request, all on its own.

## How it works

Each cycle the loop checks which git branch you are on:

- **On a feature branch with a draft PR** — it reviews its own diff against `main`, fixes every issue it finds (no matter how small), pushes, and marks the PR ready for review.
- **On a feature branch with a ready PR** — it looks for outstanding review comments (for example from CodeRabbit). If there are any, it fixes them and pushes. If everything is resolved — or CodeRabbit skipped its review because it is out of reviews for a while — and GitHub reports the PR merge state as clean, it merges the PR into `main`, switches the local branch to `main` and pulls the latest changes.
- **On a feature branch with no PR** — if the working tree is clean it switches back to `main` and pulls. If there is uncommitted work it leaves things alone for you to handle.
- **On `main`** — it picks the next unblocked issue (checking the roadmap and vision docs to make sure it is the right one) and implements it as a draft PR, updating docs and running tests as it goes. A typical ticket takes a few 30-minute cycles: implement and open the draft PR, self-review and mark ready, address any review comments, then merge and move on to the next ticket.

## Keeping the issue tracker healthy

While working, the loop also keeps GitHub itself in good shape: it refreshes stale issue descriptions with the current state of the codebase, files new issues (using the existing labels) for bugs, DRY violations, misplaced code, or performance problems it finds outside the change it is working on, and keeps `README.md`, `docs/wiki/what-works-now.md` and `AGENTS.md` accurate as part of every documentation update.

## When it needs your help

If the loop starts an issue but realises it needs a clarification or a decision that only a human can make, it will not guess. Instead it: adds the `help-wanted` label to the issue and posts its questions as a comment on the issue. When you answer those questions in a follow-up comment, the loop notices on a future run and goes ahead with the implementation. In the meantime it moves on to another ticket that is ready, so progress never stalls.

## Use cases

- **Unattended progress** — leave it running and the backlog shrinks over time.
- **PR hygiene** — draft PRs get self-reviewed before going ready; ready PRs get review comments addressed and then merged.
- **Conversation trail** — questions and answers accumulate in the GitHub issues, building up context for future work.
- **Issue hygiene** — stale issues get refreshed and newly discovered problems are captured before they are forgotten.

## Best practices

- Run it via the provided systemd timer so it survives reboots, or leave a `setsid`-detached run going for a 30-minute cadence.
- Keep `main` clean; the loop will not start new work on a dirty `main`.
- Review merged PRs occasionally — the loop follows `AGENTS.md` but a human spot-check keeps it honest.
- Watch `~/.local/state/mimir/autonomous.log` for a full timestamped audit trail.

## How to start it

```bash
scripts/autonomous-loop.sh --once      # try a single run
scripts/autonomous-loop.sh --dry-run   # preview what it would do
MIMIR_AUTONOMOUS_INTERVAL=1800 scripts/autonomous-loop.sh   # 30-minute cadence
```

For the every-two-hours schedule, install the systemd timer described in `docs/autonomous-loop.md`. To stop a run, kill the `autonomous-loop.sh` process (and any `codex exec` children) or run `systemctl --user disable --now mimir-autonomous.timer`.
