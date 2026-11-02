-- SPDX-License-Identifier: FSL-1.1-ALv2
--
-- Core management schema of mcpa-api.
--
-- Migration numbers 0002 and later are reserved for later server layers:
--   0002: data plane (call records, daily aggregates, metering outbox).
--   0003: MCP OAuth broker grants.
-- Fix-forward only: never edit an applied migration or add reverse migrations.
--
-- Tenant, user and group IDs are Delcore UUIDs stored as text so mcpa never
-- reinterprets Delcore identities. mcpa-owned IDs are UUID v7.

CREATE TABLE resources (
    id uuid PRIMARY KEY,
    tenant_id text NOT NULL,
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 100),
    description text NOT NULL DEFAULT '' CHECK (octet_length(description) <= 1000),
    created_by_user_id text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    update_time timestamptz NOT NULL DEFAULT now(),
    -- delete_time marks a read-only tombstone. Credentials, settings, OAuth
    -- flows and call keys are deleted immediately; the row and its ACL stay so
    -- managers can still read retained usage, error and audit records.
    delete_time timestamptz
);
CREATE INDEX resources_tenant_idx ON resources (tenant_id, id) WHERE delete_time IS NULL;
-- Live resource names are unique per tenant.
CREATE UNIQUE INDEX resources_tenant_name_idx ON resources (tenant_id, name) WHERE delete_time IS NULL;

-- load_balancing and other enums store the mcpa.v1 enum numbers.
CREATE TABLE resource_routing (
    resource_id uuid PRIMARY KEY REFERENCES resources (id) ON DELETE CASCADE,
    load_balancing smallint NOT NULL,
    model_allowlist text[] NOT NULL DEFAULT '{}',
    update_time timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE resource_acl (
    resource_id uuid NOT NULL REFERENCES resources (id) ON DELETE CASCADE,
    principal_kind smallint NOT NULL CHECK (principal_kind IN (1, 2)),
    principal_id text NOT NULL,
    role smallint NOT NULL CHECK (role IN (1, 2)),
    created_by_user_id text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (resource_id, principal_kind, principal_id)
);
CREATE INDEX resource_acl_principal_idx ON resource_acl (principal_kind, principal_id);

-- Delcore offers registered services no machine-to-machine group membership
-- read. mcpa keeps the latest ListGroups result seen by a user session of the
-- tenant and resolves group ACL entries through it.
CREATE TABLE tenant_group_snapshots (
    tenant_id text PRIMARY KEY,
    refresh_time timestamptz NOT NULL
);

CREATE TABLE tenant_groups (
    tenant_id text NOT NULL REFERENCES tenant_group_snapshots (tenant_id) ON DELETE CASCADE,
    group_id text NOT NULL,
    name text NOT NULL,
    PRIMARY KEY (tenant_id, group_id)
);

CREATE TABLE tenant_group_members (
    tenant_id text NOT NULL,
    group_id text NOT NULL,
    user_id text NOT NULL,
    PRIMARY KEY (tenant_id, group_id, user_id),
    FOREIGN KEY (tenant_id, group_id) REFERENCES tenant_groups (tenant_id, group_id) ON DELETE CASCADE
);
CREATE INDEX tenant_group_members_user_idx ON tenant_group_members (tenant_id, user_id);

-- Encrypted provider credentials. The payload is envelope-encrypted; the
-- additional data binds id and resource_id.
CREATE TABLE credentials (
    id uuid PRIMARY KEY,
    resource_id uuid NOT NULL REFERENCES resources (id) ON DELETE CASCADE,
    tenant_id text NOT NULL,
    provider smallint NOT NULL,
    kind smallint NOT NULL CHECK (kind IN (1, 2)),
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 100),
    account_label text NOT NULL DEFAULT '',
    secret_hint text NOT NULL DEFAULT '',
    base_url text NOT NULL DEFAULT '',
    models text[] NOT NULL DEFAULT '{}',
    created_by_user_id text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    update_time timestamptz NOT NULL DEFAULT now(),
    key_id text NOT NULL,
    wrapped_key bytea NOT NULL,
    key_nonce bytea NOT NULL,
    nonce bytea NOT NULL,
    ciphertext bytea NOT NULL
);
CREATE INDEX credentials_resource_idx ON credentials (resource_id, id);
CREATE INDEX credentials_key_id_idx ON credentials (key_id);

-- Provider OAuth flows. Flow secrets (PKCE verifier, state, device code) are
-- envelope-encrypted; the data plane layer implements the flow itself.
CREATE TABLE provider_oauth_flows (
    id uuid PRIMARY KEY,
    resource_id uuid NOT NULL REFERENCES resources (id) ON DELETE CASCADE,
    tenant_id text NOT NULL,
    provider smallint NOT NULL,
    kind smallint NOT NULL CHECK (kind IN (1, 2)),
    state smallint NOT NULL CHECK (state IN (1, 2, 3, 4)),
    name text NOT NULL,
    authorization_url text NOT NULL DEFAULT '',
    user_code text NOT NULL DEFAULT '',
    verification_uri text NOT NULL DEFAULT '',
    expire_time timestamptz NOT NULL,
    credential_id uuid,
    failure_message text NOT NULL DEFAULT '',
    created_by_user_id text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    update_time timestamptz NOT NULL DEFAULT now(),
    key_id text NOT NULL,
    wrapped_key bytea NOT NULL,
    key_nonce bytea NOT NULL,
    nonce bytea NOT NULL,
    ciphertext bytea NOT NULL
);
CREATE INDEX provider_oauth_flows_resource_idx ON provider_oauth_flows (resource_id, id);
CREATE INDEX provider_oauth_flows_key_id_idx ON provider_oauth_flows (key_id);
CREATE INDEX provider_oauth_flows_expire_idx ON provider_oauth_flows (expire_time);

-- Call keys store only the SHA-256 digest of the full secret.
CREATE TABLE call_keys (
    id uuid PRIMARY KEY,
    resource_id uuid NOT NULL REFERENCES resources (id) ON DELETE CASCADE,
    tenant_id text NOT NULL,
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 100),
    secret_hash bytea NOT NULL UNIQUE CHECK (octet_length(secret_hash) = 32),
    issuer_user_id text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    expire_time timestamptz,
    last_used_time timestamptz,
    revoke_time timestamptz
);
CREATE INDEX call_keys_resource_idx ON call_keys (resource_id, id);
CREATE INDEX call_keys_issuer_idx ON call_keys (resource_id, issuer_user_id, id);

CREATE TABLE resource_budgets (
    resource_id uuid PRIMARY KEY REFERENCES resources (id) ON DELETE CASCADE,
    limit_usd numeric NOT NULL CHECK (limit_usd >= 0),
    updated_by_user_id text NOT NULL,
    update_time timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tenant_budgets (
    tenant_id text PRIMARY KEY,
    limit_usd numeric NOT NULL CHECK (limit_usd >= 0),
    updated_by_user_id text NOT NULL,
    update_time timestamptz NOT NULL DEFAULT now()
);

-- Audit events are kept for 13 months. resource_id has no foreign key so
-- events outlive resource deletion.
CREATE TABLE audit_events (
    id uuid PRIMARY KEY,
    tenant_id text NOT NULL,
    resource_id uuid,
    action smallint NOT NULL,
    actor_kind smallint NOT NULL,
    actor_user_id text NOT NULL DEFAULT '',
    target_id text NOT NULL DEFAULT '',
    details jsonb NOT NULL DEFAULT '{}',
    occur_time timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_events_resource_idx ON audit_events (resource_id, id DESC);
CREATE INDEX audit_events_tenant_idx ON audit_events (tenant_id, id DESC);
CREATE INDEX audit_events_occur_idx ON audit_events (occur_time);

-- Idempotency records expire 30 days after creation. A replay with the same
-- fingerprint returns the stored result; another fingerprint conflicts.
CREATE TABLE idempotency_records (
    operation text NOT NULL,
    principal_id text NOT NULL,
    request_key uuid NOT NULL,
    fingerprint bytea NOT NULL,
    result_id uuid NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    expire_time timestamptz NOT NULL,
    PRIMARY KEY (operation, principal_id, request_key)
);
CREATE INDEX idempotency_records_expire_idx ON idempotency_records (expire_time);

-- Console login flows hold the PKCE verifier and nonce between /auth/login
-- and /auth/callback. They are deleted on use and expire after 10 minutes.
CREATE TABLE console_login_flows (
    state_hash bytea PRIMARY KEY,
    browser_hash bytea NOT NULL,
    code_verifier text NOT NULL,
    nonce text NOT NULL,
    return_to text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    expire_time timestamptz NOT NULL
);
CREATE INDEX console_login_flows_expire_idx ON console_login_flows (expire_time);

-- Console sessions. The cookie value is stored only as a SHA-256 digest;
-- Logto tokens are envelope-encrypted with additional data bound to id.
CREATE TABLE console_sessions (
    id uuid PRIMARY KEY,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    user_id text NOT NULL,
    subject text NOT NULL,
    csrf_token text NOT NULL,
    mfa boolean NOT NULL,
    access_expire_time timestamptz NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    update_time timestamptz NOT NULL DEFAULT now(),
    expire_time timestamptz NOT NULL,
    key_id text NOT NULL,
    wrapped_key bytea NOT NULL,
    key_nonce bytea NOT NULL,
    nonce bytea NOT NULL,
    ciphertext bytea NOT NULL
);
CREATE INDEX console_sessions_expire_idx ON console_sessions (expire_time);
CREATE INDEX console_sessions_key_id_idx ON console_sessions (key_id);
