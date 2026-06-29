# Forgetting Facts

Mimir never hard-deletes a fact on the first request. Forgetting is a **soft-delete**: the fact moves to a trash bin with a 30-day expiry, so you can always undo a mistake. Inferred facts that depended on a forgotten fact are re-evaluated automatically.

This page covers how forgetting works, the trash bin, restoration, the cascade, and the safeguards against accidental bulk loss. For the command reference, see [CLI Commands](cli-commands.md).

---

## How to Forget

Forget at any granularity with `mimir kb forget`:

```bash
# A single fact
mimir kb forget --fact-id 42

# Every fact with a given predicate
mimir kb forget --predicate visited --yes

# Everything about an entity (as subject or object)
mimir kb forget --entity "Alice" --yes

# Everything from a given source
mimir kb forget --source gmail --yes

# Facts created within a time window
mimir kb forget --from 2025-01-01 --to 2025-06-30 --yes

# A full reset (creates a timestamped backup first)
mimir kb forget --all --confirmation-phrase "DELETE EVERYTHING"
```

Every filter can be combined. Without `--all`, matched facts are soft-deleted to trash.

---

## The Trash Bin

When a fact is forgotten:

1. The fact row and its `sources` are serialised into a `trash` row as a JSON payload (the fact, its sources, and the `(parent_fact_id, relation_type_id)` pairs needed to rebuild its dependency chain).
2. The fact row is deleted from `facts`; its `sources` cascade away.
3. The trash row gets a 30-day `expires_at` (from the moment of deletion).
4. A `Forgotten` audit entry is written.

Trash is **not** a second copy of the graph — it holds enough to fully reconstruct a forgotten fact on restore, and nothing else.

### Listing and emptying

```bash
mimir kb trash              # list trash contents (newest first)
mimir kb trash --empty      # permanently remove all trash rows now
```

Expired trash rows (past their 30-day `expires_at`) are also permanently removed by a nightly cleanup pass (see [Nightly Optimization](nightly-optimization.md)).

---

## Restoring

Restore reverses a soft-delete:

```bash
mimir kb restore --trash-id 7   # restore one fact
mimir kb restore --all          # restore everything in trash
```

Restoration runs in two passes:

1. **Facts first** — each trash payload is deserialised and the fact (with its sources) is re-inserted. New entity ids are mapped if the original entity still exists; the fact is restored to `Active`, or to `Disputed` if it now temporally overlaps an existing fact, with a `Restored` audit entry.
2. **Dependencies second** — `fact_dependencies` edges are rebuilt using the stored `(parent_fact_id, relation_type_id)` pairs, but only where the parent fact still exists. Edges to parents that are themselves still in trash (or gone) are skipped, so restore never recreates a dangling chain.

Once restored, the fact's confidence is recalculated from its (now partial) parent set, so an inferred fact whose parents were not restored will end up with a lower confidence rather than an inconsistent one.

---

## Cascade Forget

Forgetting a fact does not just remove that one row — Mimir tracks which facts were **inferred from** it via the `fact_dependencies` junction table (`InferredFrom` edges). After a fact is forgotten:

1. Every inferred child is collected.
2. The forgotten fact is removed from each child's dependency chain.
3. Each orphaned child is re-evaluated:
   - If it has **no remaining `InferredFrom` parents**, it can no longer be derived, so it is **soft-deleted to trash too** (the cascade continues into its own children).
   - If it still has parents, its confidence is **recalculated** from the remaining chain. If the recalculated confidence drops below `0.20`, the fact is marked **`Disputed`** (a `StatusChange` audit entry) rather than trusted as reliable; otherwise it survives with its updated confidence.

This means removing a root fact cleanly collapses the inference subtree that depended on it, while facts that are still derivable from other sources survive (with adjusted confidence). The cascade is recursive and cycle-safe.

---

## Bulk Safeguards

Bulk forgetting can touch a lot of data at once, so Mimir gates it:

- **`>100` facts** — bulk deletions that match more than 100 facts require `--yes` to proceed; otherwise the command aborts with a count and a prompt to confirm.
- **Sensitive predicates** — forgetting facts whose predicate is flagged sensitive (medical, financial, identity — e.g. `allergy`, `medication`, `password`) requires `--confirm-sensitive`, even for small sets.
- **Full reset** — `--all` requires typing the exact phrase `DELETE EVERYTHING` via `--confirmation-phrase`, and creates a **timestamped database backup** before deleting anything. With `--archive`, a full reset soft-deletes to trash instead of hard-deleting; without it, the reset hard-deletes (no trash recovery).

These safeguards are deterministic Rust checks — the LLM cannot bypass them.

---

## Auditing Forgets

Every forget, restore, and bulk operation writes to the immutable `fact_audit_log` (`Forgotten`, `Restored` change types). Inspect the history with:

```bash
mimir kb audit --entity Alice --change_type forgotten
```

See [CLI Commands](cli-commands.md) for the full `mimir kb audit` reference.
