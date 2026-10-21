// SPDX-License-Identifier: FSL-1.1-ALv2

package events

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"time"
	"unicode/utf8"

	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

type meter struct {
	ID         string
	Definition metering.Definition
}

// contribution is one matched meter group and value of one revision.
type contribution struct {
	MeterID     string
	MeterKey    string
	Aggregation metering.Aggregation
	GroupKey    string
	GroupValues map[string]metering.Value
	Value       metering.Value
}

func newID() string { return uuid.Must(uuid.NewV7()).String() }

func validKey(key string) error {
	if key == "" || len(key) > MaxKeyBytes || !utf8.ValidString(key) {
		return reject(ReasonInvalidInput, fmt.Sprintf("event key must contain 1 to %d bytes", MaxKeyBytes))
	}
	return nil
}

// Ingest validates and durably accepts one original event with its queued
// processing work. Identical replay returns the original identity without
// repeated effects; different content under the same key conflicts.
func Ingest(ctx context.Context, tx pgx.Tx, now time.Time, policy Policy, in Input) (Accepted, error) {
	if err := validKey(in.Key); err != nil {
		return Accepted{}, err
	}
	if err := ValidateContent(in.Type, in.Attributes); err != nil {
		return Accepted{}, err
	}
	// PostgreSQL keeps microseconds; normalize before hashing and storage.
	in.OccurredAt = in.OccurredAt.UTC().Truncate(time.Microsecond)
	digest := eventFingerprint(in)
	var existing, previous string
	err := tx.QueryRow(ctx, `SELECT id::text,fingerprint FROM metering_events WHERE service_id=$1 AND event_key=$2
 UNION ALL SELECT event_id::text,fingerprint FROM metering_replay WHERE service_id=$1 AND event_key=$2 LIMIT 1`, in.ServiceID, in.Key).Scan(&existing, &previous)
	if err == nil {
		if previous != digest {
			return Accepted{}, reject(ReasonIdentityConflict, "event key was accepted with different content")
		}
		return Accepted{EventID: existing, Revision: 1, Replayed: true}, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return Accepted{}, err
	}
	if in.OccurredAt.After(now) {
		return Accepted{}, reject(ReasonInvalidInput, "event occurrence is in the future")
	}
	if in.OccurredAt.Before(now.Add(-policy.Lateness)) {
		return Accepted{}, reject(ReasonWindowClosed, "event occurrence is older than the accepted lateness")
	}
	if err = tenantOpen(ctx, tx, in.TenantID, in.OccurredAt, now, policy); err != nil {
		return Accepted{}, err
	}
	start, end, err := postgres.BillingPeriod(ctx, tx, in.TenantID, in.OccurredAt)
	if errors.Is(err, postgres.ErrPeriodUnavailable) {
		return Accepted{}, reject(ReasonPeriodUnavailable, "historical billing period unavailable")
	}
	if err != nil {
		return Accepted{}, err
	}
	meters, err := loadMeters(ctx, tx, `ce.kind='meter' AND ce.scope=$1`, in.ServiceID)
	if err != nil {
		return Accepted{}, err
	}
	added, err := match(meters, metering.Event{Type: in.Type, Attributes: in.Attributes})
	if err != nil {
		return Accepted{}, err
	}
	if err = admit(ctx, tx, in.TenantID, start, nil, added, policy.Limits); err != nil {
		return Accepted{}, err
	}
	id := newID()
	meterIDs := make([]string, 0, len(meters))
	for _, m := range meters {
		meterIDs = append(meterIDs, m.ID)
	}
	if _, err = tx.Exec(ctx, `INSERT INTO metering_events(id,tenant_id,service_id,event_key,fingerprint,occurred_at,accepted_at,period_start,period_end,meter_ids,latest_revision,applied_revision)
 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10::uuid[],1,0)`, id, in.TenantID, in.ServiceID, in.Key, digest, in.OccurredAt, now, start, end, meterIDs); err != nil {
		return Accepted{}, err
	}
	if err = record(ctx, tx, id, in.TenantID, 1, metering.Insert, in.Type, in.Attributes, digest, added, now); err != nil {
		return Accepted{}, err
	}
	return Accepted{EventID: id, Revision: 1, Meters: len(added)}, nil
}

// Correct appends a replacement or cancellation when the caller names the
// latest revision. Tenant, service, occurrence, and period remain fixed.
func Correct(ctx context.Context, tx pgx.Tx, now time.Time, policy Policy, c Correction) (Accepted, error) {
	if err := validKey(c.Key); err != nil {
		return Accepted{}, err
	}
	if c.ExpectedRevision < 1 {
		return Accepted{}, reject(ReasonInvalidInput, "expected revision is required")
	}
	switch c.Kind {
	case metering.Replace:
		if err := ValidateContent(c.Type, c.Attributes); err != nil {
			return Accepted{}, err
		}
	case metering.Cancel:
		if c.Type != "" || len(c.Attributes) != 0 {
			return Accepted{}, reject(ReasonInvalidInput, "a cancellation carries no event content")
		}
	default:
		return Accepted{}, reject(ReasonInvalidInput, "correction kind must be replace or cancel")
	}
	var id, tenant string
	var occurred, start time.Time
	var meterIDs []string
	var latest int32
	err := tx.QueryRow(ctx, `SELECT id::text,tenant_id::text,occurred_at,period_start,meter_ids::text[],latest_revision FROM metering_events WHERE service_id=$1 AND event_key=$2 FOR UPDATE`, c.ServiceID, c.Key).Scan(&id, &tenant, &occurred, &start, &meterIDs, &latest)
	if errors.Is(err, pgx.ErrNoRows) {
		return Accepted{}, reject(ReasonNotFound, "event is not retained for this service")
	}
	if err != nil {
		return Accepted{}, err
	}
	if tenant != c.TenantID {
		return Accepted{}, reject(ReasonForbidden, "operation is not permitted")
	}
	digest := correctionFingerprint(id, c)
	if c.ExpectedRevision != latest {
		if c.ExpectedRevision < latest {
			var next string
			if err = tx.QueryRow(ctx, "SELECT fingerprint FROM metering_event_revisions WHERE event_id=$1 AND revision=$2", id, c.ExpectedRevision+1).Scan(&next); err != nil {
				return Accepted{}, err
			}
			if next == digest {
				return Accepted{EventID: id, Revision: c.ExpectedRevision + 1, Replayed: true}, nil
			}
		}
		return Accepted{}, reject(ReasonStaleRevision, "expected revision is not the latest revision")
	}
	var kind string
	if err = tx.QueryRow(ctx, "SELECT kind FROM metering_event_revisions WHERE event_id=$1 AND revision=$2", id, latest).Scan(&kind); err != nil {
		return Accepted{}, err
	}
	if kind == string(metering.Cancel) {
		return Accepted{}, reject(ReasonCancelled, "a cancelled event cannot be corrected")
	}
	if occurred.Before(now.Add(-policy.Lateness)) {
		return Accepted{}, reject(ReasonWindowClosed, "event occurrence is older than the accepted lateness")
	}
	if err = tenantOpen(ctx, tx, tenant, occurred, now, policy); err != nil {
		return Accepted{}, err
	}
	removed, err := loadContributions(ctx, tx, id, latest)
	if err != nil {
		return Accepted{}, err
	}
	var added []contribution
	if c.Kind == metering.Replace {
		// A replacement is matched against the meters evaluated when the
		// original was accepted; meters added later are not backfilled.
		meters, err := loadMeters(ctx, tx, `ce.id=ANY($1::uuid[])`, meterIDs)
		if err != nil {
			return Accepted{}, err
		}
		if added, err = match(meters, metering.Event{Type: c.Type, Attributes: c.Attributes}); err != nil {
			return Accepted{}, err
		}
	}
	if err = admit(ctx, tx, tenant, start, removed, added, policy.Limits); err != nil {
		return Accepted{}, err
	}
	next := latest + 1
	if _, err = tx.Exec(ctx, "UPDATE metering_events SET latest_revision=$2 WHERE id=$1", id, next); err != nil {
		return Accepted{}, err
	}
	if err = record(ctx, tx, id, tenant, next, c.Kind, c.Type, c.Attributes, digest, added, now); err != nil {
		return Accepted{}, err
	}
	return Accepted{EventID: id, Revision: next, Meters: len(added)}, nil
}

// Get returns the latest accepted revision of an event owned by the service.
func Get(ctx context.Context, tx pgx.Tx, tenantID, serviceID, key string) (Snapshot, error) {
	if err := validKey(key); err != nil {
		return Snapshot{}, err
	}
	s := Snapshot{ServiceID: serviceID, Key: key}
	var kind string
	var raw []byte
	err := tx.QueryRow(ctx, `SELECT e.id::text,e.tenant_id::text,e.occurred_at,e.latest_revision,e.applied_revision,r.kind,r.event_type,r.attributes
 FROM metering_events e JOIN metering_event_revisions r ON r.event_id=e.id AND r.revision=e.latest_revision
 WHERE e.service_id=$1 AND e.event_key=$2`, serviceID, key).Scan(&s.ID, &s.TenantID, &s.OccurredAt, &s.Revision, &s.AppliedRevision, &kind, &s.Type, &raw)
	if errors.Is(err, pgx.ErrNoRows) {
		return Snapshot{}, reject(ReasonNotFound, "event is not retained for this service")
	}
	if err != nil {
		return Snapshot{}, err
	}
	if s.TenantID != tenantID {
		return Snapshot{}, reject(ReasonForbidden, "operation is not permitted")
	}
	s.Cancelled = kind == string(metering.Cancel)
	if s.Attributes, err = DecodeValues(raw); err != nil {
		return Snapshot{}, err
	}
	s.OccurredAt = s.OccurredAt.UTC()
	return s, nil
}

// tenantOpen applies the legacy reporting rules: deleted tenants reject usage,
// and deleting tenants accept prior occurrences only during the reporting grace.
func tenantOpen(ctx context.Context, tx pgx.Tx, tenant string, occurred, now time.Time, policy Policy) error {
	var deleting *time.Time
	err := tx.QueryRow(ctx, "SELECT deleting_at FROM tenants WHERE id=$1 AND deleted_at IS NULL", tenant).Scan(&deleting)
	if errors.Is(err, pgx.ErrNoRows) {
		return reject(ReasonForbidden, "operation is not permitted")
	}
	if err != nil {
		return err
	}
	if deleting != nil && (occurred.After(*deleting) || now.After(deleting.Add(policy.ReportingGrace))) {
		return reject(ReasonWindowClosed, "tenant reporting grace has ended")
	}
	return nil
}

// loadMeters reads typed catalog meters. Legacy meters without a typed
// definition never match events.
func loadMeters(ctx context.Context, tx pgx.Tx, condition string, argument any) ([]meter, error) {
	rows, err := tx.Query(ctx, `SELECT ce.id::text,cv.definition FROM catalog_entities ce JOIN catalog_versions cv ON cv.id=ce.active_version_id
 WHERE `+condition+` AND ce.kind='meter' AND COALESCE(cv.definition->>'event_type','')<>'' ORDER BY ce.key,ce.id`, argument)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var meters []meter
	for rows.Next() {
		var m meter
		var raw []byte
		if err = rows.Scan(&m.ID, &raw); err != nil {
			return nil, err
		}
		if m.Definition, err = decodeMeter(raw); err != nil {
			return nil, fmt.Errorf("stored meter definition is invalid: %w", err)
		}
		meters = append(meters, m)
	}
	return meters, rows.Err()
}

// match validates every matched meter before returning any contribution.
func match(meters []meter, event metering.Event) ([]contribution, error) {
	if len(meters) == 0 {
		return nil, reject(ReasonNoMatchingMeter, "no meter matches the event")
	}
	definitions := make([]metering.Definition, 0, len(meters))
	byKey := make(map[string]meter, len(meters))
	for _, m := range meters {
		definitions = append(definitions, m.Definition)
		byKey[m.Definition.Key] = m
	}
	matched, err := metering.Match(definitions, event)
	switch {
	case errors.Is(err, metering.ErrNoMatch):
		return nil, reject(ReasonNoMatchingMeter, "no meter matches the event")
	case errors.Is(err, metering.ErrEvent):
		return nil, reject(ReasonInvalidAttribute, "a matched meter lacks a required attribute or receives an invalid type")
	case err != nil:
		return nil, err
	}
	out := make([]contribution, 0, len(matched))
	for _, c := range matched {
		m := byKey[c.MeterKey]
		out = append(out, contribution{MeterID: m.ID, MeterKey: c.MeterKey, Aggregation: m.Definition.Aggregation, GroupKey: c.GroupKey, GroupValues: c.GroupValues, Value: c.Value})
	}
	return out, nil
}

// record appends one revision with its contributions and queues durable work
// before the caller acknowledges the request.
func record(ctx context.Context, tx pgx.Tx, eventID, tenant string, revision int32, kind metering.RevisionKind, eventType string, attributes map[string]metering.Value, digest string, contributions []contribution, now time.Time) error {
	if kind == metering.Cancel {
		attributes = nil
	}
	encoded, err := EncodeValues(attributes)
	if err != nil {
		return err
	}
	if _, err = tx.Exec(ctx, `INSERT INTO metering_event_revisions(event_id,tenant_id,revision,kind,event_type,attributes,fingerprint,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)`,
		eventID, tenant, revision, string(kind), eventType, encoded, digest, now); err != nil {
		return err
	}
	for _, c := range contributions {
		groupValues, err := EncodeValues(c.GroupValues)
		if err != nil {
			return err
		}
		if _, err = tx.Exec(ctx, `INSERT INTO metering_contributions(event_id,tenant_id,revision,meter_id,meter_key,aggregation,group_key,group_values,value_kind,value) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`,
			eventID, tenant, revision, c.MeterID, c.MeterKey, string(c.Aggregation), c.GroupKey, groupValues, string(c.Value.Kind), c.Value.Text); err != nil {
			return err
		}
	}
	payload := map[string]any{"event_id": eventID, "revision": revision}
	_, err = tx.Exec(ctx, `INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$6,$6)`,
		newID(), tenant, JobKind, fmt.Sprintf("%s:%s:%d", JobKind, eventID, revision), payload, now)
	return err
}

func loadContributions(ctx context.Context, tx pgx.Tx, eventID string, revision int32) ([]contribution, error) {
	rows, err := tx.Query(ctx, `SELECT meter_id::text,meter_key,aggregation,group_key,group_values,value_kind,value FROM metering_contributions WHERE event_id=$1 AND revision=$2 ORDER BY meter_key,meter_id`, eventID, revision)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []contribution
	for rows.Next() {
		var c contribution
		var aggregation, kind string
		var raw []byte
		if err = rows.Scan(&c.MeterID, &c.MeterKey, &aggregation, &c.GroupKey, &raw, &kind, &c.Value.Text); err != nil {
			return nil, err
		}
		c.Aggregation = metering.Aggregation(aggregation)
		c.Value.Kind = metering.ValueKind(kind)
		if c.GroupValues, err = DecodeValues(raw); err != nil {
			return nil, err
		}
		out = append(out, c)
	}
	return out, rows.Err()
}

// admit updates admission accounting from the previous latest revision to the
// new latest revision of one event. Removals are applied first, so moving an
// event between groups needs capacity only for the destination. Accepted but
// unprocessed revisions are counted because accounting follows the latest
// revision, not the materialized aggregates. Period rows are locked in a fixed
// order so concurrent requests cannot both claim the last slot.
func admit(ctx context.Context, tx pgx.Tx, tenant string, period time.Time, removed, added []contribution, limits metering.Limits) error {
	ids := map[string]bool{}
	for _, c := range append(append([]contribution(nil), removed...), added...) {
		ids[c.MeterID] = true
	}
	ordered := make([]string, 0, len(ids))
	for id := range ids {
		ordered = append(ordered, id)
	}
	sort.Strings(ordered)
	groups := make(map[string]int64, len(ordered))
	for _, id := range ordered {
		if _, err := tx.Exec(ctx, "INSERT INTO metering_admission_periods(tenant_id,meter_id,period_start,groups) VALUES($1,$2,$3,0) ON CONFLICT DO NOTHING", tenant, id, period); err != nil {
			return err
		}
		var n int64
		if err := tx.QueryRow(ctx, "SELECT groups FROM metering_admission_periods WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 FOR UPDATE", tenant, id, period).Scan(&n); err != nil {
			return err
		}
		groups[id] = n
	}
	for _, c := range removed {
		if err := release(ctx, tx, tenant, period, c, groups); err != nil {
			return err
		}
	}
	for _, c := range added {
		if err := acquire(ctx, tx, tenant, period, c, groups, limits); err != nil {
			return err
		}
	}
	for _, id := range ordered {
		if _, err := tx.Exec(ctx, "UPDATE metering_admission_periods SET groups=$4 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3", tenant, id, period, groups[id]); err != nil {
			return err
		}
	}
	return nil
}

func release(ctx context.Context, tx pgx.Tx, tenant string, period time.Time, c contribution, groups map[string]int64) error {
	digest := GroupDigest(c.GroupKey)
	var events, distinct int64
	if err := tx.QueryRow(ctx, "SELECT events,distinct_values FROM metering_admission_groups WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4 FOR UPDATE", tenant, c.MeterID, period, digest).Scan(&events, &distinct); err != nil {
		return fmt.Errorf("admission group is missing: %w", err)
	}
	if metering.RetainsValues(c.Aggregation) {
		var valueEvents int64
		if err := tx.QueryRow(ctx, "SELECT events FROM metering_admission_values WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6 FOR UPDATE", tenant, c.MeterID, period, digest, string(c.Value.Kind), c.Value.Text).Scan(&valueEvents); err != nil {
			return fmt.Errorf("admission value is missing: %w", err)
		}
		var err error
		if valueEvents == 1 {
			_, err = tx.Exec(ctx, "DELETE FROM metering_admission_values WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6", tenant, c.MeterID, period, digest, string(c.Value.Kind), c.Value.Text)
			distinct--
		} else {
			_, err = tx.Exec(ctx, "UPDATE metering_admission_values SET events=events-1 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6", tenant, c.MeterID, period, digest, string(c.Value.Kind), c.Value.Text)
		}
		if err != nil {
			return err
		}
	}
	var err error
	if events == 1 {
		_, err = tx.Exec(ctx, "DELETE FROM metering_admission_groups WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4", tenant, c.MeterID, period, digest)
		groups[c.MeterID]--
	} else {
		_, err = tx.Exec(ctx, "UPDATE metering_admission_groups SET events=events-1,distinct_values=$5 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4", tenant, c.MeterID, period, digest, distinct)
	}
	return err
}

func acquire(ctx context.Context, tx pgx.Tx, tenant string, period time.Time, c contribution, groups map[string]int64, limits metering.Limits) error {
	digest := GroupDigest(c.GroupKey)
	var distinct int64
	err := tx.QueryRow(ctx, "SELECT distinct_values FROM metering_admission_groups WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4 FOR UPDATE", tenant, c.MeterID, period, digest).Scan(&distinct)
	switch {
	case errors.Is(err, pgx.ErrNoRows):
		if limits.MaxGroupsPerMeter > 0 && groups[c.MeterID]+1 > limits.MaxGroupsPerMeter {
			return reject(ReasonCardinalityExceeded, fmt.Sprintf("meter %q reached its group limit for the billing period", c.MeterKey))
		}
		groups[c.MeterID]++
		if _, err = tx.Exec(ctx, "INSERT INTO metering_admission_groups(tenant_id,meter_id,period_start,group_digest,events,distinct_values) VALUES($1,$2,$3,$4,1,0)", tenant, c.MeterID, period, digest); err != nil {
			return err
		}
	case err != nil:
		return err
	default:
		if _, err = tx.Exec(ctx, "UPDATE metering_admission_groups SET events=events+1 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4", tenant, c.MeterID, period, digest); err != nil {
			return err
		}
	}
	if !metering.RetainsValues(c.Aggregation) {
		return nil
	}
	tag, err := tx.Exec(ctx, "UPDATE metering_admission_values SET events=events+1 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4 AND value_kind=$5 AND value=$6", tenant, c.MeterID, period, digest, string(c.Value.Kind), c.Value.Text)
	if err != nil || tag.RowsAffected() == 1 {
		return err
	}
	if limits.MaxDistinctValuesPerGroup > 0 && distinct+1 > limits.MaxDistinctValuesPerGroup {
		return reject(ReasonCardinalityExceeded, fmt.Sprintf("meter %q reached its distinct-value limit for a group", c.MeterKey))
	}
	if _, err = tx.Exec(ctx, "INSERT INTO metering_admission_values(tenant_id,meter_id,period_start,group_digest,value_kind,value,events) VALUES($1,$2,$3,$4,$5,$6,1)", tenant, c.MeterID, period, digest, string(c.Value.Kind), c.Value.Text); err != nil {
		return err
	}
	_, err = tx.Exec(ctx, "UPDATE metering_admission_groups SET distinct_values=distinct_values+1 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_digest=$4", tenant, c.MeterID, period, digest)
	return err
}
