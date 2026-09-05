# Categories & Aliases

> **Added:** 0.49.0 (Issue #135)
>
> **Crate:** `mimir-knowledge`

## What it is

Mimir's knowledge graph organises facts with two complementary layers:

- **Predicate aliases** — thin canonical verbs (`studied_at`, `works_at`, `has_partner`) with English synonyms (`attended`, `employer`, `wife`). Used to *canonicalise* the verb on a fact so the same relationship is never stored under multiple predicate rows.
- **Categories (Dewey-Decimal taxonomy)** — the semantic home for grouping, hierarchy, and multi-tag precision. A fact carries 1–3 category tags; categories carry a `memory_weight` that drives memory ranking and a `memory_bucket_id` that decides which memory section (Identity, Relationships, Preferences, Upcoming, or General) the fact is rendered under. Buckets are seeded with the taxonomy (migration `052`), so memory classification follows the data, not hard-coded id ranges.

**Category aliases** map natural-language domain words (`"hobbies"`, `"education"`, `"family"`) to a category id, so a user or agent can ask for a domain by name and retrieve every fact in that domain — including subcategories — without knowing the numeric Dewey ids.

## How it works

1. **Resolve the word** — `KnowledgeGraph::resolve_category_alias("hobbies")` → `Some(770)`. Lookup normalises case/whitespace (`"  Hobbies  "` → `770`); unknown or empty words return `None`.
2. **Expand the subtree** — `KnowledgeGraph::get_descendant_category_ids(700)` walks the `categories.parent_id` tree recursively, returning all descendant ids (710, 740, 770, 780, …).
3. **Gather facts** — `KnowledgeGraph::get_facts_in_category_subtree(700, limit)` returns every fact tagged anywhere in the subtree (root + descendants), ordered by confidence.

Aliases are stored in the `category_aliases` table (globally unique `alias` → `category_id`). Insertion is idempotent and race-safe: `insert_category_alias` performs an atomic `INSERT OR IGNORE` then resolves the resulting mapping, so concurrent writers never leak a raw `UNIQUE`-constraint error — rebinds to a different category return a `Validation` error, and empty aliases / unknown category ids are rejected. The seed migration (`038`) runs inside a transaction with foreign-key enforcement on and uses `CREATE … IF NOT EXISTS` for defensive idempotency.

## Why categories, not a predicate hierarchy

A predicate tree follows one axis (a predicate has one canonical name and one parent path). Categories are many-to-many: "Alice works_at Foo as an engineer" can be both `Current Role` and `Skills & Expertise`; "hobbies" spans `Music`, `Gaming`, `Outdoor Activities`. That granularity is what a reasoning agent needs — indoor vs outdoor for weather-aware suggestions, budget-relevant tags, shared-ground detection across two people. So grouping lives in categories; the predicate hierarchy is seeded (issue #403) with a few abstract parents (`employment`, `education`, `residence`, `containment`) purely so `kg_query --include-subtree` can ask "everything about employment" in one call. The parents are query-only — they can never be used as fact predicates.

## Use cases

- **"Find an activity for me and my wife on Saturday"** — resolve `has_partner` to find her entity (predicate alias), then gather both people's interests via the `Entertainment & Leisure` category subtree, then filter by calendar/weather/budget (connectors, later phases).
- **Domain summarisation** — "tell me everything about her education" → resolve `education` → 550 → subtree facts.
- **Memory ranking** — `memory_weight` from `MAX(c.memory_weight)` over a fact's categories shapes what surfaces in condensed memory.

## Best practices

- Tag facts with the **most specific** subcategory available at extraction time (the extraction prompt renders the complete category tree and instructs the LLM to assign 1–3 valid category ids).
- Add new domain words via `insert_category_alias` rather than inventing new predicates — keep predicates as thin verbs.
- Use the seeded predicate DAG parents (`employment`, `education`, `residence`, `containment`) for coarse verb generalisation in `kg_query --include-subtree`, and category subtrees for multi-axis grouping and memory ranking.
- When adding a category outside the seeded taxonomy, set its memory bucket explicitly (`kb category add --memory-bucket-id 4` for a preference) — an unset bucket classifies as General.

## See also

- [Knowledge Graph Schema](../knowledge-graph-schema.md) — `Category Aliases & Subtree Retrieval` section
- [KG Tools](../kg-tools.md)
