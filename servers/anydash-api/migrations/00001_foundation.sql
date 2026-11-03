-- SPDX-License-Identifier: FSL-1.1-ALv2
-- Enum columns store the anydash.v1 protocol numeric values.
CREATE TABLE accounts (
  user_id uuid PRIMARY KEY,
  logto_subject text NOT NULL UNIQUE,
  personal_tenant_id uuid NOT NULL UNIQUE,
  time_zone text NOT NULL DEFAULT 'UTC',
  preferred_device_id uuid,
  data_deleted_at timestamptz,
  created_at timestamptz NOT NULL,
  updated_at timestamptz NOT NULL
);

CREATE TABLE enrollments (
  id uuid PRIMARY KEY,
  device_code_sha256 text NOT NULL UNIQUE,
  user_code_sha256 text NOT NULL,
  state smallint NOT NULL,
  device_name text NOT NULL,
  platform smallint NOT NULL,
  architecture smallint NOT NULL,
  collector_kind smallint NOT NULL,
  collector_version text NOT NULL,
  os_version text NOT NULL,
  previous_device_id uuid,
  approved_by_user_id uuid,
  device_id uuid,
  failed_attempts integer NOT NULL DEFAULT 0,
  created_at timestamptz NOT NULL,
  expires_at timestamptz NOT NULL,
  approved_at timestamptz,
  consumed_at timestamptz,
  last_polled_at timestamptz
);
CREATE INDEX enrollments_expires_at ON enrollments (expires_at);

CREATE TABLE devices (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL,
  name text NOT NULL,
  platform smallint NOT NULL,
  architecture smallint NOT NULL,
  collector_kind smallint NOT NULL,
  collector_version text NOT NULL,
  os_version text NOT NULL,
  enrolled_at timestamptz NOT NULL,
  last_heartbeat_at timestamptz,
  last_upload_at timestamptz,
  credential_state smallint NOT NULL,
  last_sync_requested_at timestamptz,
  sync_requested boolean NOT NULL DEFAULT false,
  deleted_at timestamptz
);
CREATE INDEX devices_user ON devices (user_id) WHERE deleted_at IS NULL;
CREATE INDEX devices_last_heartbeat_at ON devices (last_heartbeat_at);

CREATE TABLE device_credentials (
  device_id uuid PRIMARY KEY REFERENCES devices (id) ON DELETE CASCADE,
  access_token_sha256 text NOT NULL UNIQUE,
  access_expires_at timestamptz NOT NULL,
  refresh_token_sha256 text NOT NULL UNIQUE,
  refresh_expires_at timestamptz NOT NULL,
  issued_at timestamptz NOT NULL
);

CREATE TABLE device_rotated_refresh_tokens (
  refresh_token_sha256 text PRIMARY KEY,
  device_id uuid NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
  rotated_at timestamptz NOT NULL
);
CREATE INDEX device_rotated_refresh_tokens_rotated_at ON device_rotated_refresh_tokens (rotated_at);

CREATE TABLE device_parser_status (
  device_id uuid NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
  tool smallint NOT NULL,
  state smallint NOT NULL,
  skipped_record_count bigint NOT NULL,
  observed_at timestamptz,
  parser_version text NOT NULL,
  source_path_count integer NOT NULL,
  reported_at timestamptz NOT NULL,
  PRIMARY KEY (device_id, tool)
);
CREATE INDEX device_parser_status_reported_at ON device_parser_status (reported_at);

CREATE TABLE price_versions (
  id uuid PRIMARY KEY,
  effective_from timestamptz NOT NULL UNIQUE,
  content_sha256 text NOT NULL,
  source_url text NOT NULL,
  fetched_at timestamptz NOT NULL,
  model_count integer NOT NULL
);

CREATE TABLE model_prices (
  price_version_id uuid NOT NULL REFERENCES price_versions (id) ON DELETE CASCADE,
  model text NOT NULL,
  input_cost_per_token numeric,
  output_cost_per_token numeric,
  cache_read_cost_per_token numeric,
  cache_write_cost_per_token numeric,
  PRIMARY KEY (price_version_id, model)
);

CREATE TABLE price_sync_runs (
  id uuid PRIMARY KEY,
  started_at timestamptz NOT NULL,
  finished_at timestamptz,
  outcome smallint NOT NULL,
  failure_category text NOT NULL DEFAULT '',
  price_version_id uuid
);
CREATE INDEX price_sync_runs_started_at ON price_sync_runs (started_at);

CREATE TABLE usage_buckets (
  device_id uuid NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
  tool smallint NOT NULL,
  model text NOT NULL,
  bucket_start timestamptz NOT NULL,
  user_id uuid NOT NULL,
  version bigint NOT NULL CHECK (version > 0),
  session_count bigint NOT NULL,
  input_tokens bigint,
  output_tokens bigint,
  cache_read_tokens bigint,
  cache_write_tokens bigint,
  price_version_id uuid,
  estimated_cost_usd numeric(30, 12),
  cost_computable boolean NOT NULL,
  updated_at timestamptz NOT NULL,
  PRIMARY KEY (device_id, tool, model, bucket_start),
  CHECK (extract(epoch FROM bucket_start) % 900 = 0)
);
CREATE INDEX usage_buckets_user_bucket ON usage_buckets (user_id, bucket_start);
CREATE INDEX usage_buckets_bucket_start ON usage_buckets (bucket_start);

CREATE TABLE connections (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL,
  provider smallint NOT NULL,
  display_name text NOT NULL,
  account_label text NOT NULL,
  account_fingerprint text NOT NULL,
  created_at timestamptz NOT NULL,
  lease_epoch bigint NOT NULL DEFAULT 0,
  lease_device_id uuid,
  lease_acquired_at timestamptz,
  lease_expires_at timestamptz,
  last_holder_device_id uuid,
  last_manual_refresh_at timestamptz,
  refresh_requested boolean NOT NULL DEFAULT false,
  latest_observed_at timestamptz,
  last_success_at timestamptz,
  last_status smallint NOT NULL DEFAULT 0,
  last_failure_class smallint NOT NULL DEFAULT 0,
  last_error_correlation_id text NOT NULL DEFAULT '',
  deleted_at timestamptz
);
CREATE INDEX connections_user ON connections (user_id) WHERE deleted_at IS NULL;
CREATE INDEX connections_lease_expires_at ON connections (lease_expires_at);

CREATE TABLE connection_devices (
  connection_id uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
  device_id uuid NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
  registered_at timestamptz NOT NULL,
  ready boolean NOT NULL DEFAULT false,
  ready_reported_at timestamptz,
  PRIMARY KEY (connection_id, device_id)
);
CREATE INDEX connection_devices_device ON connection_devices (device_id);

CREATE TABLE metric_latest (
  connection_id uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
  resource_id text NOT NULL,
  kind smallint NOT NULL,
  dimension text NOT NULL,
  state smallint NOT NULL,
  value jsonb,
  unit text NOT NULL,
  temporality smallint NOT NULL,
  period_start timestamptz,
  period_end timestamptz,
  observed_at timestamptz NOT NULL,
  failure_class smallint NOT NULL DEFAULT 0,
  last_success_value jsonb,
  last_success_at timestamptz,
  PRIMARY KEY (connection_id, resource_id, kind, dimension)
);

CREATE TABLE metric_hourly (
  connection_id uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
  resource_id text NOT NULL,
  kind smallint NOT NULL,
  dimension text NOT NULL,
  hour_start timestamptz NOT NULL,
  value jsonb NOT NULL,
  unit text NOT NULL,
  temporality smallint NOT NULL,
  period_start timestamptz,
  period_end timestamptz,
  observed_at timestamptz NOT NULL,
  PRIMARY KEY (connection_id, resource_id, kind, dimension, hour_start)
);
CREATE INDEX metric_hourly_hour_start ON metric_hourly (hour_start);

CREATE TABLE resources_latest (
  connection_id uuid NOT NULL REFERENCES connections (id) ON DELETE CASCADE,
  resource_id text NOT NULL,
  kind smallint NOT NULL,
  display_name text NOT NULL,
  parent_id text NOT NULL,
  health smallint NOT NULL,
  provider_status text NOT NULL,
  status_state smallint NOT NULL,
  observed_at timestamptz NOT NULL,
  PRIMARY KEY (connection_id, resource_id)
);

CREATE TABLE dashboards (
  id uuid PRIMARY KEY,
  tenant_id uuid NOT NULL,
  space_kind smallint NOT NULL,
  created_by_user_id uuid NOT NULL,
  name text NOT NULL,
  version bigint NOT NULL,
  created_at timestamptz NOT NULL,
  updated_at timestamptz NOT NULL,
  deleted_at timestamptz
);
CREATE INDEX dashboards_tenant ON dashboards (tenant_id) WHERE deleted_at IS NULL;

CREATE TABLE widgets (
  id uuid PRIMARY KEY,
  dashboard_id uuid NOT NULL REFERENCES dashboards (id) ON DELETE CASCADE,
  data_owner_user_id uuid NOT NULL,
  type smallint NOT NULL,
  x integer NOT NULL,
  y integer NOT NULL,
  width integer NOT NULL,
  height integer NOT NULL,
  title text NOT NULL,
  presentation smallint NOT NULL,
  query jsonb NOT NULL,
  all_devices boolean NOT NULL,
  sharing_state smallint NOT NULL,
  sharing_end_reason smallint NOT NULL DEFAULT 0,
  -- The data owner's Delcore membership start (GetTenantMemberRoles member_since) from the role
  -- read when the widget was last shared in a team space; NULL in personal spaces. A later role
  -- read reporting a different start ends sharing with OWNER_LEFT_SPACE.
  data_owner_member_since timestamptz,
  sort_order bigint NOT NULL,
  created_at timestamptz NOT NULL
);
CREATE INDEX widgets_dashboard ON widgets (dashboard_id, sort_order);
CREATE INDEX widgets_data_owner ON widgets (data_owner_user_id);

-- source_kind: 1 connection, 2 device.
CREATE TABLE widget_sources (
  widget_id uuid NOT NULL REFERENCES widgets (id) ON DELETE CASCADE,
  source_kind smallint NOT NULL,
  source_id uuid NOT NULL,
  PRIMARY KEY (widget_id, source_kind, source_id)
);
CREATE INDEX widget_sources_source ON widget_sources (source_id);

CREATE TABLE request_ids (
  caller_id text NOT NULL,
  method text NOT NULL,
  request_id uuid NOT NULL,
  fingerprint_sha256 text NOT NULL,
  result_id uuid NOT NULL,
  created_at timestamptz NOT NULL,
  PRIMARY KEY (caller_id, method, request_id)
);
CREATE INDEX request_ids_created_at ON request_ids (created_at);

CREATE TABLE audit_events (
  id uuid PRIMARY KEY,
  occurred_at timestamptz NOT NULL,
  kind smallint NOT NULL,
  actor_user_id uuid,
  actor_device_id uuid,
  subject_user_id uuid,
  tenant_id uuid,
  device_id uuid,
  connection_id uuid,
  dashboard_id uuid,
  widget_id uuid,
  correlation_id text NOT NULL,
  client_kind smallint NOT NULL
);
CREATE INDEX audit_events_subject ON audit_events (subject_user_id, occurred_at DESC);
CREATE INDEX audit_events_tenant ON audit_events (tenant_id, occurred_at DESC);
CREATE INDEX audit_events_occurred_at ON audit_events (occurred_at);

-- kind: 1 connection, 2 device, 3 dashboard, 4 account data, 5 withdrawal, 6 tenant.
CREATE TABLE deletion_jobs (
  id uuid PRIMARY KEY,
  kind smallint NOT NULL,
  target_id uuid,
  user_id uuid,
  tenant_id uuid,
  requested_at timestamptz NOT NULL,
  deadline timestamptz NOT NULL,
  attempts integer NOT NULL DEFAULT 0,
  failure_category text NOT NULL DEFAULT '',
  completed_at timestamptz
);
CREATE INDEX deletion_jobs_pending ON deletion_jobs (deadline) WHERE completed_at IS NULL;

CREATE TABLE rate_limit_windows (
  scope smallint NOT NULL,
  subject text NOT NULL,
  window_start timestamptz NOT NULL,
  count integer NOT NULL,
  PRIMARY KEY (scope, subject, window_start)
);
CREATE INDEX rate_limit_windows_window_start ON rate_limit_windows (window_start);

CREATE TABLE worker_job_runs (
  job text PRIMARY KEY,
  last_started_at timestamptz,
  last_success_at timestamptz,
  last_failure_at timestamptz
);
