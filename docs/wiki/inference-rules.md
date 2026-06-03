# Inference Rules

## How It Works

Mimir's knowledge graph automatically draws new conclusions from existing facts using a small set of Rust-native rules. When you insert a fact, the engine evaluates all registered rules against it. If a rule finds a match, it produces a new inferred fact, which is itself evaluated — creating a cascade up to the limits of the data.

A `CascadeContext` tracks every triple processed in the current cascade. If a rule tries to infer a fact that has already been processed, it is skipped. This prevents infinite loops in cyclic graphs while preserving unbounded depth.

## Examples

### Transitivity

If you tell Mimir:

- "Devansh visited Rome"
- "Rome is_in Italy"

The transitivity rule infers:

- "Devansh visited Italy"

Confidence is computed from both parent facts, penalised slightly for depth.

### Contradiction

If you add:

- "Alice is_in London" (explicit, today)
- "Alice is_in Paris" (from a connector, same time range)

Both facts are marked `Disputed` and linked by `Contradicts` edges. During nightly optimization, if one is explicit and the other is inferred, the inferred is automatically `Superseded`.

### Threshold

If you reject the action "hiking" three times, the threshold rule creates a preference:

```
reject_hiking = true  (confidence 0.70)
```

If you later delete one of the rejections, the nightly pass detects the count dropped below 3 and writes an audit log entry recommending review.

## Best Practices

- **Temporal ranges matter.** Overlapping facts with the same subject and predicate trigger contradiction logic. Use explicit `valid_from`/`valid_until` when you know them.
- **Explicit facts win.** A user-edited fact (`confidence = 1.0`) always takes precedence over an inferred one during contradiction resolution.
- **Depth is cheap but not free.** Each inference step multiplies confidence by `0.8`. Deep chains naturally decay in certainty.
- **Check inferred facts.** The engine is deterministic and transparent, but it is not omniscient. Review `Disputed` facts periodically.
