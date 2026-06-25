-- 040: Drop the superseded `entity_dates` subsystem (issue #74).
--
-- Events & reminders are now modelled as a lifecycle overlay on facts (see
-- migration 039). The `entity_dates` table had no rows in any known deployment,
-- so this is a pure schema cleanup with no data migration. `entity_date_types`
-- is dropped after `entity_dates` because it has no remaining dependents.

DROP TABLE IF EXISTS entity_dates;
DROP TABLE IF EXISTS entity_date_types;
