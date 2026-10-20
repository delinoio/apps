-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Price evidence follows accepted usage retention, never consumer pending-record TTL.
CREATE TABLE ai_usage_prices (
 usage_id uuid PRIMARY KEY REFERENCES usage_events(id) ON DELETE CASCADE,
 details jsonb NOT NULL,
 cost numeric NOT NULL CHECK(cost >= 0)
);
