-- SPDX-License-Identifier: FSL-1.1-ALv2
ALTER TABLE lane_entities DROP CONSTRAINT lane_entities_kind_check;
ALTER TABLE lane_entities ADD CONSTRAINT lane_entities_kind_check CHECK (kind IN (1,2,3));
CREATE INDEX lane_repository_rules ON lane_entities(account_id,repository_id) WHERE kind=3 AND NOT deleted;
CREATE TABLE lane_attachments (
 account_id text NOT NULL, id text NOT NULL, task_id text NOT NULL,
 manifest jsonb NOT NULL, object_key text NOT NULL, available boolean NOT NULL DEFAULT false,
 pending_png bytea, attempt_after timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(account_id,id),
 FOREIGN KEY(account_id,task_id) REFERENCES lane_entities(account_id,id) ON DELETE CASCADE
);
CREATE TABLE lane_attachment_uploads (
 account_id text NOT NULL, id text NOT NULL, attachment_id text NOT NULL,
 object_key text NOT NULL, expires_at timestamptz NOT NULL,
 PRIMARY KEY(account_id,id),
 FOREIGN KEY(account_id,attachment_id) REFERENCES lane_attachments(account_id,id) ON DELETE CASCADE
);
-- Durable garbage work survives account clearing and failures of the object provider.
-- Keys contain random IDs, no task content, URLs or credential material.
CREATE TABLE lane_attachment_garbage (
 object_key text PRIMARY KEY, delete_after timestamptz NOT NULL DEFAULT now()
);

-- Content-free identity bindings survive task deletion to prevent attachment ID reassignment.
CREATE TABLE lane_attachment_identities (
 account_id text NOT NULL, id text NOT NULL, task_id text NOT NULL, fingerprint text NOT NULL,
 PRIMARY KEY(account_id,id)
);
