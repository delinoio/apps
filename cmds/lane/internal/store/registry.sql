-- SPDX-License-Identifier: Apache-2.0
CREATE TABLE spaces (
  id TEXT PRIMARY KEY, account_key TEXT NOT NULL UNIQUE,
  account_id TEXT, tenant_id TEXT, issuer TEXT, sync_enabled INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE registry_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
