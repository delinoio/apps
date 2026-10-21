// SPDX-License-Identifier: FSL-1.1-ALv2

package events

import (
	"context"
	"errors"
	"fmt"
	"math/big"
	"sort"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"github.com/jackc/pgx/v5"
)

// MeterChange is an exact before/after group value of one meter. Before is nil
// for an introduced group and After is nil for a group left empty.
type MeterChange struct {
	MeterID string
	metering.Change
}

// Applied describes one materialized transition. Rating can append ledger
// differences for Changes in the same transaction before the job completes.
type Applied struct {
	EventID      string
	TenantID     string
	ServiceID    string
	PeriodStart  time.Time
	PeriodEnd    time.Time
	FromRevision int32
	ToRevision   int32
	Changes      []MeterChange
}

// Process moves an event's materialized state from its applied revision to its
// latest accepted revision. It is idempotent: once the latest revision is
// applied, repeated work changes nothing. Several queued revisions collapse into
// one transition with the same exact result as applying them in order.
func Process(ctx context.Context, tx pgx.Tx, now time.Time, eventID string) (Applied, error) {
	a := Applied{EventID: eventID}
	err := tx.QueryRow(ctx, `SELECT tenant_id::text,service_id::text,period_start,period_end,applied_revision,latest_revision FROM metering_events WHERE id=$1 FOR UPDATE`, eventID).
		Scan(&a.TenantID, &a.ServiceID, &a.PeriodStart, &a.PeriodEnd, &a.FromRevision, &a.ToRevision)
	if errors.Is(err, pgx.ErrNoRows) {
		return Applied{}, fmt.Errorf("%w: event is missing", ErrPermanent)
	}
	if err != nil {
		return Applied{}, err
	}
	a.PeriodStart, a.PeriodEnd = a.PeriodStart.UTC(), a.PeriodEnd.UTC()
	if a.FromRevision == a.ToRevision {
		return a, nil
	}
	var old []contribution
	if a.FromRevision > 0 {
		if old, err = loadContributions(ctx, tx, eventID, a.FromRevision); err != nil {
			return Applied{}, err
		}
	}
	latest, err := loadContributions(ctx, tx, eventID, a.ToRevision)
	if err != nil {
		return Applied{}, err
	}
	type pair struct{ old, latest *contribution }
	byMeter := map[string]*pair{}
	var order []string
	for i := range old {
		byMeter[old[i].MeterID] = &pair{old: &old[i]}
		order = append(order, old[i].MeterID)
	}
	for i := range latest {
		p := byMeter[latest[i].MeterID]
		if p == nil {
			p = &pair{}
			byMeter[latest[i].MeterID] = p
			order = append(order, latest[i].MeterID)
		}
		p.latest = &latest[i]
	}
	key := func(id string) string {
		if p := byMeter[id]; p.latest != nil {
			return p.latest.MeterKey
		}
		return byMeter[id].old.MeterKey
	}
	sort.Slice(order, func(i, j int) bool {
		if key(order[i]) != key(order[j]) {
			return key(order[i]) < key(order[j])
		}
		return order[i] < order[j]
	})
	for _, id := range order {
		p := byMeter[id]
		type delta struct {
			values  map[string]metering.Value
			removes []metering.Value
			adds    []metering.Value
		}
		groups := map[string]*delta{}
		if p.old != nil {
			groups[p.old.GroupKey] = &delta{values: p.old.GroupValues, removes: []metering.Value{p.old.Value}}
		}
		if p.latest != nil {
			d := groups[p.latest.GroupKey]
			if d == nil {
				d = &delta{values: p.latest.GroupValues}
				groups[p.latest.GroupKey] = d
			}
			d.adds = append(d.adds, p.latest.Value)
		}
		keys := make([]string, 0, len(groups))
		for k := range groups {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		reference := p.latest
		if reference == nil {
			reference = p.old
		}
		for _, groupKey := range keys {
			d := groups[groupKey]
			before, after, err := applyGroup(ctx, tx, now, a, reference, groupKey, d.values, d.removes, d.adds)
			if err != nil {
				return Applied{}, err
			}
			if before == nil && after == nil || before != nil && after != nil && sameAggregate(*before, *after) {
				continue
			}
			a.Changes = append(a.Changes, MeterChange{MeterID: id, Change: metering.Change{MeterKey: reference.MeterKey, GroupKey: groupKey, Before: before, After: after}})
		}
	}
	if _, err = tx.Exec(ctx, "UPDATE metering_events SET applied_revision=$2 WHERE id=$1", eventID, a.ToRevision); err != nil {
		return Applied{}, err
	}
	return a, nil
}

func sameAggregate(a, b metering.Aggregate) bool {
	return a.Count == b.Count && a.DistinctCount == b.DistinctCount && a.Sum.Cmp(b.Sum) == 0 && a.Value.Cmp(b.Value) == 0
}

func tracksSum(kind metering.Aggregation) bool {
	return kind == metering.Sum || kind == metering.Avg || kind == metering.Min || kind == metering.Max
}

// applyGroup removes and adds contributions in one group and returns the exact
// aggregate before and after. A group without contributions is deleted.
func applyGroup(ctx context.Context, tx pgx.Tx, now time.Time, a Applied, c *contribution, groupKey string, values map[string]metering.Value, removes, adds []metering.Value) (*metering.Aggregate, *metering.Aggregate, error) {
	kind := c.Aggregation
	var before *metering.Aggregate
	var rawValues []byte
	var stored, sumText, valueText string
	var count, distinct int64
	err := tx.QueryRow(ctx, `SELECT group_values,aggregation,event_count,sum::text,distinct_count,value::text FROM metering_aggregates
 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key=$4 FOR UPDATE`, a.TenantID, c.MeterID, a.PeriodStart, groupKey).Scan(&rawValues, &stored, &count, &sumText, &distinct, &valueText)
	sum := accounting.Zero()
	switch {
	case errors.Is(err, pgx.ErrNoRows):
	case err != nil:
		return nil, nil, err
	default:
		if metering.Aggregation(stored) != kind {
			return nil, nil, fmt.Errorf("%w: stored aggregation differs from the meter", ErrPermanent)
		}
		value, err := accounting.Parse(valueText)
		if err != nil {
			return nil, nil, fmt.Errorf("%w: stored aggregate value", ErrPermanent)
		}
		if sum, err = accounting.Parse(sumText); err != nil {
			return nil, nil, fmt.Errorf("%w: stored aggregate sum", ErrPermanent)
		}
		if values, err = DecodeValues(rawValues); err != nil {
			return nil, nil, err
		}
		before = &metering.Aggregate{MeterKey: c.MeterKey, GroupKey: groupKey, GroupValues: values, Kind: kind, Count: count, Sum: sum, Value: value, DistinctCount: distinct}
	}
	change := func(v metering.Value, delta int64) error {
		count += delta
		if tracksSum(kind) {
			n, err := accounting.Parse(v.Text)
			if err != nil || v.Kind != metering.ValueNumber {
				return fmt.Errorf("%w: stored contribution is not numeric", ErrPermanent)
			}
			if delta < 0 {
				n = n.Neg()
			}
			sum = sum.Add(n)
		}
		if !metering.RetainsValues(kind) {
			return nil
		}
		changed, err := changeValue(ctx, tx, a, c.MeterID, groupKey, v, delta)
		distinct += changed
		return err
	}
	for _, v := range removes {
		if err := change(v, -1); err != nil {
			return nil, nil, err
		}
	}
	for _, v := range adds {
		if err := change(v, 1); err != nil {
			return nil, nil, err
		}
	}
	if count < 0 || distinct < 0 {
		return nil, nil, fmt.Errorf("%w: aggregate state would become negative", ErrPermanent)
	}
	if count == 0 {
		if distinct != 0 {
			return nil, nil, fmt.Errorf("%w: empty group retained values", ErrPermanent)
		}
		_, err = tx.Exec(ctx, "DELETE FROM metering_aggregates WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key=$4", a.TenantID, c.MeterID, a.PeriodStart, groupKey)
		return before, nil, err
	}
	var value accounting.Decimal
	switch kind {
	case metering.Count:
		value = accounting.FromMinor(big.NewInt(count), 0)
	case metering.Sum:
		value = sum
	case metering.Avg:
		value = metering.RoundedAverage(sum, count)
	case metering.UniqueCount:
		value = accounting.FromMinor(big.NewInt(distinct), 0)
	case metering.Min, metering.Max:
		function := "min"
		if kind == metering.Max {
			function = "max"
		}
		var text *string
		if err = tx.QueryRow(ctx, `SELECT `+function+`(numeric_value)::text FROM metering_aggregate_values
 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key=$4 AND value_kind='number'`, a.TenantID, c.MeterID, a.PeriodStart, groupKey).Scan(&text); err != nil {
			return nil, nil, err
		}
		if text == nil {
			return nil, nil, fmt.Errorf("%w: extremum has no retained values", ErrPermanent)
		}
		if value, err = accounting.Parse(*text); err != nil {
			return nil, nil, fmt.Errorf("%w: stored extremum", ErrPermanent)
		}
	default:
		return nil, nil, fmt.Errorf("%w: unknown aggregation", ErrPermanent)
	}
	encoded, err := EncodeValues(values)
	if err != nil {
		return nil, nil, err
	}
	if _, err = tx.Exec(ctx, `INSERT INTO metering_aggregates(tenant_id,meter_id,period_start,period_end,group_key,group_values,aggregation,event_count,sum,distinct_count,value,updated_at)
 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9::numeric,$10,$11::numeric,$12)
 ON CONFLICT(tenant_id,meter_id,period_start,group_key) DO UPDATE SET event_count=EXCLUDED.event_count,sum=EXCLUDED.sum,distinct_count=EXCLUDED.distinct_count,value=EXCLUDED.value,updated_at=EXCLUDED.updated_at`,
		a.TenantID, c.MeterID, a.PeriodStart, a.PeriodEnd, groupKey, encoded, string(kind), count, sum.String(), distinct, value.String(), now); err != nil {
		return nil, nil, err
	}
	after := &metering.Aggregate{MeterKey: c.MeterKey, GroupKey: groupKey, GroupValues: values, Kind: kind, Count: count, Sum: sum, Value: value, DistinctCount: distinct}
	if !metering.RetainsValues(kind) {
		after.DistinctCount = 0
	}
	if kind == metering.Count || kind == metering.UniqueCount {
		after.Sum = accounting.Zero()
	}
	return before, after, nil
}

// changeValue updates one retained value multiset entry and returns the change
// in the group's distinct-value count.
func changeValue(ctx context.Context, tx pgx.Tx, a Applied, meterID, groupKey string, v metering.Value, delta int64) (int64, error) {
	if delta > 0 {
		var numeric *string
		if v.Kind == metering.ValueNumber {
			numeric = &v.Text
		}
		var events int64
		if err := tx.QueryRow(ctx, `INSERT INTO metering_aggregate_values(tenant_id,meter_id,period_start,group_key,value_kind,value,numeric_value,events) VALUES($1,$2,$3,$4,$5,$6,$7::numeric,1)
 ON CONFLICT(tenant_id,meter_id,period_start,group_key,value_kind,value) DO UPDATE SET events=metering_aggregate_values.events+1 RETURNING events`,
			a.TenantID, meterID, a.PeriodStart, groupKey, string(v.Kind), v.Text, numeric).Scan(&events); err != nil {
			return 0, err
		}
		if events == 1 {
			return 1, nil
		}
		return 0, nil
	}
	var events int64
	err := tx.QueryRow(ctx, `SELECT events FROM metering_aggregate_values WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key=$4 AND value_kind=$5 AND value=$6 FOR UPDATE`,
		a.TenantID, meterID, a.PeriodStart, groupKey, string(v.Kind), v.Text).Scan(&events)
	if errors.Is(err, pgx.ErrNoRows) {
		return 0, fmt.Errorf("%w: retained value is missing", ErrPermanent)
	}
	if err != nil {
		return 0, err
	}
	if events == 1 {
		_, err = tx.Exec(ctx, `DELETE FROM metering_aggregate_values WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key=$4 AND value_kind=$5 AND value=$6`,
			a.TenantID, meterID, a.PeriodStart, groupKey, string(v.Kind), v.Text)
		return -1, err
	}
	_, err = tx.Exec(ctx, `UPDATE metering_aggregate_values SET events=events-1 WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 AND group_key=$4 AND value_kind=$5 AND value=$6`,
		a.TenantID, meterID, a.PeriodStart, groupKey, string(v.Kind), v.Text)
	return 0, err
}

// Retain removes event bodies, revision history, and derived metering rows of
// tenants whose deletion finished before cutoff, after copying replay
// identities. Tenants with unfinished metering work are skipped. The caller
// enables the controlled retention setting in the same transaction.
func Retain(ctx context.Context, tx pgx.Tx, cutoff time.Time) (int, error) {
	rows, err := tx.Query(ctx, `SELECT t.id::text FROM tenants t WHERE t.deleted_at<$1
 AND EXISTS(SELECT 1 FROM metering_events e WHERE e.tenant_id=t.id)
 AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.tenant_id=t.id AND j.kind=$2 AND j.state<>'done')`, cutoff, JobKind)
	if err != nil {
		return 0, err
	}
	var tenants []string
	for rows.Next() {
		var id string
		if err = rows.Scan(&id); err != nil {
			rows.Close()
			return 0, err
		}
		tenants = append(tenants, id)
	}
	rows.Close()
	if err = rows.Err(); err != nil || len(tenants) == 0 {
		return 0, err
	}
	for _, statement := range []string{
		"INSERT INTO metering_replay(service_id,event_key,event_id,fingerprint) SELECT service_id,event_key,id,fingerprint FROM metering_events WHERE tenant_id=ANY($1::uuid[]) ON CONFLICT DO NOTHING",
		"DELETE FROM metering_aggregate_values WHERE tenant_id=ANY($1::uuid[])",
		"DELETE FROM metering_aggregates WHERE tenant_id=ANY($1::uuid[])",
		"DELETE FROM metering_admission_values WHERE tenant_id=ANY($1::uuid[])",
		"DELETE FROM metering_admission_groups WHERE tenant_id=ANY($1::uuid[])",
		"DELETE FROM metering_admission_periods WHERE tenant_id=ANY($1::uuid[])",
		"DELETE FROM metering_contributions WHERE tenant_id=ANY($1::uuid[])",
		"DELETE FROM metering_event_revisions WHERE tenant_id=ANY($1::uuid[])",
		"DELETE FROM metering_events WHERE tenant_id=ANY($1::uuid[])",
	} {
		if _, err = tx.Exec(ctx, statement, tenants); err != nil {
			return 0, err
		}
	}
	return len(tenants), nil
}
