-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Execution-side schema: public catalog price versions, request state, attempts with usage and
-- delivery state, and idempotency records. The execution pipeline fills these tables; the
-- foundation only creates and retains them. No table stores input text, JSON schemas, results or
-- credentials. Tenant rate and concurrency state is in memory (single instance) and not persisted.
--
-- Retention: requests, attempts and idempotency_records expire independently 30 days after their
-- own immutable created_at, in every state. There are deliberately no foreign keys between them:
-- deleting one record must never delete another record early or hold it past its own expiry.

-- Public Vercel AI Gateway catalog prices, versioned by effective time. Changed prices apply to
-- requests that start after effective_at; running requests keep their frozen snapshot.
CREATE TABLE catalog_prices (
    id                       uuid PRIMARY KEY,
    -- Gateway model identifier, for example provider/model-name.
    model                    text NOT NULL CHECK (model <> ''),
    -- Provider route the price applies to. Empty means the model's default route.
    provider_route           text NOT NULL DEFAULT '',
    effective_at             timestamptz NOT NULL,
    input_usd_per_token      numeric NOT NULL CHECK (input_usd_per_token >= 0),
    output_usd_per_token     numeric NOT NULL CHECK (output_usd_per_token >= 0),
    -- NULL means the category has no published price; usage needing it stays pending.
    cache_read_usd_per_token numeric CHECK (cache_read_usd_per_token >= 0),
    cache_write_usd_per_token numeric CHECK (cache_write_usd_per_token >= 0),
    -- Opaque digest of the source catalog entry, to detect changes between synchronizations.
    catalog_revision         text NOT NULL DEFAULT '',
    synced_at                timestamptz NOT NULL,
    created_at               timestamptz NOT NULL,
    UNIQUE (model, provider_route, effective_at)
);
CREATE INDEX catalog_prices_lookup ON catalog_prices (model, provider_route, effective_at DESC);

-- capability: 1 translator, 2 extractor. state: 1 processing, 2 completed, 3 unknown.
-- error_reason: delitools.v1.ErrorReason value of a definitive failure, 0 for success or unknown.
-- credential_kind: 1 Logto user token, 2 personal key, 3 service key.
CREATE TABLE requests (
    id                 uuid PRIMARY KEY,
    tenant_id          uuid NOT NULL,
    capability         smallint NOT NULL CHECK (capability IN (1, 2)),
    state              smallint NOT NULL CHECK (state IN (1, 2, 3)),
    error_reason       smallint NOT NULL DEFAULT 0 CHECK (error_reason BETWEEN 0 AND 18),
    model              text NOT NULL,
    credential_kind    smallint NOT NULL CHECK (credential_kind IN (1, 2, 3)),
    api_key_id         uuid,
    user_id            uuid,
    -- Price version frozen at the initial request start and used by every retry attempt.
    price_id           uuid,
    price_effective_at timestamptz,
    created_at         timestamptz NOT NULL,
    updated_at         timestamptz NOT NULL,
    completed_at       timestamptz
);
CREATE TRIGGER requests_created_at BEFORE UPDATE ON requests
    FOR EACH ROW EXECUTE FUNCTION forbid_created_at_change();
CREATE INDEX requests_created ON requests (created_at);
CREATE INDEX requests_tenant_created ON requests (tenant_id, created_at);
CREATE INDEX requests_unresolved ON requests (state) WHERE state IN (1, 3);

-- One AI call. The attempt UUID is the durable usage identity: it is the delcore event key and
-- request key, and the safe user reference sent to the Gateway. Each attempt carries what delivery
-- and reconciliation need, so it never depends on another table or on the request row surviving.
CREATE TABLE attempts (
    id                 uuid PRIMARY KEY,
    request_id         uuid NOT NULL,
    tenant_id          uuid NOT NULL,
    capability         smallint NOT NULL CHECK (capability IN (1, 2)),
    model              text NOT NULL,
    provider_route     text NOT NULL DEFAULT '',
    -- External Gateway generation ID. Empty until known or associated by an operator.
    generation_id      text NOT NULL DEFAULT '' CHECK (octet_length(generation_id) <= 256),
    price_effective_at timestamptz NOT NULL,
    -- Frozen public catalog price snapshot (decimal strings).
    catalog_price      jsonb NOT NULL CHECK (jsonb_typeof(catalog_price) = 'object'),
    occurred_at        timestamptz NOT NULL,
    -- Confirmed token counts. NULL while usage is unknown; unknown usage is never estimated.
    measured_usage     jsonb CHECK (measured_usage IS NULL OR jsonb_typeof(measured_usage) = 'object'),
    delivered_at       timestamptz,
    delivery_failures  integer NOT NULL DEFAULT 0 CHECK (delivery_failures >= 0),
    -- Next background delivery or reconciliation attempt.
    retry_at           timestamptz,
    created_at         timestamptz NOT NULL,
    updated_at         timestamptz NOT NULL
);
CREATE TRIGGER attempts_created_at BEFORE UPDATE ON attempts
    FOR EACH ROW EXECUTE FUNCTION forbid_created_at_change();
-- A Gateway generation can be associated with at most one retained attempt.
CREATE UNIQUE INDEX attempts_generation ON attempts (generation_id) WHERE generation_id <> '';
CREATE INDEX attempts_request ON attempts (request_id, created_at, id);
CREATE INDEX attempts_created ON attempts (created_at);
CREATE INDEX attempts_pending ON attempts (retry_at, id) WHERE delivered_at IS NULL;

-- Deduplication scope is tenant + API method + key, across credentials. method: 1 translator,
-- 2 extractor. The key, an input-comparison hash and the processing state are stored; the source
-- and the result are never stored. state mirrors requests.state: 1 processing, 2 completed,
-- 3 unknown.
CREATE TABLE idempotency_records (
    tenant_id       uuid NOT NULL,
    method          smallint NOT NULL CHECK (method IN (1, 2)),
    idempotency_key text NOT NULL CHECK (octet_length(idempotency_key) BETWEEN 1 AND 256),
    request_id      uuid NOT NULL,
    input_hash      bytea NOT NULL CHECK (octet_length(input_hash) = 32),
    state           smallint NOT NULL CHECK (state IN (1, 2, 3)),
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, method, idempotency_key)
);
CREATE TRIGGER idempotency_records_created_at BEFORE UPDATE ON idempotency_records
    FOR EACH ROW EXECUTE FUNCTION forbid_created_at_change();
CREATE INDEX idempotency_records_created ON idempotency_records (created_at);
