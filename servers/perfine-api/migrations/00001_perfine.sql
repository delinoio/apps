-- SPDX-License-Identifier: FSL-1.1-ALv2
BEGIN;
SELECT pg_advisory_xact_lock(hashtextextended('perfine:migrations',0));
CREATE TABLE IF NOT EXISTS perfine_connections (
 id uuid PRIMARY KEY, space uuid NOT NULL, repository bigint NOT NULL CHECK(repository>0),
 owner_subject text NOT NULL, github_actor text NOT NULL, active boolean NOT NULL, data jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(space,repository)
);
CREATE UNIQUE INDEX IF NOT EXISTS perfine_single_space ON perfine_connections(repository) WHERE active;
CREATE TABLE IF NOT EXISTS perfine_experiments (
 id uuid PRIMARY KEY, space uuid NOT NULL, connection uuid NOT NULL REFERENCES perfine_connections(id),
 data jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS perfine_tombstones (id uuid PRIMARY KEY);
CREATE TABLE IF NOT EXISTS perfine_uploads (
 id uuid PRIMARY KEY, experiment uuid NOT NULL REFERENCES perfine_experiments(id), data jsonb NOT NULL,
 staging_cleaned boolean NOT NULL DEFAULT false, reserved_bytes bigint NOT NULL CHECK(reserved_bytes>=0), staging_expires_at timestamptz NOT NULL DEFAULT now()+interval '16 minutes', created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS perfine_rejected_objects (object_key text PRIMARY KEY, expires_at timestamptz NOT NULL, next_attempt timestamptz NOT NULL DEFAULT now());
CREATE TABLE IF NOT EXISTS perfine_candidates (
 id uuid PRIMARY KEY, experiment uuid NOT NULL REFERENCES perfine_experiments(id), data jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS perfine_approvals (
 id uuid PRIMARY KEY, experiment uuid NOT NULL REFERENCES perfine_experiments(id), data jsonb NOT NULL,
 publication jsonb, attempts integer NOT NULL DEFAULT 0, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS perfine_cleanup (
 experiment uuid PRIMARY KEY, space uuid NOT NULL, data jsonb NOT NULL,
 attempts bigint NOT NULL DEFAULT 0, next_attempt timestamptz NOT NULL DEFAULT now(),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS perfine_dedup (
 actor text NOT NULL, space uuid NOT NULL, method text NOT NULL, request uuid NOT NULL,
 digest text NOT NULL, response jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(actor,space,method,request)
);
CREATE TABLE IF NOT EXISTS perfine_audit (
 id uuid PRIMARY KEY, space uuid NOT NULL, actor text NOT NULL, method text NOT NULL,
 request uuid NOT NULL, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS perfine_rate (
 space uuid PRIMARY KEY, window_started timestamptz NOT NULL DEFAULT now(), requests bigint NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS perfine_cursors (
 id uuid PRIMARY KEY, binding text NOT NULL, after_id uuid NOT NULL, after_time timestamptz NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
COMMIT;
