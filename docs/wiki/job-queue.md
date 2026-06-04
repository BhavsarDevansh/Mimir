# Background Jobs

## What is it?

Mimir runs maintenance tasks in the background so your knowledge graph stays clean and accurate without manual intervention.

## Current jobs

- **Knowledge graph optimization** – runs every night (default 02:00) to deduplicate facts, resolve contradictions, recalculate confidence, and clean up old data.

## How to check status

```bash
mimir kb optimization --status
```

Shows the job schedule, next run time, and the result of the last run.

## How to run manually

```bash
mimir kb optimization --run-now
```

Triggers the full optimization pipeline immediately. This can take a few minutes depending on graph size.

## Best practices

- Let the nightly schedule handle routine maintenance.
- Run manually only when you want to force cleanup after a large import or batch operation.
- The daemon must be running for `--status` and `--run-now` to work.
