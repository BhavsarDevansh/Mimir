-- 049: Connector-side durable state (issue #262).
--
-- Opaque, connector-owned state that must survive daemon restarts alongside
-- the sync progress. The Email connector persists its LLM prose-extraction
-- retry ledger here (pending retries with attempt counts + backoff, and
-- terminal failures with reasons) so a message whose extraction failed keeps
-- its bounded retry budget across restarts instead of being dropped when the
-- in-memory buffer is lost. The column is deliberately generic: any connector
-- may store its own opaque durable state, mirroring the opaque `sync_cursor`.
-- Connectors never write it directly (the crate is sqlx-free); the supervisor
-- persists `Connector::durable_state()` after each successful extraction
-- cycle via `KnowledgeGraph::update_durable_state` and re-injects it at
-- construction as `__durable_state`.

ALTER TABLE connectors ADD COLUMN durable_state TEXT;
