# Nightly Knowledge Graph Optimization

## What does it do?

Every night Mimir performs a multi-step health check on your knowledge graph:

1. **Back up** – creates a dated snapshot of the knowledge database.
2. **Deduplicate** – merges identical facts and boosts their confidence.
3. **Semantic deduplication** – asks the LLM whether similar facts should be merged; high-confidence merges happen automatically, uncertain ones are queued for your review.
4. **Resolve contradictions** – keeps the most reliable version when facts disagree.
5. **Recalculate confidence** – updates any fact flagged as needing recalculation, recomputing it from its parents and clearing the flag, then refreshing the facts inferred from it.
6. **Clean up dormant facts** – safely forgets old, disputed facts that have been superseded.
7. **Compact database** – rebuilds search indexes and frees unused space.

## What happens to my data?

- Merged facts are marked **Superseded**, not deleted, so you can still trace their history.
- Forgotten facts go to the **Trash** for 30 days before permanent removal.
- Pending confirmations older than 7 days are automatically rejected and deleted.
- A backup is created before any changes begin.

## Configuration

You can change the schedule and timeout in `~/.config/mimir/config.toml`:

```toml
[knowledge.optimization]
schedule_time = "02:00"
timeout_minutes = 120
cpu_cores = 1
nice_level = 10
# memory_limit_mb = 2048  # Optional: best-effort memory cap (Linux cgroup v2)
```

`cpu_cores` limits how many CPUs the optimizer may use (Linux), `nice_level` is a signed Unix priority value (positive values lower scheduling priority, negative values raise it and may require additional privileges), and `memory_limit_mb` optionally caps the Mimir process's memory while the optimizer runs (Linux systems with a writable cgroup v2 setup). All limits are best-effort: if your system cannot apply one, the optimizer still runs.

## What if I use Mimir while it runs?

The optimizer checks whether you are actively chatting. If you have interacted within the last minute, it pauses between passes so it never slows down your session. The background scheduler also gates the optimization start on LLM downtime, ensuring it does not compete with your conversations.

## Troubleshooting

- **"Job not registered"** – the daemon is not running. Start it with `mimir start`.
- **Backup already exists** – if you run optimization twice in one day, the backup gets a numeric suffix (e.g. `knowledge-2026-06-04-1.db`). Each run always gets its own uniquely named file, so overlapping runs never overwrite each other's snapshots.
