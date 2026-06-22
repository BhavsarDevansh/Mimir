# Code review — `tests-and-benchmarks` change set

Review run per `AGENTS.md` (mandatory, non-negotiable) over every file touched
in this change set, after documentation was updated and before commit.

## Scope

Touched files:
- `mimir-api-types/src/lib.rs`
- `mimir-client/src/lib.rs`
- `mimir-core/src/job_queue.rs`
- `mimir-core/src/tools/{output,permission,error}.rs`
- `mimir-knowledge/src/models/enums.rs`
- `mimir-knowledge/src/retrieval/types.rs`
- `mimir-knowledge/src/inference/rules/transitivity.rs`
- `mimir-knowledge/src/models/{entity_date,memory}.rs`
- `mimir-server/src/error.rs`
- `mimir/src/kb.rs`
- new bench files: `mimir-api-types/benches/wire_types.rs`,
  `mimir-core/benches/pure_helpers.rs`,
  `mimir-knowledge/benches/pure_helpers.rs`
- `Cargo.toml` (root + crate manifests: bench registration, dev-deps, version)

## Findings (new test/bench code)

| Dimension | Finding | Severity | Action |
|-----------|---------|----------|--------|
| Code quality | `roundtrip_tests!` macro left `json` unused when `sparse_skips` empty | low | Fixed: added `let _ = &json;` with explanatory comment |
| Code quality | New benches used deprecated `criterion::black_box` | low | Fixed: switched to `std::hint::black_box` |
| Performance | `DailySchedule::parse` bench dropped `Result` (must_use) | low | Fixed: bind + `unwrap()` (inputs are valid) |
| Doc comments | Bench files have module doc comments; test fns intentionally omit docs (idiomatic) | n/a | No action |
| DRY | `mimir-client` test module centralises `sample_fact_row()` helper | n/a | No action (already DRY) |
| Type consistency | All new types use existing workspace types; no new public API | n/a | No action |
| Security | `mimir-server::error` tests assert internal details are masked | n/a | Locked in as regression guard |
| Guideline compliance | No `unsafe`, no `set_var`/`remove_var`, no global mutation | n/a | Confirmed |

## Findings (existing code surfaced during the pass)

Existing-code findings were triaged into prescriptive GitHub issues rather
than fixed in this change set (out of scope for a tests/benchmarks pass):

| Issue | Dimension | Severity |
|-------|-----------|----------|
| #160 api-types `skip_serializing_if` consistency | performance/api-hygiene | low |
| #161 `JobError` predicate doc comments | documentation | low |
| #162 `DailySchedule::parse` strictness | input validation | low |
| #163 `escape_fts5` whitespace in phrase | FTS5 correctness | low |
| #164 SSE parser unbounded buffer / O(n²) scan | security/perf | medium/low |
| #165 `MimirClient::new` panics on build failure | robustness | low |
| #166 `LlmClient` `.expect` on reqwest build | robustness | low |
| #167 `MimirClient` DRY `check_response` pattern | code quality | low |
| #168 `parse_datetime` silent UTC assumption | data-correctness | low |

## Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --tests --benches -- -D warnings` — clean.
- `cargo test --workspace` — all suites pass (api-types 46, client 64,
  core lib 211, knowledge lib 110, server 65, bin 29, plus all integration
  suites).
- `cargo build --workspace` — clean.

Zero unactioned findings in the new code; review returns zero findings.

## Review-Fix Pass (PR #169, v0.54.5)

CodeRabbit posted five inline review comments on the change set; all were
verified against current code and actioned in v0.54.5.

| # | File | Finding | Action |
|---|------|---------|--------|
| 1 | `mimir-api-types/src/lib.rs` | `json.contains($skip)` is substring-based and can match value text | Parse JSON into `serde_json::Map` and assert `!obj.contains_key($skip)` |
| 2 | `mimir-client/src/lib.rs` | KB endpoint tests only matched the path, not query params | Added `query_param` matchers for `kb_query`, `kb_browse`, `kb_profile`, `kb_audit`, `kb_trash` |
| 3 | `mimir-core/benches/pure_helpers.rs` | `Utc::now()` baseline made the schedule benchmark non-deterministic | Replaced with fixed `2024-06-15T14:30:00Z` `DateTime<Utc>` |
| 4 | `mimir-core/src/job_queue.rs` | No test documented chrono's padding-agnostic `%H:%M` parsing | Added `daily_schedule_parse_accepts_non_zero_padded_input` |
| 5 | `mimir/src/kb.rs` | Comment implied conditional "ellipsis or empty" behaviour | Corrected to deterministic "just ellipsis" |

All five findings fixed; `cargo fmt`, `clippy --all-targets`, and the
workspace test suite remain green. Review returns zero findings.
