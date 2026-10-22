-- SPDX-License-Identifier: FSL-1.1-ALv2
-- DeliTools persists only usage delivery state. It never stores submitted text, schemas,
-- outputs or credentials. A row exists from the moment provider usage is confirmed until
-- Delcore accepts the event, the tenant is reported DELETED, or 30 days after first creation.
CREATE TABLE usage_outbox (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL,
    request_id uuid NOT NULL,
    attempt integer NOT NULL CHECK (attempt >= 1),
    direction text NOT NULL CHECK (direction IN ('in', 'out')),
    -- Delcore event identity, stable across delivery retries: <request_id>:<attempt>:in|out.
    event_key text NOT NULL UNIQUE,
    -- Delcore request key (UUID v7) reused by every delivery retry of this row.
    delivery_key uuid NOT NULL,
    meter_key text NOT NULL,
    quantity bigint NOT NULL CHECK (quantity > 0),
    -- Initial execution start, preserved across execution attempts.
    occurred_at timestamptz NOT NULL,
    state text NOT NULL CHECK (state IN ('pending', 'poisoned')),
    delivery_attempts integer NOT NULL DEFAULT 0 CHECK (delivery_attempts >= 0),
    next_attempt_at timestamptz NOT NULL,
    last_attempt_at timestamptz,
    -- Bounded category such as unavailable or identity_conflict; never a message body.
    last_failure text,
    created_at timestamptz NOT NULL,
    -- 30 days after first creation. Retries and state changes never move it.
    expires_at timestamptz NOT NULL,
    CHECK (expires_at = created_at + interval '30 days')
);
CREATE INDEX usage_outbox_due ON usage_outbox (next_attempt_at) WHERE state = 'pending';
CREATE INDEX usage_outbox_expiry ON usage_outbox (expires_at);
CREATE INDEX usage_outbox_tenant ON usage_outbox (tenant_id);

CREATE FUNCTION usage_outbox_keep_identity() RETURNS trigger AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id
        OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.event_key IS DISTINCT FROM OLD.event_key
        OR NEW.delivery_key IS DISTINCT FROM OLD.delivery_key
        OR NEW.meter_key IS DISTINCT FROM OLD.meter_key
        OR NEW.quantity IS DISTINCT FROM OLD.quantity
        OR NEW.occurred_at IS DISTINCT FROM OLD.occurred_at
        OR NEW.created_at IS DISTINCT FROM OLD.created_at
        OR NEW.expires_at IS DISTINCT FROM OLD.expires_at THEN
        RAISE EXCEPTION 'usage_outbox identity, content and expiry are immutable';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER usage_outbox_keep_identity
    BEFORE UPDATE ON usage_outbox
    FOR EACH ROW EXECUTE FUNCTION usage_outbox_keep_identity();
