## [0.166.3] — 2026-09-13

### Fix: KB category create validation and prompt safety (issue #599)

- The knowledge-graph category create boundary now validates new category IDs and names before insert. New IDs must be positive, names are trimmed, and empty or whitespace-only names are rejected.
- Control-character names, including newlines and carriage returns, are rejected so the extraction prompt cannot be reshaped by user-supplied category names.
- Parent relationships keep their existing foreign-key constraint and self-parent cycle guard.
- Knowledge-graph and API tests cover the canonicalised names, rejected inputs, positive IDs, self-parent cycle prevention, and valid parent relationships.
- Docs updated: `docs/knowledge-graph-schema.md`, `docs/wiki/categories-and-aliases.md`.
- Version bumped 0.166.2 → 0.166.3 (patch — bugfix).
