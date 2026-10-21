-- SPDX-License-Identifier: FSL-1.1-ALv2

-- One durable identity per service-scoped event key. Tenant, service,
-- occurrence, billing period, and the meter set evaluated at acceptance never
-- change. latest_revision advances with accepted corrections; applied_revision
-- trails it until asynchronous processing materializes the latest revision.
CREATE TABLE metering_events (
 id uuid PRIMARY KEY,
 tenant_id uuid NOT NULL REFERENCES tenants(id),
 service_id uuid NOT NULL REFERENCES services(id),
 event_key text NOT NULL,
 fingerprint text NOT NULL,
 occurred_at timestamptz NOT NULL,
 accepted_at timestamptz NOT NULL,
 period_start timestamptz NOT NULL,
 period_end timestamptz NOT NULL,
 meter_ids uuid[] NOT NULL,
 latest_revision integer NOT NULL DEFAULT 1,
 applied_revision integer NOT NULL DEFAULT 0,
 UNIQUE(service_id,event_key),
 UNIQUE(id,tenant_id),
 CHECK(period_end>period_start),
 CHECK(latest_revision>=1 AND applied_revision>=0 AND applied_revision<=latest_revision)
);
CREATE INDEX metering_events_tenant ON metering_events(tenant_id);

-- Original content and every correction, in revision order. Rows are
-- append-only; a cancellation is a revision without content.
CREATE TABLE metering_event_revisions (
 event_id uuid NOT NULL,
 tenant_id uuid NOT NULL,
 revision integer NOT NULL CHECK(revision>=1),
 kind text NOT NULL CHECK(kind IN ('insert','replace','cancel')),
 event_type text NOT NULL,
 attributes jsonb NOT NULL,
 fingerprint text NOT NULL,
 created_at timestamptz NOT NULL,
 PRIMARY KEY(event_id,revision),
 FOREIGN KEY(event_id,tenant_id) REFERENCES metering_events(id,tenant_id),
 CHECK((revision=1)=(kind='insert')),
 CHECK((kind='cancel')=(event_type='')),
 CHECK(kind<>'cancel' OR attributes='{}'::jsonb)
);

-- Matched meter contributions of each revision, computed once at acceptance
-- with the immutable meter definitions. Processing and admission accounting
-- read these rows instead of re-evaluating filters.
CREATE TABLE metering_contributions (
 event_id uuid NOT NULL,
 tenant_id uuid NOT NULL,
 revision integer NOT NULL,
 meter_id uuid NOT NULL REFERENCES catalog_entities(id),
 meter_key text NOT NULL,
 aggregation text NOT NULL CHECK(aggregation IN ('count','sum','min','max','avg','unique_count')),
 group_key text NOT NULL,
 group_values jsonb NOT NULL,
 value_kind text NOT NULL CHECK(value_kind IN ('','string','number','boolean')),
 value text NOT NULL,
 PRIMARY KEY(event_id,revision,meter_id),
 FOREIGN KEY(event_id,revision) REFERENCES metering_event_revisions(event_id,revision),
 FOREIGN KEY(event_id,tenant_id) REFERENCES metering_events(id,tenant_id),
 CHECK((aggregation='count')=(value_kind=''))
);

-- Replay identities that outlive retained event bodies.
CREATE TABLE metering_replay (
 service_id uuid NOT NULL REFERENCES services(id),
 event_key text NOT NULL,
 event_id uuid NOT NULL,
 fingerprint text NOT NULL,
 PRIMARY KEY(service_id,event_key)
);

-- Materialized group aggregates of applied revisions. Count, sum, and the
-- distinct-value count are exact; value is the exposed result, with averages
-- rounded half-up to six places. A group without contributions has no row.
-- group_key keeps the canonical encoding, which can exceed the B-tree entry
-- limit for several maximum-size group attributes, so keys and references
-- use group_digest, the SHA-256 digest of group_key.
CREATE TABLE metering_aggregates (
 tenant_id uuid NOT NULL REFERENCES tenants(id),
 meter_id uuid NOT NULL REFERENCES catalog_entities(id),
 period_start timestamptz NOT NULL,
 period_end timestamptz NOT NULL,
 group_key text NOT NULL,
 group_digest bytea NOT NULL CHECK(octet_length(group_digest)=32),
 group_values jsonb NOT NULL,
 aggregation text NOT NULL CHECK(aggregation IN ('count','sum','min','max','avg','unique_count')),
 event_count bigint NOT NULL CHECK(event_count>0),
 sum numeric NOT NULL,
 distinct_count bigint NOT NULL CHECK(distinct_count>=0),
 value numeric NOT NULL,
 updated_at timestamptz NOT NULL,
 PRIMARY KEY(tenant_id,meter_id,period_start,group_digest),
 CHECK(period_end>period_start)
);
-- Value multisets for minimum, maximum, and unique-count groups.
CREATE TABLE metering_aggregate_values (
 tenant_id uuid NOT NULL,
 meter_id uuid NOT NULL,
 period_start timestamptz NOT NULL,
 group_digest bytea NOT NULL,
 value_kind text NOT NULL CHECK(value_kind IN ('string','number','boolean')),
 value text NOT NULL,
 numeric_value numeric,
 events bigint NOT NULL CHECK(events>0),
 PRIMARY KEY(tenant_id,meter_id,period_start,group_digest,value_kind,value),
 FOREIGN KEY(tenant_id,meter_id,period_start,group_digest)
  REFERENCES metering_aggregates(tenant_id,meter_id,period_start,group_digest) DEFERRABLE INITIALLY DEFERRED,
 CHECK((value_kind='number')=(numeric_value IS NOT NULL))
);
CREATE INDEX metering_aggregate_numbers ON metering_aggregate_values(tenant_id,meter_id,period_start,group_digest,numeric_value)
 WHERE value_kind='number';

-- Admission accounting over the latest accepted revision of every event,
-- including revisions that processing has not materialized yet. The period
-- row is locked before groups or values change. Groups are identified by
-- group_digest, the SHA-256 digest of the canonical group key.
CREATE TABLE metering_admission_periods (
 tenant_id uuid NOT NULL REFERENCES tenants(id),
 meter_id uuid NOT NULL REFERENCES catalog_entities(id),
 period_start timestamptz NOT NULL,
 groups bigint NOT NULL CHECK(groups>=0),
 PRIMARY KEY(tenant_id,meter_id,period_start)
);
CREATE TABLE metering_admission_groups (
 tenant_id uuid NOT NULL,
 meter_id uuid NOT NULL,
 period_start timestamptz NOT NULL,
 group_digest bytea NOT NULL CHECK(octet_length(group_digest)=32),
 events bigint NOT NULL CHECK(events>0),
 distinct_values bigint NOT NULL CHECK(distinct_values>=0),
 PRIMARY KEY(tenant_id,meter_id,period_start,group_digest),
 FOREIGN KEY(tenant_id,meter_id,period_start) REFERENCES metering_admission_periods(tenant_id,meter_id,period_start)
);
CREATE TABLE metering_admission_values (
 tenant_id uuid NOT NULL,
 meter_id uuid NOT NULL,
 period_start timestamptz NOT NULL,
 group_digest bytea NOT NULL,
 value_kind text NOT NULL CHECK(value_kind IN ('string','number','boolean')),
 value text NOT NULL,
 events bigint NOT NULL CHECK(events>0),
 PRIMARY KEY(tenant_id,meter_id,period_start,group_digest,value_kind,value),
 FOREIGN KEY(tenant_id,meter_id,period_start,group_digest)
  REFERENCES metering_admission_groups(tenant_id,meter_id,period_start,group_digest) DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX jobs_metering_ready ON jobs(available_at,id) WHERE kind='metering' AND state='pending';

-- History and identities remain until controlled retention of fully deleted
-- tenants, matching the raw usage and ledger retention boundary.
CREATE FUNCTION metering_history_record() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' AND current_setting('delcore.retention',true)='on'
 AND EXISTS(SELECT 1 FROM tenants WHERE id=OLD.tenant_id AND deleted_at IS NOT NULL)
 THEN RETURN OLD; END IF;
 RAISE EXCEPTION 'metering event history is append-only';
END $$;
CREATE TRIGGER metering_revisions_immutable BEFORE UPDATE OR DELETE ON metering_event_revisions
 FOR EACH ROW EXECUTE FUNCTION metering_history_record();
CREATE TRIGGER metering_contributions_immutable BEFORE UPDATE OR DELETE ON metering_contributions
 FOR EACH ROW EXECUTE FUNCTION metering_history_record();

CREATE FUNCTION metering_event_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN
  IF current_setting('delcore.retention',true)='on'
  AND EXISTS(SELECT 1 FROM tenants WHERE id=OLD.tenant_id AND deleted_at IS NOT NULL)
  THEN RETURN OLD; END IF;
  RAISE EXCEPTION 'metering event identities are retained';
 END IF;
 IF ROW(NEW.id,NEW.tenant_id,NEW.service_id,NEW.event_key,NEW.fingerprint,NEW.occurred_at,NEW.accepted_at,NEW.period_start,NEW.period_end,NEW.meter_ids)
  IS DISTINCT FROM ROW(OLD.id,OLD.tenant_id,OLD.service_id,OLD.event_key,OLD.fingerprint,OLD.occurred_at,OLD.accepted_at,OLD.period_start,OLD.period_end,OLD.meter_ids)
  OR NEW.latest_revision<OLD.latest_revision OR NEW.applied_revision<OLD.applied_revision
 THEN RAISE EXCEPTION 'metering event identity is immutable'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER metering_events_identity BEFORE UPDATE OR DELETE ON metering_events
 FOR EACH ROW EXECUTE FUNCTION metering_event_identity();
