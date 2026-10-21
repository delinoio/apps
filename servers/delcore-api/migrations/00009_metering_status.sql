-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Unfinished metering work for GetMeteringStatus. Completed metering jobs are
-- retained, so tenant and service-wide status reads must not scan them.
CREATE INDEX jobs_metering_unfinished ON jobs(tenant_id,created_at) WHERE kind='metering' AND state<>'done';
