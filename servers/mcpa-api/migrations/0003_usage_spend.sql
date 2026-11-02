-- SPDX-License-Identifier: FSL-1.1-ALv2
--
-- Budget spend entries: committed bytes per call start instant.
--
-- Fix-forward only: never edit an applied migration or add reverse migrations.
--
-- Budgets measure the bytes of calls that started in the tenant's Delcore
-- billing period. Fully covered UTC days come from usage_daily. The partial
-- first and last days of a period need exact call start times, and
-- call_records expire 30 days after the call start, before a 31-day monthly
-- period ends. usage_spend keeps only the tenant, resource, call start
-- instant and committed bytes, and is pruned 35 days after the call start,
-- which outlives every Delcore billing period (at most one month).
CREATE TABLE usage_spend (
    tenant_id text NOT NULL,
    resource_id uuid NOT NULL,
    start_time timestamptz NOT NULL,
    bytes bigint NOT NULL CHECK (bytes > 0),
    PRIMARY KEY (tenant_id, resource_id, start_time)
);
CREATE INDEX usage_spend_tenant_start_idx ON usage_spend (tenant_id, start_time);
CREATE INDEX usage_spend_start_idx ON usage_spend (start_time);

-- Retained call records carry the committed bytes of their calls.
INSERT INTO usage_spend (tenant_id, resource_id, start_time, bytes)
SELECT tenant_id, resource_id, start_time, sum(request_bytes + response_bytes)
FROM call_records
GROUP BY tenant_id, resource_id, start_time
HAVING sum(request_bytes + response_bytes) > 0;
