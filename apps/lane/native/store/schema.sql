-- SPDX-License-Identifier: Apache-2.0
CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE entities (
  id TEXT PRIMARY KEY, kind INTEGER NOT NULL CHECK(kind IN (1,2,3,4,5,6)),
  repository_id TEXT, normalized_url TEXT,
  deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1))
);
CREATE UNIQUE INDEX repository_url ON entities(normalized_url)
  WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL;
CREATE TABLE versions (
  version_id TEXT PRIMARY KEY, entity_id TEXT NOT NULL REFERENCES entities(id),
  envelope_json TEXT NOT NULL, created_at_us INTEGER NOT NULL,
  prompt_version INTEGER NOT NULL DEFAULT 0 CHECK(prompt_version IN (0,1))
);
CREATE TABLE heads (
  entity_id TEXT NOT NULL REFERENCES entities(id),
  version_id TEXT NOT NULL REFERENCES versions(version_id),
  PRIMARY KEY(entity_id,version_id)
);
CREATE TABLE receipts (
  method TEXT NOT NULL, request_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
  entity_id TEXT, result_json TEXT NOT NULL, PRIMARY KEY(method,request_id)
);
CREATE TABLE outbox (
  request_id TEXT PRIMARY KEY, entity_id TEXT NOT NULL,
  request_json TEXT NOT NULL, state INTEGER NOT NULL DEFAULT 1,
  error_reason TEXT, created_at_us INTEGER NOT NULL
);
CREATE TABLE changes (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  entity_id TEXT, change_kind INTEGER NOT NULL
);
CREATE TABLE remote_objects (
  kind INTEGER NOT NULL, id TEXT NOT NULL, body_json TEXT,
  deleted INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(kind,id)
);
CREATE TABLE repository_mappings (
  local_id TEXT PRIMARY KEY, canonical_id TEXT NOT NULL
);
CREATE TABLE notification_receipts (
  proposal_id TEXT NOT NULL, device_id TEXT NOT NULL,
  state INTEGER NOT NULL, PRIMARY KEY(proposal_id,device_id)
);

CREATE TABLE attachments (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES entities(id),
  sha256 TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL,
  png_base64 TEXT,
  synchronized INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX attachment_owner ON attachments(task_id);

-- Retain only immutable identities after owner deletion; never image content.
CREATE TABLE attachment_identities (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL
);
