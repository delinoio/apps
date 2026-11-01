-- SPDX-License-Identifier: FSL-1.1-ALv2
--
-- MCP OAuth broker state of mcpa-api.
--
-- MCP clients authorize through the broker at /oauth/*. The broker runs its
-- own Logto authorization code flow and keeps the upstream Logto refresh
-- token envelope-encrypted in mcp_grants. Clients receive the Logto access
-- token for the MCP-only audience and an opaque refresh handle. Every bearer
-- value is stored only as a SHA-256 digest.
--
-- 0002 is reserved for the data plane layer; see store.reservedMigrations.
-- Fix-forward only: never edit an applied migration or add reverse migrations.

-- Pending authorizations between /oauth/authorize and /oauth/callback. The
-- primary key is the digest of the upstream Logto state; browser_hash binds
-- the flow to the browser that started it. Rows are deleted on use and
-- expire after 10 minutes.
CREATE TABLE mcp_authorization_requests (
    state_hash bytea PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    browser_hash bytea NOT NULL CHECK (octet_length(browser_hash) = 32),
    client_id text NOT NULL,
    redirect_uri text NOT NULL,
    -- client_state is the MCP client's opaque state, returned unchanged.
    client_state text NOT NULL,
    -- code_challenge is the MCP client's S256 PKCE challenge.
    code_challenge text NOT NULL,
    -- code_verifier and nonce belong to the upstream Logto flow.
    code_verifier text NOT NULL,
    nonce text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    expire_time timestamptz NOT NULL
);
CREATE INDEX mcp_authorization_requests_expire_idx ON mcp_authorization_requests (expire_time);

-- Durable broker grants: one per completed MCP login, bound to the client,
-- the Delcore internal user and the Logto subject. The sealed payload holds
-- the upstream Logto refresh token and the current Delcore-audience access
-- token, with additional data bound to id. Revocation deletes the row.
CREATE TABLE mcp_grants (
    id uuid PRIMARY KEY,
    client_id text NOT NULL,
    user_id text NOT NULL,
    subject text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    update_time timestamptz NOT NULL DEFAULT now(),
    -- expire_time is an idle limit that each token issuance extends.
    expire_time timestamptz NOT NULL,
    key_id text NOT NULL,
    wrapped_key bytea NOT NULL,
    key_nonce bytea NOT NULL,
    nonce bytea NOT NULL,
    ciphertext bytea NOT NULL
);
CREATE INDEX mcp_grants_expire_idx ON mcp_grants (expire_time);
CREATE INDEX mcp_grants_key_id_idx ON mcp_grants (key_id);

-- One-use authorization codes. use_time is set on redemption; a second
-- redemption revokes the grant. Rows expire after one minute.
CREATE TABLE mcp_authorization_codes (
    code_hash bytea PRIMARY KEY CHECK (octet_length(code_hash) = 32),
    grant_id uuid NOT NULL REFERENCES mcp_grants (id) ON DELETE CASCADE,
    client_id text NOT NULL,
    redirect_uri text NOT NULL,
    code_challenge text NOT NULL,
    create_time timestamptz NOT NULL DEFAULT now(),
    expire_time timestamptz NOT NULL,
    use_time timestamptz
);
CREATE INDEX mcp_authorization_codes_grant_idx ON mcp_authorization_codes (grant_id);
CREATE INDEX mcp_authorization_codes_expire_idx ON mcp_authorization_codes (expire_time);

-- Opaque refresh handles. Each refresh retires the presented handle and
-- issues a new one; presenting a retired handle revokes the grant. Handles
-- live as long as their grant.
CREATE TABLE mcp_refresh_handles (
    handle_hash bytea PRIMARY KEY CHECK (octet_length(handle_hash) = 32),
    grant_id uuid NOT NULL REFERENCES mcp_grants (id) ON DELETE CASCADE,
    create_time timestamptz NOT NULL DEFAULT now(),
    retire_time timestamptz
);
CREATE INDEX mcp_refresh_handles_grant_idx ON mcp_refresh_handles (grant_id);

-- Digests of the MCP-audience access tokens issued for a grant, so /mcp
-- maps a verified token to its grant and revocation takes effect before the
-- token expires.
CREATE TABLE mcp_access_tokens (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    grant_id uuid NOT NULL REFERENCES mcp_grants (id) ON DELETE CASCADE,
    expire_time timestamptz NOT NULL
);
CREATE INDEX mcp_access_tokens_grant_idx ON mcp_access_tokens (grant_id);
CREATE INDEX mcp_access_tokens_expire_idx ON mcp_access_tokens (expire_time);
