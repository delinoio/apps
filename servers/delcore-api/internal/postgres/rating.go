// SPDX-License-Identifier: FSL-1.1-ALv2

package postgres

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"github.com/delinoio/apps/servers/delcore-api/internal/rating"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// LedgerKind is a stored ledger entry kind.
type LedgerKind string

const (
	LedgerUsage      LedgerKind = "usage"
	LedgerCorrection LedgerKind = "correction"
	LedgerPostpaid   LedgerKind = "postpaid"
)

// MeterPeriod identifies one tenant meter billing period. MeterID is the
// catalog meter entity and may be empty for a priced meter key without one.
type MeterPeriod struct {
	TenantID  string
	ServiceID string
	MeterID   string
	MeterKey  string
	Start     time.Time
	End       time.Time
}

// RateMeterPeriod rates the applied aggregates of one tenant meter billing
// period with the prices attributed at event acceptance and the allowance
// grants of the period. It reads only materialized state, so it reflects the
// last applied revisions while later work is pending. See package rating for
// the formula and the mixed-price rule.
func RateMeterPeriod(ctx context.Context, tx pgx.Tx, p MeterPeriod) (rating.Result, error) {
	in := rating.Input{Prices: map[string]accounting.Decimal{}}
	rows, err := tx.Query(ctx, "SELECT amount::text FROM allowance_grants WHERE tenant_id=$1 AND service_id=$2 AND meter=$3 AND period_start=$4 ORDER BY applied_at,id", p.TenantID, p.ServiceID, p.MeterKey, p.Start)
	if err != nil {
		return rating.Result{}, err
	}
	for rows.Next() {
		var text string
		if err = rows.Scan(&text); err != nil {
			rows.Close()
			return rating.Result{}, err
		}
		amount, err := accounting.Parse(text)
		if err != nil {
			rows.Close()
			return rating.Result{}, fmt.Errorf("%w: stored allowance grant", rating.ErrInconsistent)
		}
		in.Grants = append(in.Grants, amount)
	}
	rows.Close()
	if err = rows.Err(); err != nil {
		return rating.Result{}, err
	}
	if p.MeterID == "" {
		return rating.Compute(in)
	}
	rows, err = tx.Query(ctx, "SELECT group_key,aggregation,value::text FROM metering_aggregates WHERE tenant_id=$1 AND meter_id=$2 AND period_start=$3 ORDER BY group_key", p.TenantID, p.MeterID, p.Start)
	if err != nil {
		return rating.Result{}, err
	}
	for rows.Next() {
		var group rating.Group
		var kind, text string
		if err = rows.Scan(&group.Key, &kind, &text); err != nil {
			rows.Close()
			return rating.Result{}, err
		}
		if in.Aggregation != "" && in.Aggregation != metering.Aggregation(kind) {
			rows.Close()
			return rating.Result{}, fmt.Errorf("%w: aggregations differ within a meter", rating.ErrInconsistent)
		}
		in.Aggregation = metering.Aggregation(kind)
		if group.Value, err = accounting.Parse(text); err != nil {
			rows.Close()
			return rating.Result{}, fmt.Errorf("%w: stored aggregate value", rating.ErrInconsistent)
		}
		in.Groups = append(in.Groups, group)
	}
	rows.Close()
	if err = rows.Err(); err != nil {
		return rating.Result{}, err
	}
	// Bucket the applied contributions by group, attributed price, and the
	// number of grants applied at or before their occurrence. A grant applied
	// at the same instant as an occurrence is available to it.
	rows, err = tx.Query(ctx, `WITH applied AS (
 SELECT c.group_key,COALESCE(ep.price_id::text,'') price,
  (SELECT count(*) FROM allowance_grants g WHERE g.tenant_id=e.tenant_id AND g.service_id=$2 AND g.meter=$3 AND g.period_start=e.period_start AND g.applied_at<=e.occurred_at)::integer epoch,
  e.occurred_at,CASE WHEN c.value_kind='number' THEN c.value::numeric END amount
 FROM metering_events e
 JOIN metering_contributions c ON c.event_id=e.id AND c.revision=e.applied_revision AND c.meter_id=$4
 LEFT JOIN metering_event_prices ep ON ep.event_id=e.id AND ep.meter_id=$4
 WHERE e.tenant_id=$1 AND e.period_start=$5 AND e.applied_revision>0)
SELECT group_key,price,epoch,min(occurred_at),max(occurred_at),count(*),COALESCE(sum(amount),0)::text FROM applied GROUP BY group_key,price,epoch ORDER BY group_key,price,epoch`, p.TenantID, p.ServiceID, p.MeterKey, p.MeterID, p.Start)
	if err != nil {
		return rating.Result{}, err
	}
	var priceIDs []string
	for rows.Next() {
		var b rating.Bucket
		var text string
		if err = rows.Scan(&b.GroupKey, &b.PriceID, &b.Epoch, &b.First, &b.Last, &b.Count, &text); err != nil {
			rows.Close()
			return rating.Result{}, err
		}
		if b.Sum, err = accounting.Parse(text); err != nil {
			rows.Close()
			return rating.Result{}, fmt.Errorf("%w: stored contribution sum", rating.ErrInconsistent)
		}
		if b.PriceID != "" {
			if _, seen := in.Prices[b.PriceID]; !seen {
				in.Prices[b.PriceID] = accounting.Zero()
				priceIDs = append(priceIDs, b.PriceID)
			}
		}
		in.Buckets = append(in.Buckets, b)
	}
	rows.Close()
	if err = rows.Err(); err != nil {
		return rating.Result{}, err
	}
	if len(priceIDs) > 0 {
		rows, err = tx.Query(ctx, "SELECT id::text,unit_price::text,currency FROM prices WHERE id=ANY($1::uuid[])", priceIDs)
		if err != nil {
			return rating.Result{}, err
		}
		found := 0
		for rows.Next() {
			var id, text, currency string
			if err = rows.Scan(&id, &text, &currency); err != nil {
				rows.Close()
				return rating.Result{}, err
			}
			unit, err := accounting.Parse(text)
			if err != nil || currency != accounting.USD {
				rows.Close()
				return rating.Result{}, fmt.Errorf("%w: attributed price is not a USD unit price", rating.ErrInconsistent)
			}
			in.Prices[id] = unit
			found++
		}
		rows.Close()
		if err = rows.Err(); err != nil {
			return rating.Result{}, err
		}
		if found != len(priceIDs) {
			return rating.Result{}, fmt.Errorf("%w: attributed price is missing", rating.ErrInconsistent)
		}
	}
	return rating.Compute(in)
}

// PeriodPostpaid reports the postpaid policy recorded for a paid billing
// period. Calendar-month periods have no record and are prepaid, because free
// plans cannot continue postpaid.
func PeriodPostpaid(ctx context.Context, tx pgx.Tx, tenant string, start time.Time) (bool, error) {
	var postpaid bool
	err := tx.QueryRow(ctx, "SELECT postpaid FROM billing_periods WHERE tenant_id=$1 AND starts_at=$2", tenant, start).Scan(&postpaid)
	if errors.Is(err, pgx.ErrNoRows) {
		return false, nil
	}
	return postpaid, err
}

// AppendChargeDifference appends the ledger entries that move the charged
// amount of one meter period to target and returns the applied difference. It
// never rewrites entries. A decrease first reverses unpaid postpaid charges of
// the period and returns only the remainder as a correction, so a correction
// of postpaid usage cannot mint spendable credit. On a postpaid period an
// increase beyond the available credit balance is recorded as an unpaid
// postpaid charge. Increases use kind; decreases always use the correction
// kind. sourceKey identifies the main entry and sourceKey+":postpaid" the
// postpaid entry, so repeating the same reconciliation conflicts instead of
// duplicating its effect.
func AppendChargeDifference(ctx context.Context, tx pgx.Tx, p MeterPeriod, target accounting.Decimal, postpaid bool, kind LedgerKind, sourceKey string, now time.Time) (accounting.Decimal, error) {
	zero := accounting.Zero()
	var chargedText, unpaidText string
	if err := tx.QueryRow(ctx, `SELECT (-COALESCE(sum(amount),0))::text,greatest(-COALESCE(sum(amount) FILTER(WHERE kind='postpaid'),0),0)::text FROM ledger
 WHERE tenant_id=$1 AND service_id=$2 AND meter=$3 AND period_start=$4 AND kind IN ('usage','postpaid','correction')`, p.TenantID, p.ServiceID, p.MeterKey, p.Start).Scan(&chargedText, &unpaidText); err != nil {
		return zero, err
	}
	charged, err := accounting.Parse(chargedText)
	if err != nil {
		return zero, err
	}
	unpaid, err := accounting.Parse(unpaidText)
	if err != nil {
		return zero, err
	}
	delta := target.Sub(charged)
	if delta.Cmp(zero) == 0 {
		return zero, nil
	}
	remainder := delta
	if delta.Cmp(zero) < 0 {
		kind = LedgerCorrection
		refund := delta.Neg()
		if refund.Cmp(unpaid) > 0 {
			refund = unpaid
		}
		if refund.Cmp(zero) > 0 {
			if err = appendLedger(ctx, tx, p, LedgerPostpaid, refund, sourceKey+":postpaid", now); err != nil {
				return zero, err
			}
			remainder = remainder.Add(refund)
		}
	} else if postpaid {
		var balanceText string
		if err = tx.QueryRow(ctx, "SELECT greatest(COALESCE(sum(amount),0),0)::text FROM ledger WHERE tenant_id=$1 AND kind IN ('credit','usage','refund','correction')", p.TenantID).Scan(&balanceText); err != nil {
			return zero, err
		}
		balance, err := accounting.Parse(balanceText)
		if err != nil {
			return zero, err
		}
		if delta.Cmp(balance) > 0 {
			if err = appendLedger(ctx, tx, p, LedgerPostpaid, delta.Sub(balance).Neg(), sourceKey+":postpaid", now); err != nil {
				return zero, err
			}
			remainder = balance
		}
	}
	if remainder.Cmp(zero) != 0 {
		if err = appendLedger(ctx, tx, p, kind, remainder.Neg(), sourceKey, now); err != nil {
			return zero, err
		}
	}
	return delta, nil
}

func appendLedger(ctx context.Context, tx pgx.Tx, p MeterPeriod, kind LedgerKind, amount accounting.Decimal, sourceKey string, now time.Time) error {
	_, err := tx.Exec(ctx, `INSERT INTO ledger(id,tenant_id,service_id,meter,kind,amount,period_start,period_end,source_key,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`,
		uuid.Must(uuid.NewV7()).String(), p.TenantID, p.ServiceID, p.MeterKey, string(kind), amount.String(), p.Start, p.End, sourceKey, now)
	return err
}

// RecordRating stores the latest exact rating of a period so final currency
// rounding revisits it after the period closes.
func RecordRating(ctx context.Context, tx pgx.Tx, p MeterPeriod, r rating.Result, now time.Time) error {
	_, err := tx.Exec(ctx, `INSERT INTO metering_rated_periods(tenant_id,meter_id,service_id,meter,period_start,period_end,charge,remaining_included,rated_at) VALUES($1,$2,$3,$4,$5,$6,$7::numeric,$8::numeric,$9)
 ON CONFLICT(tenant_id,meter_id,period_start) DO UPDATE SET charge=EXCLUDED.charge,remaining_included=EXCLUDED.remaining_included,rated_at=EXCLUDED.rated_at`,
		p.TenantID, p.MeterID, p.ServiceID, p.MeterKey, p.Start, p.End, r.Charge.String(), r.Remaining.String(), now)
	return err
}

// HasAllowance matches the service-level paid authorization rule: any current
// usage-priced meter with remaining included usage in the period that starts
// at start permits use of that service. Remaining included usage follows the
// rating rule, so clamped group aggregates of the last applied revisions
// consume the shared allowance.
func HasAllowance(ctx context.Context, tx pgx.Tx, tenant, service string, start time.Time) (bool, error) {
	rows, e := tx.Query(ctx, `SELECT DISTINCT p.meter,COALESCE(ce.id::text,'') FROM entitlements en JOIN prices p ON p.id=en.price_id
 LEFT JOIN catalog_entities ce ON ce.kind='meter' AND ce.scope=p.service_id::text AND ce.key=p.meter
 WHERE en.tenant_id=$1 AND p.service_id=$2 AND p.kind=1 AND en.ends_at IS NULL ORDER BY 1,2`, tenant, service)
	if e != nil {
		return false, e
	}
	var meters []MeterPeriod
	for rows.Next() {
		m := MeterPeriod{TenantID: tenant, ServiceID: service, Start: start}
		if e = rows.Scan(&m.MeterKey, &m.MeterID); e != nil {
			rows.Close()
			return false, e
		}
		meters = append(meters, m)
	}
	rows.Close()
	if e = rows.Err(); e != nil {
		return false, e
	}
	for _, m := range meters {
		r, e := RateMeterPeriod(ctx, tx, m)
		if e != nil {
			return false, e
		}
		if r.Remaining.Cmp(accounting.Zero()) > 0 {
			return true, nil
		}
	}
	return false, nil
}
