-- 054: Generic email connector type (issue #400).
--
-- The IMAP mail connector is a generic IMAP client (Gmail, Outlook, Yahoo,
-- iCloud, Proton Bridge, custom servers), so the persisted connector type is
-- the generic `Email` (id 1 stays; only the display name changes). Existing
-- rows keep type id 1 — they were IMAP mail connectors all along — and the
-- `connector_reliability` score for id 1 is untouched. The wire string is
-- driven by the `ConnectorType` Rust enum (`as_str`), not this lookup row;
-- this keeps the DB label consistent with the enum for tooling that reads
-- the seed directly.

UPDATE connector_types
SET name = 'Email'
WHERE id = 1;
