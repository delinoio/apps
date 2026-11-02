-- SPDX-License-Identifier: FSL-1.1-ALv2
--
-- Data plane schema of mcpa-api: call records, daily usage aggregates and
-- the Delcore usage outbox.
--
-- Fix-forward only: never edit an applied migration or add reverse migrations.
--
-- None of these tables stores request or response bodies, call key secrets,
-- provider tokens or API keys. resource_id has no foreign key: records
-- outlive resource deletion and stay until their own retention ends.

-- Request-level call records hold metadata only and expire 30 days after the
-- call start. A record is written by the first metering checkpoint and
-- updated by later ones; checkpoint_seq is the last committed checkpoint, so
-- a resent checkpoint is recognized and applied once.
CREATE TABLE call_records (
    id uuid PRIMARY KEY,
    tenant_id text NOT NULL,
    resource_id uuid NOT NULL,
    call_key_id uuid NOT NULL,
    credential_id uuid,
    provider smallint NOT NULL DEFAULT 0,
    model text NOT NULL DEFAULT '' CHECK (octet_length(model) <= 200),
    format smallint NOT NULL,
    streaming boolean NOT NULL DEFAULT false,
    status smallint NOT NULL DEFAULT 0,
    provider_status_class smallint NOT NULL DEFAULT 0,
    error_class text NOT NULL DEFAULT '' CHECK (octet_length(error_class) <= 64),
    request_bytes bigint NOT NULL DEFAULT 0 CHECK (request_bytes >= 0),
    response_bytes bigint NOT NULL DEFAULT 0 CHECK (response_bytes >= 0),
    latency_ms bigint NOT NULL DEFAULT 0 CHECK (latency_ms >= 0),
    checkpoint_seq bigint NOT NULL CHECK (checkpoint_seq >= 1),
    final boolean NOT NULL DEFAULT false,
    start_time timestamptz NOT NULL,
    expire_time timestamptz NOT NULL,
    update_time timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX call_records_resource_idx ON call_records (resource_id, id DESC);
CREATE INDEX call_records_resource_start_idx ON call_records (resource_id, start_time);
CREATE INDEX call_records_call_key_idx ON call_records (call_key_id, id DESC);
CREATE INDEX call_records_tenant_start_idx ON call_records (tenant_id, start_time);
CREATE INDEX call_records_expire_idx ON call_records (expire_time);

-- Daily usage aggregates (UTC day of the call start) are kept for 13 months.
-- status 0 (CALL_STATUS_UNSPECIFIED) holds the bytes of calls whose final
-- checkpoint has not committed yet; the final checkpoint moves them to the
-- call's final status. call_count counts finished calls only.
CREATE TABLE usage_daily (
    id uuid PRIMARY KEY,
    day date NOT NULL,
    tenant_id text NOT NULL,
    resource_id uuid NOT NULL,
    model text NOT NULL CHECK (octet_length(model) <= 200),
    provider smallint NOT NULL,
    call_key_id uuid NOT NULL,
    status smallint NOT NULL,
    call_count bigint NOT NULL DEFAULT 0 CHECK (call_count >= 0),
    request_bytes bigint NOT NULL DEFAULT 0 CHECK (request_bytes >= 0),
    response_bytes bigint NOT NULL DEFAULT 0 CHECK (response_bytes >= 0),
    update_time timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT usage_daily_key UNIQUE (day, tenant_id, resource_id, model, provider, call_key_id, status)
);
CREATE INDEX usage_daily_resource_day_idx ON usage_daily (resource_id, day DESC, id DESC);
CREATE INDEX usage_daily_tenant_day_idx ON usage_daily (tenant_id, day);
CREATE INDEX usage_daily_day_idx ON usage_daily (day);

-- The usage outbox holds one Delcore traffic event per non-zero metering
-- checkpoint. The event key mcpa:<call_id>:<checkpoint_seq> and the Delcore
-- request key are fixed at insertion and reused by every delivery. Delivered
-- rows are deleted. A row expires 30 days after creation; poisoned rows are
-- never sent again and stay until expiry.
CREATE TABLE usage_outbox (
    id uuid PRIMARY KEY,
    event_key text NOT NULL CHECK (octet_length(event_key) BETWEEN 1 AND 200),
    request_key uuid NOT NULL,
    tenant_id text NOT NULL,
    resource_id uuid NOT NULL,
    -- occurred_at is the call start at microsecond precision.
    occurred_at timestamptz NOT NULL,
    -- bytes is the checkpoint delta (request + response bytes).
    bytes bigint NOT NULL CHECK (bytes > 0),
    -- 1 = pending, 2 = poisoned.
    state smallint NOT NULL DEFAULT 1 CHECK (state IN (1, 2)),
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz NOT NULL,
    last_attempt_at timestamptz,
    -- last_error is a bounded failure category, never a Delcore body.
    last_error text NOT NULL DEFAULT '' CHECK (octet_length(last_error) <= 64),
    create_time timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    CONSTRAINT usage_outbox_event_key UNIQUE (event_key),
    CONSTRAINT usage_outbox_expiry CHECK (expires_at = create_time + interval '30 days')
);
CREATE INDEX usage_outbox_due_idx ON usage_outbox (next_attempt_at, id) WHERE state = 1;
CREATE INDEX usage_outbox_expire_idx ON usage_outbox (expires_at);
CREATE INDEX usage_outbox_tenant_idx ON usage_outbox (tenant_id);
CREATE INDEX usage_outbox_resource_idx ON usage_outbox (resource_id);

-- Identity, content and expiry of an outbox row never change; only the
-- delivery state columns do.
CREATE FUNCTION usage_outbox_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id
        OR NEW.event_key IS DISTINCT FROM OLD.event_key
        OR NEW.request_key IS DISTINCT FROM OLD.request_key
        OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.resource_id IS DISTINCT FROM OLD.resource_id
        OR NEW.occurred_at IS DISTINCT FROM OLD.occurred_at
        OR NEW.bytes IS DISTINCT FROM OLD.bytes
        OR NEW.create_time IS DISTINCT FROM OLD.create_time
        OR NEW.expires_at IS DISTINCT FROM OLD.expires_at THEN
        RAISE EXCEPTION 'usage_outbox identity, content and expiry are immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER usage_outbox_immutable BEFORE UPDATE ON usage_outbox
    FOR EACH ROW EXECUTE FUNCTION usage_outbox_immutable();

-- Provider OAuth flows (table from 0001) are pruned one day after they end.
CREATE INDEX provider_oauth_flows_update_idx ON provider_oauth_flows (update_time);
-- Tombstone cleanup and tenant lifecycle scans.
CREATE INDEX resources_deleted_idx ON resources (delete_time) WHERE delete_time IS NOT NULL;
CREATE INDEX resources_tenant_all_idx ON resources (tenant_id);
CREATE INDEX audit_events_tenant_level_idx ON audit_events (tenant_id) WHERE resource_id IS NULL;
