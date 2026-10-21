// SPDX-License-Identifier: FSL-1.1-ALv2

package worker

import (
	"context"
	"errors"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/internal/rating"
	"github.com/jackc/pgx/v5"
)

// Round the full meter charge before final payment allocation, including amounts
// already consumed from purchased credits. Never round individual events or
// groups: each closed rated period, and each period of a tenant past its
// deletion reporting grace, is re-rated exactly and its total rounded half-up
// to USD minor units. Late processing that changes a closed period appends its
// exact difference, and the next pass rounds the new total again.
func (w *Worker) roundUsage(ctx context.Context, tx pgx.Tx, now time.Time) error {
	rows, e := tx.Query(ctx, `SELECT r.tenant_id::text,r.service_id::text,r.meter_id::text,r.meter,r.period_start,r.period_end
 FROM metering_rated_periods r JOIN tenants t ON t.id=r.tenant_id
 WHERE r.period_end<=$1 OR t.deleting_at<$2 ORDER BY r.tenant_id,r.meter_id,r.period_start`, now, now.Add(-time.Duration(w.Config.ReportingGraceSeconds)*time.Second))
	if e != nil {
		return e
	}
	var periods []postgres.MeterPeriod
	for rows.Next() {
		var p postgres.MeterPeriod
		if e = rows.Scan(&p.TenantID, &p.ServiceID, &p.MeterID, &p.MeterKey, &p.Start, &p.End); e != nil {
			rows.Close()
			return e
		}
		periods = append(periods, p)
	}
	rows.Close()
	if e = rows.Err(); e != nil {
		return e
	}
	for _, p := range periods {
		result, e := postgres.RateMeterPeriod(ctx, tx, p)
		if errors.Is(e, rating.ErrInconsistent) {
			// Inconsistent stored metering state must not stop the rest of
			// maintenance. The period keeps its last charges until the failed
			// metering work that owns the state is recovered.
			if w.Log != nil {
				w.Log.ErrorContext(ctx, "rated period cannot be rounded", "category", "metering_invalid", "meter", p.MeterKey)
			}
			continue
		}
		if e != nil {
			return e
		}
		total, e := rating.RoundedCharge(result.Charge)
		if e != nil {
			return e
		}
		postpaid, e := postgres.PeriodPostpaid(ctx, tx, p.TenantID, p.Start)
		if e != nil {
			return e
		}
		if _, e = postgres.AppendChargeDifference(ctx, tx, p, total, postpaid, postgres.LedgerCorrection, "currency_rounding:"+id(), now); e != nil {
			return e
		}
	}
	return nil
}
