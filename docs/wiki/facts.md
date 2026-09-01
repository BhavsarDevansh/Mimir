# Facts

Facts are the edges of Mimir’s knowledge graph: statements that connect entities (or literal values) with a predicate and a time range.

---

## What Is a Fact?

A fact has:
- **Subject** — the entity the statement is about (e.g., *Alice*).
- **Predicate** — the relationship (e.g., `located_in`, `works_as`, `visited`).
- **Object** — another entity or a literal string (e.g., *London* or "pizza").
- **Time range** — when the statement is true (`valid_from` → `valid_until`).
- **Confidence** — how certain Mimir is (0.0–1.0).
- **Status** — `Active`, `Inferred`, `Disputed`, `Corrected`, `Superseded`, or `Forgotten`.

Example:

```text
Alice --[located_in]--> London   (2020-01-01 → 2021-01-01, confidence 1.0, Active)
```


---

## Temporal Awareness

Mimir understands that facts can change over time without contradicting each other:

- **Non-overlapping ranges** are treated as a timeline, not a conflict.
  - Alice lived in London (2020), then moved to Paris (2021). Both facts stay `Active`.
- **Overlapping ranges** create a `Disputed` fact that needs review.
- **Open-ended facts** (no `valid_until`) are automatically closed when a new, explicitly-dated fact arrives.

Mimir applies this overlap check in SQLite rather than loading every fact for a subject. Multi-valued predicates only treat the same object as comparable; single-valued predicates may replace any overlapping value. This keeps timelines fast as a subject accumulates more facts while preserving the conflict behaviour above.

---

## Confidence

Confidence depends on where the fact came from:

| Source | Typical Confidence |
|--------|-----------------|
| You edited it directly | 1.00 |
| Connector (calendar, email, etc.) | ~0.80 (varies by connector reliability) |
| Inferred by the reasoning engine | computed from parents |
| Casual mention in conversation | 0.30 |
| Bulk import | 0.80 |
| System-generated | 1.00 |

If an inferred fact loses its supporting evidence, its confidence drops. When it falls below 0.20, Mimir flags it as `Disputed`.

A fact also gains confidence when **corroborated** — a second independent source reports the same claim (same subject + predicate + object, overlapping in time). Mimir adds the new source to the existing fact and boosts its confidence by `0.05` per corroborating source, up to `0.95`. Explicit facts (already `1.0`) and inferred facts (structurally derived) keep their confidence; only the source is added. See the [Confidence Model](Confidence-Model.md#corroboration-79).

---

## Forgetting

When a fact is removed, Mimir does not immediately erase it. Instead:

1. The fact is moved to the **trash** with a 30-day retention period.
2. Any facts that were inferred from it are re-evaluated.
3. Inferred facts with no remaining support are also forgotten.
4. Inferred facts that still have other support survive with updated confidence.

This cascade ensures the knowledge graph stays consistent when evidence changes.

---

## Audit Trail

Every insert, update, status change, confidence change, source addition, and delete is logged with a timestamp, a typed `change_type` and `changed_by`, and a **column-only** JSON snapshot of the affected field(s). The change types are `created`, `status_change`, `confidence_change`, `temporal_update`, `source_added`, `forgotten`, `restored`, `rejected`, and `content_update` (content edits such as changing a fact's object value). The `changed_by` field uses the same variant-style strings (`User`, `System`, `InferenceEngine`, `NightlyOptimization`) in both the fact-detail and audit-log views, so the two endpoints always agree.

You can inspect the full history of any fact through the API, or query the audit log directly from the CLI:

```bash
mimir kb audit --entity "Alice" --change-type status_change
mimir kb audit --entity "Alice" --change-type content_update
```

---

## Pending Sensitive-Fact Confirmation

When Mimir detects a sensitive fact (e.g. an allergy or health detail), it doesn't trust it immediately. The AI flags potential sensitive facts during extraction, but a deterministic Rust validation layer has the final say — it checks the fact's catalogue category and object text against a known sensitive set, overriding the AI if it was overly cautious. Only facts that pass both checks are stored with a **Disputed** status and flagged `pending_confirmation = TRUE` until you confirm or reject it.

### Why this matters

Sensitive facts carry real-world consequences (a wrong allergy record could be dangerous). Mimir holds them in limbo and asks you to confirm before they become active, high-confidence knowledge.

### How to use it

```bash
# See what's waiting
mimir kb pending

# Confirm a fact — it becomes Active with confidence 1.0
mimir kb confirm 42

# Reject a fact — it's permanently deleted (with an audit trail)
mimir kb reject 42 --reason "entered in error"
```

### What happens to ignored facts

Facts you neither confirm nor reject are **automatically deleted after 7 days** by the `knowledge.pending_cleanup` background job. This prevents stale, unverified claims from lingering forever. The retention period and run time are configurable:

```toml
[knowledge.pending_cleanup]
retention_days = 7
schedule_time = "03:30"
```

### Best practices

- Run `mimir kb pending` periodically after conversations that mention health, finances, or other sensitive topics.
- Use `--reason` when rejecting so the audit log explains *why*.
- Don't raise `retention_days` too high — pending facts are excluded from memory, search, and inference, so leaving them pending keeps them invisible.
