# Closed Taxonomy and Fact Staging

Mimir uses a controlled relationship vocabulary, so the same real-world fact is represented consistently. The vocabulary lives in the database, not in a hardcoded list. Roots organise queries, leaves are the only predicates the LLM can emit, and every leaf has a deterministic category rule. Extraction schemas use the database's leaf list as a closed `relationship_type` enum, and the resolver re-validates the value in Rust.

If extraction produces a predicate that is not in the controlled vocabulary, Mimir stages the full fact for review instead of dropping it. Direct fact writes also reject unknown predicates, so vocabulary gaps cannot bypass the normal pipeline. Connector status shows accepted, dropped, and staged counts, so a vocabulary gap is visible.

Review staged facts with `mimir kb staged list --json`. To map a row to an existing leaf, run `mimir kb staged map <id> --relationship-type-id <leaf-id> --note "why"`. To discard it after review, run `mimir kb staged reject <id> --note "not relevant"`. Mimir will not map a staged row to a query-only root or intermediate node.

This design prevents arbitrary predicate growth while allowing the taxonomy to evolve. Stable positive preferences are emitted as `prefers` with the thing as the object, so `prefers tennis` is canonical; `likes`, `loves`, and concrete favourite forms are aliases or legacy rows rather than separate facts. Every insertion path — shared normalization, direct insert, and batch insert — receives the deterministic category fallback, so a fact cannot bypass classification. Domain-specific detail belongs in typed entity frames and attributes—for example, a booking entity can carry check-in and check-out times—rather than a new predicate for every provider or field.
