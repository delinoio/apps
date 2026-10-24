-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Resumable position of the backlog sweep that recovers missed free calendar-month
-- allowance baselines for periods that were rated before recovery existed. The single row holds
-- the last (tenant, period start) examined in ascending order. Periods rated later are
-- recovered in the transaction that rates them, so the position only ever moves forward.
-- It is advanced in the transaction that examined its batch: a rollback keeps the work for retry.
CREATE TABLE allowance_recovery_cursor (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 tenant_id uuid NOT NULL,
 period_start timestamptz NOT NULL,
 updated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO allowance_recovery_cursor(singleton,tenant_id,period_start)
 VALUES(true,'00000000-0000-0000-0000-000000000000','1970-01-01T00:00:00Z');

-- The sweep reads rated periods in (tenant, period start) order, a bounded batch at a time.
CREATE INDEX metering_rated_periods_tenant_period ON metering_rated_periods(tenant_id,period_start,period_end);
