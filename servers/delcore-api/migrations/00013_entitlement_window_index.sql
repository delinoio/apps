-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Billing-period resolution runs for every event ingestion, metered authorization and
-- aggregate query. It asks whether a tenant held a recurring add-on entitlement at an
-- instant. Closed entitlement rows are retained across subscription changes, so look the
-- tenant's window up by index instead of scanning all entitlements. The price is included so
-- the lookup can join prices without a heap fetch per entitlement row.
CREATE INDEX entitlements_tenant_window ON entitlements(tenant_id,starts_at,ends_at) INCLUDE(price_id);
