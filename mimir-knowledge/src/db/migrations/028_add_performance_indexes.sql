-- Performance index for kg_query, kg_related, and kg_search tool queries.
-- Covers: subject_id, pending_confirmation, fact_status_id, confidence DESC
CREATE INDEX IF NOT EXISTS idx_facts_tool_query ON facts(subject_id, pending_confirmation, fact_status_id, confidence DESC);
