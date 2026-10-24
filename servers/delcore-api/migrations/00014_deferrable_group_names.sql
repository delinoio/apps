-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Final tenant deletion anonymizes every group name to the group's own ID with a single
-- UPDATE. A non-deferrable unique constraint is checked per row, so an organization whose
-- groups are legally named with each other's IDs (a crossed pair or a longer cycle) failed
-- that statement on its first changed row and aborted maintenance on every worker step.
-- A DEFERRABLE INITIALLY IMMEDIATE constraint is still checked at the end of each statement,
-- so ordinary inserts and renames reject duplicates exactly as before, while one statement
-- that permutes names within a tenant succeeds when its final names are unique.
-- PostgreSQL cannot alter the deferrability of a unique constraint in place, and a
-- deferrable constraint cannot be an ON CONFLICT arbiter. No statement uses ON CONFLICT on
-- groups. The constraint name is preserved; the primary key and the (tenant_id,id) key that
-- foreign keys reference stay immediate and are not touched.
ALTER TABLE groups DROP CONSTRAINT groups_tenant_id_name_key;
ALTER TABLE groups ADD CONSTRAINT groups_tenant_id_name_key UNIQUE(tenant_id,name) DEFERRABLE INITIALLY IMMEDIATE;
