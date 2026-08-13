#!/usr/bin/env bash
set -euo pipefail

# Mimir autonomous development loop.
#
# Runs a single iteration (or loops every MIMIR_AUTONOMOUS_INTERVAL seconds) that
# drives the GitHub PR lifecycle and issue implementation for the Mimir repo:
#
#   * On a feature branch:
#       - Draft PR  -> run a PR-style code review against main, address every
#                      valid finding, commit, push, then mark the PR ready.
#       - Ready PR  -> if there are unresolved review comments, address them via
#                      the gh-review-commit skill and push; otherwise merge the
#                      PR into main and delete the branch.
#       - No open PR -> if the tree is clean, switch to main and pull.
#
#   * On main:
#       - Pick the next unblocked code-quality issue (maintenance, DRY, bug
#         fixes, refactors, robustness, performance, security, documentation,
#         testing, build; feature development is excluded), then implement it
#         via the gh-issue-tdd skill and publish as a draft PR, or post
#         clarifying questions with the help-wanted label.
#
# The hard work (review, implementation, addressing comments) is delegated to
# `codex exec`, which loads the project's AGENTS.md and the gh-issue-tdd /
# gh-review-commit skills. This script only performs deterministic git/gh
# orchestration so that control flow never depends on LLM prompt engineering.
#
# Usage:
#   scripts/autonomous-loop.sh                # loop forever, 2h between runs
#   scripts/autonomous-loop.sh --once         # single iteration then exit
#   scripts/autonomous-loop.sh --dry-run      # print actions, do not invoke codex
#
# Environment:
#   MIMIR_AUTONOMOUS_INTERVAL   seconds between iterations (default 7200)
#   MIMIR_AUTONOMOUS_SANDBOX     codex sandbox mode (default danger-full-access)
#   MIMIR_AUTONOMOUS_MODEL       override codex model (optional)
#   MIMIR_AUTONOMOUS_BYPASS      1 => --dangerously-bypass-approvals-and-sandbox
#   MIMIR_AUTONOMOUS_LOG         log file (default ~/.local/state/mimir/autonomous.log)
#   MIMIR_AUTONOMOUS_DRY_RUN     1 => dry run (same as --dry-run)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

INTERVAL="${MIMIR_AUTONOMOUS_INTERVAL:-7200}"
SANDBOX="${MIMIR_AUTONOMOUS_SANDBOX:-danger-full-access}"
MODEL="${MIMIR_AUTONOMOUS_MODEL:-}"
BYPASS="${MIMIR_AUTONOMOUS_BYPASS:-0}"
DRY_RUN=0
ONCE=0
LOG_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/mimir"
LOG_FILE="${MIMIR_AUTONOMOUS_LOG:-$LOG_DIR/autonomous.log}"
QUESTION_MARKER_PREFIX="<!-- mimir-autonomous-question:"
# Issue categories the loop is allowed to implement. Feature development is
# excluded outright: the loop exists to pay down maintenance, DRY, bug,
# robustness, performance, security, documentation, testing and build debt.
QUALITY_LABELS="bug,refactor,maintenance,performance,security,documentation,testing,build"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
log()  { printf '%s [INFO] %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*" | tee -a "$LOG_FILE" >&2; }
warn() { printf '%s [WARN] %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*" | tee -a "$LOG_FILE" >&2; }
err()  { printf '%s [ERROR] %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*" | tee -a "$LOG_FILE" >&2; }

require() { command -v "$1" >/dev/null 2>&1 || { err "missing required tool: $1"; exit 1; }; }

# Run a codex exec session with a prompt read from stdin.
run_codex() {
    local prompt_file="$1"
    local -a args=(exec --cd "$PROJECT_ROOT" --skip-git-repo-check)
    if [[ "$BYPASS" == "1" ]]; then
        args+=(--dangerously-bypass-approvals-and-sandbox)
    else
        args+=(-s "$SANDBOX")
    fi
    [[ -n "$MODEL" ]] && args+=(-m "$MODEL")
    args+=(-o "$RUN_DIR/last-message.txt")
    if [[ "$DRY_RUN" == "1" ]]; then
        log "DRY RUN: would invoke: codex ${args[*]} - < $prompt_file"
        log "DRY RUN: prompt contents:"
        sed 's/^/    | /' "$prompt_file" >&2
        return 0
    fi
    log "invoking codex exec (prompt in $prompt_file)"
    if codex "${args[@]}" - < "$prompt_file" 2>&1 | tee -a "$LOG_FILE"; then
        return 0
    else
        local rc=$?
        err "codex exec failed with exit code $rc"
        return "$rc"
    fi
}

# Resolve owner/repo from the git remote via gh.
resolve_repo() {
    gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null \
        || { err "unable to resolve repo (is gh authenticated?)"; exit 1; }
}

current_branch() { git -C "$PROJECT_ROOT" rev-parse --abbrev-ref HEAD; }

working_tree_clean() { [[ -z "$(git -C "$PROJECT_ROOT" status --porcelain)" ]]; }

# ---------------------------------------------------------------------------
# PR inspection
# ---------------------------------------------------------------------------

# Print the open PR number for the given head branch, or empty if none.
pr_for_branch() {
    local branch="$1"
    gh pr list --head "$branch" --state open --json number --jq '.[0].number // empty' 2>/dev/null || true
}

pr_is_draft() {
    local num="$1"
    [[ "$(gh pr view "$num" --json isDraft --jq '.isDraft' 2>/dev/null)" == "true" ]]
}

# Count unresolved, non-outdated review threads on a PR via GraphQL.
count_unresolved_threads() {
    local owner_repo="$1" num="$2"
    local owner repo
    owner="${owner_repo%%/*}"
    repo="${owner_repo#*/}"
    local cursor=""
    local total=0
    while true; do
        local data
        local -a fields=(-F owner="$owner" -F repo="$repo" -F number="$num")
        [[ -n "$cursor" ]] && fields+=(-F cursor="$cursor")
        data=$(gh api graphql -f query="
        query(\$owner:String!,\$repo:String!,\$number:Int!,\$cursor:String){
          repository(owner:\$owner,name:\$repo){
            pullRequest(number:\$number){
              reviewThreads(first:100,after:\$cursor){
                pageInfo{hasNextPage endCursor}
                nodes{ isResolved isOutdated }
              }
            }
          }
        }" "${fields[@]}" 2>/dev/null) || return 1
        [[ -z "$data" ]] && return 1
        local count
        count=$(printf '%s' "$data" | jq '[.data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved == false and .isOutdated == false)] | length' 2>/dev/null) || return 1
        total=$((total + count))
        local has_next
        has_next=$(printf '%s' "$data" | jq '.data.repository.pullRequest.reviewThreads.pageInfo.hasNextPage' 2>/dev/null) || return 1
        [[ "$has_next" == "true" ]] || break
        cursor=$(printf '%s' "$data" | jq -r '.data.repository.pullRequest.reviewThreads.pageInfo.endCursor' 2>/dev/null) || return 1
    done
    echo "$total"
}

# True if any review on the PR is in CHANGES_REQUESTED state.
has_changes_requested() {
    local num="$1"
    local states
    states=$(gh pr view "$num" --json reviews --jq '.reviews[].state' 2>/dev/null || true)
    grep -q '^CHANGES_REQUESTED$' <<<"$states"
}

# True if CodeRabbit skipped its review (e.g. it is out of reviews for a
# while). The absence of CodeRabbit threads must then not block merging.
coderabbit_skipped() {
    local num="$1"
    local bodies
    bodies=$(gh pr view "$num" --json reviews \
        --jq '.reviews[] | select(.author.login == "coderabbitai") | .body // ""' 2>/dev/null || true)
    grep -qiE 'out of reviews|skipping review|review skipped|skipped.*review|no reviews? (left|remaining)' <<<"$bodies"
}

pr_mergeable() {
    local num="$1"
    local status
    status=$(gh pr view "$num" --json mergeStateStatus --jq '.mergeStateStatus' 2>/dev/null || echo "UNKNOWN")
    [[ "$status" == "CLEAN" ]]
}

# ---------------------------------------------------------------------------
# Issue selection
# ---------------------------------------------------------------------------

# For a help-wanted issue, return 0 (answered) if the repo owner has commented
# after the agent's most recent question-marker comment, 1 otherwise.
issue_questions_answered() {
    local owner_repo="$1" num="$2"
    local owner repo
    owner="${owner_repo%%/*}"
    repo="${owner_repo#*/}"
    local comments marker_time marker_author
    comments=$(gh api "repos/$owner/$repo/issues/$num/comments" --jq '.[] | "\(.created_at)\t\(.user.login)\t\((.body|gsub("\n";" ")))"' 2>/dev/null || true)
    [[ -z "$comments" ]] && return 1
    # Find the latest comment containing the question marker.
    local marker_line
    marker_line=$(printf '%s\n' "$comments" | grep -F "$QUESTION_MARKER_PREFIX" | tail -n1 || true)
    [[ -z "$marker_line" ]] && return 1
    marker_time="${marker_line%%	*}"
    local rest="${marker_line#*	}"
    marker_author="${rest%%	*}"
    # Look for any later comment by a different author.
    if printf '%s\n' "$comments" | awk -F'\t' -v t="$marker_time" -v a="$marker_author" \
        '$1 > t && $2 != a { found=1 } END { exit !found }'; then
        return 0
    fi
    return 1
}

# Return 0 if the issue is code-quality work the loop may implement, 1 if it is
# feature development (or unclassifiable). Quality is decided by label first,
# then by title heuristics; the `feature` label and obvious feature titles are
# hard exclusions.
is_quality_issue() {
    local title="$1" labels="$2"
    local label_set=",$labels,"
    [[ "$label_set" == *",feature,"* ]] && return 1
    local lbl q
    IFS=',' read -ra q <<<"$QUALITY_LABELS"
    for lbl in "${q[@]}"; do
        [[ "$label_set" == *",$lbl,"* ]] && return 0
    done
    if [[ "$title" =~ ^(DRY|Robustness|Refactor|Maintenance|Bug|Fix|Flaky|Code[[:space:]]quality|Spec[[:space:]]drift|E2E[[:space:]]test|Security|Perf|Cleanup|Docs?)[:[:space:]] ]]; then
        return 0
    fi
    if [[ "$title" =~ ^(Implement|Future:) ]]; then
        return 1
    fi
    return 1
}

# Print candidate issue numbers (one per line), sorted by current-phase priority
# then number, excluding blocked, unanswered help-wanted, and non-quality
# (feature-development) issues.
candidate_issues() {
    local owner_repo="$1"
    local owner repo
    owner="${owner_repo%%/*}"
    repo="${owner_repo#*/}"
    local all current_phase
    current_phase="phase-3" # active phase per VISION/09-Roadmap
    all=$(gh issue list --state open --limit 200 --json number,title,labels --jq '.[] | "\(.number)\t\(.title)\t\([.labels[].name]|join(","))"' 2>/dev/null || true)
    [[ -z "$all" ]] && return 0
    local in_phase="" others="" num title labels
    while IFS=$'\t' read -r num title labels; do
        [[ -z "$num" ]] && continue
        local label_set=",$labels,"
        if [[ "$label_set" == *",blocked,"* ]]; then continue; fi
        if [[ "$label_set" == *",help-wanted,"* ]]; then
            if ! issue_questions_answered "$owner_repo" "$num"; then
                continue
            fi
        fi
        if ! is_quality_issue "$title" "$labels"; then continue; fi
        if [[ "$label_set" == *",$current_phase,"* ]]; then
            in_phase+="$num"$'\n'
        else
            others+="$num"$'\n'
        fi
    done <<<"$all"
    printf '%s' "$in_phase" | sort -n
    printf '%s' "$others" | sort -n
}

issue_title() {
    gh issue view "$1" --json title --jq '.title' 2>/dev/null || echo "(no title)"
}

# ---------------------------------------------------------------------------
# Branch handlers
# ---------------------------------------------------------------------------

handle_draft_pr() {
    local num="$1" branch="$2"
    log "Draft PR #$num on branch '$branch': running PR-style review + fixes."
    local prompt="$RUN_DIR/draft-review.txt"
    cat >"$prompt" <<'PROMPT'
You are the Mimir autonomous code-reviewer. The current branch has a DRAFT pull request.

Perform a thorough PR-style code review of this branch against `main`:
  1. Run `git diff main...HEAD` and read every changed file.
  2. Apply the full code-review checklist from the project's AGENTS.md
     (correctness, performance, security, doc comments, DRY, modern design,
     VISION compliance, type consistency, public API surface).
  3. Address EVERY valid finding, no matter how small — fix it in the code.
  4. Run `cargo test`, `cargo fmt`, `cargo clippy` and resolve any failures.
  5. Commit the fixes with a clear descriptive message and `git push` to origin.

Rules:
  - If there are genuinely zero findings, make NO changes and do NOT commit.
  - Do NOT mark the PR ready for review — the orchestrator does that.
  - Do NOT co-author or co-sign commits.
  - Follow AGENTS.md strictly. Do not fix unrelated pre-existing failures;
    if you find unrelated quality problems (misplaced code, DRY violations,
    bugs, performance issues), file a GitHub issue using only existing labels.
  - Update README.md, docs/wiki/what-works-now.md and AGENTS.md only if this
    PR's changes make them inaccurate.
PROMPT
    run_codex "$prompt" || return 1
    log "marking PR #$num ready for review"
    [[ "$DRY_RUN" == "1" ]] || gh pr ready "$num" 2>&1 | tee -a "$LOG_FILE" >&2
}

handle_ready_pr() {
    local num="$1" branch="$2" owner_repo="$3"
    log "Ready PR #$num on branch '$branch': checking for unresolved review comments."
    local unresolved
    unresolved=$(count_unresolved_threads "$owner_repo" "$num") || { warn "could not inspect review threads for PR #$num; deferring."; return 0; }
    local changes=0
    has_changes_requested "$num" && changes=1
    local skipped=0
    coderabbit_skipped "$num" && skipped=1
    log "unresolved threads: ${unresolved:-0}; changes_requested: $changes; coderabbit_skipped: $skipped"
    if [[ "${unresolved:-0}" -gt 0 || "$changes" -eq 1 ]]; then
        local prompt="$RUN_DIR/ready-review.txt"
        cat >"$prompt" <<PROMPT
You are the Mimir autonomous PR-maintainer. PR #$num on the current branch has unresolved review comments (e.g. CodeRabbit findings).

Use the \`gh-review-commit\` skill to:
  1. Fetch all unresolved review threads for PR #$num.
  2. Address every actionable finding with minimal, traceable code changes.
  3. Run \`cargo test\`, \`cargo fmt\`, \`cargo clippy\` and resolve failures.
  4. Commit the fixes and \`git push\` to origin so the PR updates.

Rules:
  - Do NOT resolve review threads on GitHub or submit reviews yourself.
  - Do NOT co-author or co-sign commits.
  - Only modify files referenced by review comments; avoid unrelated refactoring.
  - Follow AGENTS.md strictly.
PROMPT
        run_codex "$prompt" || return 1
        log "addressed review comments for PR #$num; will re-check next iteration."
        return 0
    fi

    if [[ "$skipped" == "1" ]]; then
        log "CodeRabbit skipped its review for PR #$num and no open comments remain; merging into main."
    else
        log "no open review comments on PR #$num; merging into main."
    fi
    if ! pr_mergeable "$num"; then
        warn "PR #$num is not in a mergeable state; leaving for next iteration."
        return 0
    fi
    if [[ "$DRY_RUN" == "1" ]]; then
        log "DRY RUN: would merge PR #$num and delete branch '$branch'."
        return 0
    fi
    gh pr merge "$num" --merge --delete-branch 2>&1 | tee -a "$LOG_FILE" >&2 || { err "merge of PR #$num failed"; return 1; }
    git -C "$PROJECT_ROOT" checkout main 2>&1 | tee -a "$LOG_FILE" >&2
    git -C "$PROJECT_ROOT" pull --ff-only 2>&1 | tee -a "$LOG_FILE" >&2
    git -C "$PROJECT_ROOT" branch -D "$branch" 2>/dev/null | tee -a "$LOG_FILE" >&2 || true
    log "merged PR #$num and deleted branch '$branch'."
}

handle_feature_branch() {
    local branch="$1" owner_repo="$2"
    local num
    num=$(pr_for_branch "$branch")
    if [[ -z "$num" ]]; then
        log "No open PR for branch '$branch'."
        if ! working_tree_clean; then
            warn "uncommitted work on branch '$branch' with no open PR; leaving for user."
            return 0
        fi
        log "tree clean; switching to main and pulling."
        [[ "$DRY_RUN" == "1" ]] || { git -C "$PROJECT_ROOT" checkout main 2>&1 | tee -a "$LOG_FILE" >&2; \
            git -C "$PROJECT_ROOT" pull --ff-only 2>&1 | tee -a "$LOG_FILE" >&2; }
        [[ "$DRY_RUN" == "1" ]] || git -C "$PROJECT_ROOT" branch -D "$branch" 2>/dev/null || true
        return 0
    fi
    if pr_is_draft "$num"; then
        handle_draft_pr "$num" "$branch"
    else
        handle_ready_pr "$num" "$branch" "$owner_repo"
    fi
}

handle_main_branch() {
    local owner_repo="$1"
    if ! working_tree_clean; then
        warn "main has uncommitted work; not starting new issue work."
        return 0
    fi
    log "On main: selecting next unblocked issue."
    local candidates cand_list=""
    candidates=$(candidate_issues "$owner_repo")
    if [[ -z "$candidates" ]]; then
        log "No unblocked candidate issues found; nothing to do."
        return 0
    fi
    local num title count=0
    while IFS= read -r num; do
        [[ -z "$num" ]] && continue
        title=$(issue_title "$num")
        cand_list+="#$num: $title"$'\n'
        count=$((count + 1))
        [[ "$count" -ge 5 ]] && break
    done <<<"$candidates"
    log "candidate issues:"$'\n'"$cand_list"
    local prompt="$RUN_DIR/implement-issue.txt"
    cat >"$prompt" <<PROMPT
You are the Mimir autonomous developer. Select the next unblocked issue to implement from this candidate list (number: title):

$cand_list

Process:
  1. All candidates are code-quality issues. Read \`VISION/09-Roadmap/*.md\` and \`Mimir-Implementation-Context.md\` to confirm which candidate is the correct next issue per the roadmap (earliest active phase, lowest number, not blocked, dependencies satisfied). NEVER implement feature development (new capabilities, new tools, new phases) — if every candidate is feature work or blocked, do nothing and report that.
  2. Fetch the FULL chosen issue before working on it: \`gh issue view N --comments\` plus \`gh api repos/<owner>/<repo>/issues/N\` for every field (title, body, labels, state, milestone, assignees) and every comment. Also read the VISION docs for the issue's subsystem.
  3. The issue spec may be outdated vs. the current codebase. Verify every claim against the code and implement against CURRENT reality, not stale wording. If the issue body is materially outdated, update it (\`gh issue edit N --body-file\`) or post a follow-up comment so it reflects the latest context.
  4. If the chosen issue has the \`help-wanted\` label and its clarifying questions have been answered by the repo owner in the comments, proceed to implement it.
  5. If the issue absolutely cannot be worked on without human intervention, do NOT guess:
       - Add the \`help-wanted\` label: \`gh issue edit <N> --add-label help-wanted\`.
       - Post a comment detailing exactly what is required from a human (files, decisions, acceptance criteria), ending with the marker line (replace N with the issue number): \`<!-- mimir-autonomous-question:N -->\`.
       - Then stop and do not create a branch; the orchestrator will pick a different ticket.
  6. Otherwise use the \`gh-issue-tdd\` skill to implement the chosen issue end-to-end:
       - Create a branch named \`feat/<task-name>\` or \`bugfix/<description>\` (lowercase, hyphenated, descriptive).
       - Follow Test-Driven Development (Red-Green-Refactor).
       - Update \`docs/\` (technical) and \`docs/wiki/\` (user-facing) documentation, plus README.md, docs/wiki/what-works-now.md and AGENTS.md as needed.
       - Run a code review against every changed file and action ALL findings.
       - Run \`cargo test\`, \`cargo fmt\`, \`cargo clippy\` and resolve failures.
       - Bump workspace versions and CHANGELOG per AGENTS.md.
       - Push the branch and publish as a DRAFT pull request linking the issue with a closing statement (e.g. \`Closes #N\`).
  7. Issue and codebase hygiene while working:
       - If you encounter other open issues whose contents are stale relative to the codebase, update them (body edit or comment) with the latest context so the tracker stays accurate.
       - If an issue you inspect is missing its appropriate quality labels (bug, refactor, maintenance, performance, security, documentation, testing, build) or a feature request lacks the \`feature\` label, add the missing labels via \`gh issue edit\` so the tracker stays accurate.
       - If you find problems OUTSIDE the current change (misplaced code, DRY violations, performance issues, bugs, security concerns), create new GitHub issues with clear, self-contained context, using ONLY the repo's existing labels (bug, performance, refactor, maintenance, documentation, build, testing, security, feature, core-agent, knowledge-graph, connectors, cli, chat, memory, tools, phase-2, phase-3, etc).
       - Take small opportunities to improve code quality in files you already touch, without expanding scope.

Rules:
  - Do NOT co-author or co-sign commits.
  - Follow AGENTS.md strictly, including the no-unsafe policy, semantic versioning, and single-line markdown prose.
  - Only implement ONE issue per run.
  - Quality issues only: maintenance, DRY, bug fixes, refactors, robustness, performance, security, documentation, testing, build. Feature development is never in scope.
PROMPT
    run_codex "$prompt" || return 1
}

# ---------------------------------------------------------------------------
# Single iteration
# ---------------------------------------------------------------------------

run_iteration() {
    mkdir -p "$LOG_DIR" "$RUN_DIR"
    local owner_repo
    owner_repo=$(resolve_repo)
    log "=== iteration start (repo: $owner_repo) ==="
    git -C "$PROJECT_ROOT" fetch origin --prune 2>&1 | tee -a "$LOG_FILE" >&2 || warn "git fetch failed"

    local branch
    branch=$(current_branch)
    log "current branch: $branch"

    if [[ "$branch" == "main" ]]; then
        handle_main_branch "$owner_repo"
    elif [[ "$branch" == "HEAD" ]]; then
        warn "detached HEAD; switching to main."
        [[ "$DRY_RUN" == "1" ]] || { git -C "$PROJECT_ROOT" checkout main 2>&1 | tee -a "$LOG_FILE" >&2; \
            git -C "$PROJECT_ROOT" pull --ff-only 2>&1 | tee -a "$LOG_FILE" >&2; }
    else
        handle_feature_branch "$branch" "$owner_repo"
    fi
    log "=== iteration end ==="
}

# ---------------------------------------------------------------------------
# Args & main
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --once) ONCE=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help)
            sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) err "unknown arg: $1"; exit 2 ;;
    esac
done
[[ "${MIMIR_AUTONOMOUS_DRY_RUN:-0}" == "1" ]] && DRY_RUN=1

require git
require gh
require codex
require jq

if ! [[ "$INTERVAL" =~ ^[0-9]+$ ]] || [[ "$INTERVAL" -le 0 ]]; then
    err "MIMIR_AUTONOMOUS_INTERVAL must be a positive integer (got '$INTERVAL')"
    exit 2
fi

RUN_DIR="$(mktemp -d -t mimir-autonomous.XXXXXX)"
trap 'rm -rf "$RUN_DIR"' EXIT

mkdir -p "$LOG_DIR"
touch "$LOG_FILE"

# Prevent overlapping iterations (e.g. timer fired while previous still running).
exec 9>"$LOG_DIR/autonomous.lock"
if ! flock -n 9; then
    warn "another iteration is already running; exiting."
    exit 0
fi

if [[ "$ONCE" == "1" ]]; then
    run_iteration
    exit 0
fi

log "autonomous loop started (interval: ${INTERVAL}s, sandbox: $SANDBOX)"
while true; do
    run_iteration || warn "iteration exited with errors; continuing."
    log "sleeping ${INTERVAL}s until next iteration."
    sleep "$INTERVAL"
done
