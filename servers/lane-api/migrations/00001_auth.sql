-- SPDX-License-Identifier: FSL-1.1-ALv2
CREATE TABLE lane_accounts (
    account_id text PRIMARY KEY,
    tenant_id text NOT NULL,
    epoch text NOT NULL
);
CREATE TABLE lane_devices (
    account_id text NOT NULL REFERENCES lane_accounts(account_id),
    id text NOT NULL,
    name text NOT NULL,
    last_sync_at timestamptz,
    disconnected_at timestamptz,
    PRIMARY KEY(account_id,id)
);
CREATE TABLE lane_sessions (
    id text PRIMARY KEY,
    account_id text NOT NULL REFERENCES lane_accounts(account_id),
    device_id text NOT NULL,
    client_id text NOT NULL,
    origin_kind text NOT NULL,
    resource text NOT NULL,
    issuer text NOT NULL,
    csrf_hash text NOT NULL,
    redirect_uri text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    revoked_at timestamptz,
    FOREIGN KEY(account_id,device_id) REFERENCES lane_devices(account_id,id)
);
CREATE INDEX lane_sessions_account ON lane_sessions(account_id,id);
CREATE TABLE lane_credentials (
    digest text PRIMARY KEY,
    session_id text NOT NULL REFERENCES lane_sessions(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK(kind IN ('access','refresh','cookie')),
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz
);
CREATE INDEX lane_credentials_session ON lane_credentials(session_id);
CREATE TABLE lane_auth_flows (
    state_hash text PRIMARY KEY,
    browser_hash text NOT NULL,
    nonce text NOT NULL,
    verifier text NOT NULL,
    request jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz
);
CREATE TABLE lane_auth_codes (
    digest text PRIMARY KEY,
    session_id text NOT NULL REFERENCES lane_sessions(id) ON DELETE CASCADE,
    redirect_uri text NOT NULL,
    challenge text NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz
);
CREATE TABLE lane_auth_consents (
    digest text PRIMARY KEY,
    browser_hash text NOT NULL,
    account_id text NOT NULL,
    tenant_id text NOT NULL,
    request jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz
);
CREATE TABLE lane_reauth_challenges (
    id text PRIMARY KEY,
    account_id text NOT NULL REFERENCES lane_accounts(account_id),
    session_id text NOT NULL REFERENCES lane_sessions(id),
    mode text NOT NULL CHECK(mode IN ('DATA_DELETION_MODE_DELETE_EVERYWHERE','DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL,
    authenticated_at timestamptz,
    proof_hash text UNIQUE,
    proof_expires_at timestamptz,
    consumed_at timestamptz
);
CREATE INDEX lane_reauth_account ON lane_reauth_challenges(account_id);
-- Shared database counters bound auth abuse across replicas, including unauthenticated requests.
CREATE TABLE lane_auth_limits (
    bucket text NOT NULL,
    minute bigint NOT NULL,
    count bigint NOT NULL,
    PRIMARY KEY(bucket,minute)
);
ALTER TABLE lane_sessions ADD COLUMN epoch text NOT NULL;
CREATE TABLE lane_account_deletions (
    account_id text NOT NULL REFERENCES lane_accounts(account_id),
    old_epoch text NOT NULL,
    new_epoch text NOT NULL,
    mode text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(account_id,old_epoch)
);
