-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE TABLE lane_push_registrations (
    account_id text NOT NULL,
    device_id text NOT NULL,
    provider smallint NOT NULL CHECK (provider BETWEEN 1 AND 3),
    endpoint text NOT NULL,
    public_key text NOT NULL DEFAULT '',
    auth_secret text NOT NULL DEFAULT '',
    enabled boolean NOT NULL DEFAULT false,
    content smallint NOT NULL DEFAULT 1 CHECK (content IN (1, 2)),
    PRIMARY KEY (account_id, device_id)
);
CREATE TABLE lane_push_deliveries (
    account_id text NOT NULL,
    proposal_id text NOT NULL,
    device_id text NOT NULL,
    repository_name text NOT NULL,
    title text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL DEFAULT (clock_timestamp() + interval '2 hours'),
    next_attempt_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','success','cancelled','expired','permanent_failure','exhausted')),
    PRIMARY KEY (account_id, proposal_id, device_id)
);
CREATE INDEX lane_push_due ON lane_push_deliveries (next_attempt_at) WHERE state = 'pending';
CREATE TABLE lane_push_attempts (
    account_id text NOT NULL,
    proposal_id text NOT NULL,
    device_id text NOT NULL,
    attempt integer NOT NULL CHECK (attempt BETWEEN 1 AND 6),
    started_at timestamptz NOT NULL,
    finished_at timestamptz,
    result text CHECK (result IN ('success','transient_failure','permanent_failure')),
    retry_after timestamptz,
    PRIMARY KEY (account_id, proposal_id, device_id, attempt),
    FOREIGN KEY (account_id, proposal_id, device_id)
        REFERENCES lane_push_deliveries ON DELETE CASCADE
);
-- Content-free lifetime counters survive account-data deletion.
CREATE TABLE lane_push_counters (
    result text PRIMARY KEY CHECK (result IN ('success','transient_failure','permanent_failure')),
    total bigint NOT NULL DEFAULT 0 CHECK (total >= 0)
);
INSERT INTO lane_push_counters(result) VALUES ('success'),('transient_failure'),('permanent_failure');
