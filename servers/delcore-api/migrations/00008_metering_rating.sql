-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Usage price of each evaluated meter, attributed once from the tenant's
-- entitlements at the event's occurrence time when the original event is
-- accepted. Corrections keep this attribution. A meter without an applicable
-- usage price has no row; its contributions are metered but not rated.
CREATE TABLE metering_event_prices (
 event_id uuid NOT NULL,
 tenant_id uuid NOT NULL,
 meter_id uuid NOT NULL REFERENCES catalog_entities(id),
 price_id uuid NOT NULL REFERENCES prices(id),
 PRIMARY KEY(event_id,meter_id),
 FOREIGN KEY(event_id,tenant_id) REFERENCES metering_events(id,tenant_id)
);
CREATE TRIGGER metering_event_prices_immutable BEFORE UPDATE OR DELETE ON metering_event_prices
 FOR EACH ROW EXECUTE FUNCTION metering_history_record();

-- Latest exact rating of each tenant meter billing period that processing has
-- rated. The ledger stays the authoritative record of charged amounts; this
-- row names the periods that final currency rounding revisits.
CREATE TABLE metering_rated_periods (
 tenant_id uuid NOT NULL REFERENCES tenants(id),
 meter_id uuid NOT NULL REFERENCES catalog_entities(id),
 service_id uuid NOT NULL REFERENCES services(id),
 meter text NOT NULL,
 period_start timestamptz NOT NULL,
 period_end timestamptz NOT NULL,
 charge numeric NOT NULL CHECK(charge>=0),
 remaining_included numeric NOT NULL CHECK(remaining_included>=0),
 rated_at timestamptz NOT NULL,
 PRIMARY KEY(tenant_id,meter_id,period_start),
 CHECK(period_end>period_start)
);
CREATE INDEX metering_rated_periods_end ON metering_rated_periods(period_end);

-- Rating scans the applied contributions of one tenant billing period.
CREATE INDEX metering_events_period ON metering_events(tenant_id,period_start);

-- Rating loads the allowance grants of one tenant service meter period on
-- every rated transition and paid authorization.
CREATE INDEX allowance_grants_period ON allowance_grants(tenant_id,service_id,meter,period_start,applied_at);
