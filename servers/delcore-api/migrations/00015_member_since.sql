-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Consumer services read the start of a user's current membership through
-- GetTenantMemberRoles, so that a departure followed by a rejoin is detectable even when no
-- read ran during the absence. Leaving deletes the members row and rejoining inserts a new one,
-- so a column set on insert and never updated by role changes marks the current membership.
-- Existing rows receive the migration time: their real start is not recorded, and a value that
-- does not change on later role updates is all consumers compare.
ALTER TABLE members ADD COLUMN member_since timestamptz NOT NULL DEFAULT now();
