# Changelog

## 0.24.3 (2026-05-31)

### Added

- Structural confidence model (issue #51): confidence derived entirely from graph structure, zero LLM involvement, zero time-based decay.
- New `SourceType` variants: `CasualMention`, `Import`, `System`.
- New `ConnectorType` enum with SQLite lookup table and reliability tracking.
- `inference_confidence` formula: signed parent sum × chain penalty (0.8^depth) × breadth factor.
- `inference_depth` and `stale_confidence` columns on `facts` table.
- `is_positive` column on `fact_dependencies` for signed parent contributions.
- Per-connector reliability scores with feedback loop (`adjust_connector_reliability`).
- Eager bounded confidence cascade on parent removal.

### Changed

- `NewFact` no longer accepts caller-provided `confidence`; all confidence is Rust-computed. This is an internal-only change — `mimir-knowledge` is not a public library and `NewFact` is only used within the Mimir workspace.
- Connector-type source facts now use per-connector reliability scores instead of flat 0.80.
- Initial confidence values: `UserEdit`/`System` = 1.0, `CasualMention` = 0.30, `Import` = 0.80.

### Fixed

- Updated all test assertions and raw SQL to match new schema columns.
