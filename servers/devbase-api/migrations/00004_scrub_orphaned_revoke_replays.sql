-- A repeated RevokeApiKey could commit its replay record after a concurrent project deletion or
-- tenant cleanup had already scrubbed the project's records, leaving an unscrubbed record for a
-- project that no longer exists. Scrub those orphaned records now: clear the response and
-- fingerprint and set the scrubbed marker, keeping the caller, key, operation, tenant, project and
-- expiry. Records of existing projects, delete records and already-scrubbed records are unchanged.
-- The statement is idempotent: a repeat finds no matching row.
UPDATE replays
SET scrubbed = true,
    response = NULL,
    fingerprint = '\x'::bytea,
    target_user_id = NULL
WHERE operation = 'RevokeApiKey'
  AND NOT scrubbed
  AND project_id IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM projects p WHERE p.id = replays.project_id);
