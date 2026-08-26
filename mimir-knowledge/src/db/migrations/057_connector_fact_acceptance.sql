-- 057: Connector fact-acceptance counters (issue #508).
--
-- Cumulative counters for the email LLM prose-extraction layer, written by
-- the `connector_item.remember` hook after each successful extraction run:
-- `facts_accepted` counts validated facts handed to the knowledge graph and
-- `facts_dropped` counts LLM-emitted facts rejected by Rust-side validation
-- (non-canonical predicates, invalid entity types). Surfaced by
-- `mimir connector list` / `status` so silent vocabulary drops are visible
-- instead of hiding behind the `items` count (the outlook backfill that hid
-- 247 dropped facts behind `items: 14`). Both default to 0 for existing and
-- new instances; backends that do not run the LLM layer leave them at 0.

ALTER TABLE connectors ADD COLUMN facts_accepted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE connectors ADD COLUMN facts_dropped INTEGER NOT NULL DEFAULT 0;
