-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE TABLE lane_entities (
 account_id text NOT NULL, id text NOT NULL, kind smallint NOT NULL CHECK(kind IN (1,2)),
 deleted boolean NOT NULL DEFAULT false, repository_id text, normalized_url text,
 accepted_version_id text NOT NULL,
 PRIMARY KEY(account_id,id)
);
CREATE UNIQUE INDEX lane_repository_url ON lane_entities(account_id,normalized_url)
 WHERE kind=1 AND NOT deleted;
CREATE INDEX lane_repository_tasks ON lane_entities(account_id,repository_id) WHERE kind=2 AND NOT deleted;
CREATE TABLE lane_versions (
 account_id text NOT NULL, version_id text NOT NULL, entity_id text NOT NULL,
 envelope jsonb NOT NULL, prompt_version boolean NOT NULL DEFAULT false,
 PRIMARY KEY(account_id,version_id),
 UNIQUE(account_id,entity_id,version_id),
 FOREIGN KEY(account_id,entity_id) REFERENCES lane_entities(account_id,id) ON DELETE CASCADE
);
CREATE TABLE lane_heads (
 account_id text NOT NULL, entity_id text NOT NULL, version_id text NOT NULL,
 PRIMARY KEY(account_id,entity_id,version_id),
 FOREIGN KEY(account_id,entity_id,version_id) REFERENCES lane_versions(account_id,entity_id,version_id) ON DELETE CASCADE
);
CREATE TABLE lane_entity_receipts (
 account_id text NOT NULL, method text NOT NULL, request_id text NOT NULL,
 fingerprint text NOT NULL, entity_id text NOT NULL, response jsonb NOT NULL,
 PRIMARY KEY(account_id,method,request_id)
);
CREATE TABLE lane_repository_mappings (
 account_id text NOT NULL, local_id text NOT NULL, canonical_id text NOT NULL,
 PRIMARY KEY(account_id,local_id),
 FOREIGN KEY(account_id,canonical_id) REFERENCES lane_entities(account_id,id)
);
CREATE TABLE lane_changes (
 account_id text NOT NULL, sequence bigserial PRIMARY KEY, data jsonb NOT NULL
);
CREATE INDEX lane_changes_account_sequence ON lane_changes(account_id,sequence);
-- Cursors are opaque, account/query-bound durable positions, never client-trusted offsets.
CREATE TABLE lane_entity_cursors (
 token text PRIMARY KEY, account_id text NOT NULL, scope text NOT NULL, position jsonb NOT NULL
);
