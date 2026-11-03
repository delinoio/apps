-- SPDX-License-Identifier: FSL-1.1-ALv2

-- Verified GitHub webhook deliveries. The webhook route stores a delivery here and answers 204;
-- the maintenance cycle applies unapplied rows in receipt order. `delivery_id` is the
-- `X-GitHub-Delivery` header, so a redelivery conflicts and is ignored. Applied rows are deleted
-- after the audit retention.
CREATE TABLE git_tracker_webhook_events (
 id uuid PRIMARY KEY,
 delivery_id text NOT NULL UNIQUE,
 event_type text NOT NULL,
 action text NOT NULL,
 installation_id text NOT NULL,
 github_id bigint NOT NULL,
 removed_ids bigint[] NOT NULL DEFAULT '{}',
 received_at timestamptz NOT NULL,
 applied_at timestamptz
);

CREATE INDEX git_tracker_webhook_events_pending ON git_tracker_webhook_events(received_at, id) WHERE applied_at IS NULL;
CREATE INDEX git_tracker_webhook_events_applied ON git_tracker_webhook_events(applied_at) WHERE applied_at IS NOT NULL;
