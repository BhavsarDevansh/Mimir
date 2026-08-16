#!/usr/bin/env bash
set -euo pipefail

# Tests for scripts/autonomous-loop.sh's conversation-only logging.
#
# The loop log must contain the agent's messages (and fatal codex errors) but
# never the raw codex transcript: file contents, shell commands and their
# output, patches, web searches, or reasoning.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_FILE="$(mktemp)"

fail() { echo "FAIL: $*" >&2; exit 1; }

# Point the loop's log at a temp file before sourcing so emit() writes there.
export MIMIR_AUTONOMOUS_LOG="$LOG_FILE"
PROMPT_FILE="$(mktemp)"
trap 'rm -f "$LOG_FILE" "$PROMPT_FILE"' EXIT

# shellcheck source=../autonomous-loop.sh
source "$SCRIPT_DIR/autonomous-loop.sh"

# A realistic `codex exec --json` stream: agent messages interleaved with
# reasoning, a shell command (with output), a file change, and turn bookends.
FIXTURE='
{"type":"thread.started","thread_id":"019ffd71-9cee-7503-9c58-c219bd6e6367"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"I found issue #42; the duplicate upsert is the cause."}}
{"type":"item.completed","item":{"id":"item_2","type":"reasoning","text":"super-secret chain-of-thought"}}
{"type":"item.started","item":{"id":"item_3","type":"command_execution","command":"cat /etc/shadow","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_3","type":"command_execution","command":"cat /etc/shadow","aggregated_output":"root:$6$hugehash:1:0:99999:7:::","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"item_4","type":"file_change","changes":[{"path":"mimir-core/src/lib.rs","kind":"update"}],"status":"completed"}}
{"type":"item.completed","item":{"id":"item_5","type":"agent_message","text":"Implemented the fix.\nTests pass.\nCloses #42."}}
{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0}}
'

run_filter() { printf '%s\n' "$1" | log_agent_stream >/dev/null 2>&1; }
logged() { grep -qF "$1" "$LOG_FILE"; }

# 1. Agent messages are logged.
run_filter "$FIXTURE"
logged "I found issue #42" || fail "first agent message missing from log"
logged "Closes #42" || fail "final agent message missing from log"

# 2. Multi-line agent messages keep every line.
logged "Tests pass" || fail "multi-line agent message truncated"

# 3. Commands, their output, reasoning, and file changes never reach the log.
for forbidden in "/etc/shadow" "hugehash" "chain-of-thought" "mimir-core/src/lib.rs" "command_execution"; do
    logged "$forbidden" && fail "log contains forbidden transcript content: $forbidden"
done

# 4. Stray non-JSON stderr noise (e.g. "Reading prompt from stdin...") is
#    skipped without breaking the stream or losing later messages.
printf 'Reading prompt from stdin...\n%s\n' "$FIXTURE" | log_agent_stream >/dev/null 2>&1
logged "Closes #42" || fail "stream with non-JSON noise lost agent messages"

# 5. Fatal codex errors stay visible in the log.
printf '%s\n' '{"type":"error","message":"You have hit your usage limit."}' \
    '{"type":"turn.failed","error":{"message":"turn failed"}}' \
    | log_agent_stream >/dev/null 2>&1
logged "usage limit" || fail "codex error not logged"
logged "turn failed" || fail "turn.failed error not logged"

# 6. Prompts sent to codex are logged verbatim with a [PROMPT] marker.
printf 'Implement issue #42.\n\nFollow AGENTS.md strictly.\n' > "$PROMPT_FILE"
log_prompt "$PROMPT_FILE" >/dev/null 2>&1
logged "Implement issue #42" || fail "prompt line 1 not logged"
logged "Follow AGENTS.md strictly" || fail "prompt line 3 not logged"
grep -q "\[PROMPT\]" "$LOG_FILE" || fail "prompt lines lack the [PROMPT] marker"

# 7. Issue classification: feature tickets (feature label, or Implement /
#    Future: titles) are never quality work, but stay candidates at lower
#    priority than quality issues.
is_feature_issue "Implement a widget" "bug" \
    || fail "Implement: title should classify as feature work"
is_feature_issue "Future: connectors v2" "" \
    || fail "Future: title should classify as feature work"
is_feature_issue "Widget builder" "feature" \
    || fail "feature label should classify as feature work"
is_feature_issue "Fix crash" "bug" \
    && fail "bug-labelled issue classified as feature work"

# 8. Quality classification still excludes feature work, with labels taking
#    precedence over title heuristics exactly as before.
is_quality_issue "Fix crash" "bug" || fail "bug label should be quality work"
is_quality_issue "DRY: dedupe config" "" || fail "DRY: title should be quality work"
is_quality_issue "Implement tests" "testing" \
    || fail "quality label should win over an Implement: title"
is_quality_issue "Implement a widget" "feature" \
    && fail "feature label must not classify as quality work"
is_quality_issue "Implement a widget" "bug,feature" \
    && fail "feature label must win over quality labels"
is_quality_issue "Implement a widget" "" \
    && fail "unlabelled feature title classified as quality work"
is_quality_issue "Widget builder" "" \
    && fail "unclassifiable issue classified as quality work"

echo "all autonomous-loop tests passed"
