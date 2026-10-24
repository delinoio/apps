-- A scrubbed replay record keeps only non-personal routing data. Earlier releases cleared the
-- response and fingerprint but left the deleted end user's ID in target_user_id. Clear it on the
-- existing scrubbed create and update records. Delete records are never scrubbed and keep their
-- data. The statement is idempotent: a repeat finds no matching row.
UPDATE replays
SET target_user_id = NULL
WHERE scrubbed
  AND target_user_id IS NOT NULL
  AND operation NOT LIKE 'Delete%';
