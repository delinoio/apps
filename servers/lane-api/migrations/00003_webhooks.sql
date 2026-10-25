-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Receiver IDs route unauthenticated HTTP requests and therefore are globally unique.
CREATE TABLE lane_webhook_receivers (
    account_id text NOT NULL,
    id text PRIMARY KEY,
    repository_id text NOT NULL,
    name text NOT NULL,
    revision bigint NOT NULL DEFAULT 1,
    encrypted_key bytea,
    deletion_version_id text,
    UNIQUE(account_id, id)
);
CREATE INDEX lane_webhook_receivers_repository ON lane_webhook_receivers(account_id, repository_id, id);
CREATE TABLE lane_webhook_events (
    account_id text NOT NULL,
    id text NOT NULL,
    receiver_id text NOT NULL,
    repository_id text NOT NULL,
    sender_event_id text NOT NULL,
    fingerprint text NOT NULL,
    data jsonb,
    state text NOT NULL CHECK (state IN ('PENDING','LEASED','COMPLETED','DELETED')),
    revision bigint NOT NULL DEFAULT 1,
    lease_hash text,
    claimant text,
    expires_at timestamptz,
    last_error text,
    deletion_version_id text,
    PRIMARY KEY(account_id,id),
    UNIQUE(account_id,receiver_id,sender_event_id),
    CHECK ((state = 'LEASED') = (lease_hash IS NOT NULL AND claimant IS NOT NULL AND expires_at IS NOT NULL)),
    CHECK ((state = 'DELETED') = (data IS NULL AND deletion_version_id IS NOT NULL))
);
CREATE INDEX lane_webhook_events_repository ON lane_webhook_events(account_id,repository_id,id);
CREATE TABLE lane_webhook_processing (
    account_id text NOT NULL,
    id text NOT NULL,
    event_id text NOT NULL,
    data jsonb NOT NULL,
    PRIMARY KEY(account_id,id),
    FOREIGN KEY(account_id,event_id) REFERENCES lane_webhook_events(account_id,id) ON DELETE CASCADE
);
-- No plaintext receiver keys, lease tokens, or request bodies in receipts.
CREATE TABLE lane_webhook_receipts (
    account_id text NOT NULL,
    method text NOT NULL,
    request_id text NOT NULL,
    resource_id text NOT NULL,
    fingerprint text NOT NULL,
    response jsonb NOT NULL,
    PRIMARY KEY(account_id,method,request_id)
);
