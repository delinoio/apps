-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE TABLE settings (singleton boolean PRIMARY KEY CHECK(singleton), value jsonb NOT NULL);
CREATE TABLE api_keys (
 id uuid PRIMARY KEY, tenant_id uuid NOT NULL, user_id text NOT NULL,
 kind integer NOT NULL CHECK(kind IN (1,2)), capabilities integer[] NOT NULL,
 secret_hash bytea NOT NULL UNIQUE, created_at timestamptz NOT NULL,
 expires_at timestamptz, revoked boolean NOT NULL DEFAULT false
);
CREATE TABLE catalog (model text PRIMARY KEY, price jsonb NOT NULL, effective_at timestamptz NOT NULL);
CREATE TABLE requests (
 id uuid PRIMARY KEY, tenant_id uuid NOT NULL, capability integer NOT NULL,
 dedup_key text, input_hash bytea NOT NULL, state integer NOT NULL CHECK(state IN (1,2,3)),
 model text NOT NULL, created_at timestamptz NOT NULL,
 UNIQUE(tenant_id,capability,dedup_key)
);
-- Attempts have their own 30-day lifetime; request deletion cannot cascade into them.
CREATE TABLE attempts (
 id uuid PRIMARY KEY, request_id uuid NOT NULL, tenant_id uuid NOT NULL,
 capability integer NOT NULL, model text NOT NULL, price jsonb NOT NULL,
 price_effective_at timestamptz NOT NULL, occurred_at timestamptz NOT NULL,
 generation_id text NOT NULL DEFAULT '', usage jsonb,
 delivered boolean NOT NULL DEFAULT false, created_at timestamptz NOT NULL,
 retry_at timestamptz NOT NULL, failures integer NOT NULL DEFAULT 0
);
CREATE INDEX pending_attempts ON attempts(retry_at) WHERE NOT delivered;
CREATE TABLE audit (
 id uuid PRIMARY KEY, actor_id text NOT NULL, action text NOT NULL,
 reason text NOT NULL, request_id uuid NOT NULL, created_at timestamptz NOT NULL
);
CREATE TABLE operational_logs (
 id uuid PRIMARY KEY, category text NOT NULL, request_id uuid NOT NULL,
 created_at timestamptz NOT NULL
);
