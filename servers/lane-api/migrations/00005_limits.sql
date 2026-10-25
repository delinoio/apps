-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE TABLE IF NOT EXISTS lane_rate_limits (
    scope text NOT NULL,
    identity text NOT NULL,
    window_start timestamptz NOT NULL,
    requests bigint NOT NULL CHECK (requests > 0),
    PRIMARY KEY (scope, identity)
);
CREATE TABLE IF NOT EXISTS lane_sync_rejections (
    account_id text NOT NULL,
    request_id text NOT NULL,
    reason integer NOT NULL,
    PRIMARY KEY(account_id,request_id)
);
CREATE TABLE IF NOT EXISTS lane_control_receipts (
    account_id text NOT NULL,
    method text NOT NULL,
    request_id text NOT NULL,
    session_id text NOT NULL,
    fingerprint text NOT NULL,
    response jsonb NOT NULL,
    PRIMARY KEY(account_id,method,request_id)
);
