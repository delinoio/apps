-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Unfinished metering work for GetMeteringStatus. Completed metering jobs are
-- retained, so tenant and service-wide status reads must not scan them.
CREATE INDEX jobs_metering_unfinished ON jobs(tenant_id,created_at) WHERE kind='metering' AND state<>'done';

-- The time of a job's last recorded metering failure. RetryJob rewrites
-- updated_at, so status reads take the latest failure time from this column.
-- Earlier recorded failures keep their last update time only while the
-- failure is the job's last write: the worker increments attempts when it
-- records a failure, and RetryJob resets attempts to 0 while it keeps the
-- category, so a retried job's updated_at is the retry time and stays unset.
ALTER TABLE jobs ADD COLUMN failed_at timestamptz;
UPDATE jobs SET failed_at=updated_at WHERE kind='metering' AND error_category<>'' AND attempts>0;
