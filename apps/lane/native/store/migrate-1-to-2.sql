-- SPDX-License-Identifier: Apache-2.0
-- The caller disables foreign keys before BEGIN EXCLUSIVE, holds the space's
-- exclusive lifetime lock, validates the old format, and checks foreign keys
-- before committing. Immutable envelopes and request fingerprints stay intact.
CREATE TABLE entities_next (
  id TEXT PRIMARY KEY, kind INTEGER NOT NULL CHECK(kind IN (1,2,3)),
  repository_id TEXT, normalized_url TEXT,
  deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1))
);
INSERT INTO entities_next SELECT id,kind,repository_id,normalized_url,deleted FROM entities;
DROP TABLE entities;
ALTER TABLE entities_next RENAME TO entities;
CREATE UNIQUE INDEX repository_url ON entities(normalized_url)
  WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL;
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
UPDATE store_meta SET value='2' WHERE key='format_version';
PRAGMA user_version=2;

-- Retain only immutable identities after owner deletion; never image content.
CREATE TABLE attachment_identities (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  sha256 TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL
);
