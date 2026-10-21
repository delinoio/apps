// SPDX-License-Identifier: FSL-1.1-ALv2

package postgres

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

// SelectedPlanBoolSQL returns the selected plan policy for a tenant. The plan
// row is the fallback for legacy tenants that have no catalog selection.
func SelectedPlanBoolSQL(tenant, plan, field string) string {
	if field != "free" && field != "postpaid" {
		panic("invalid plan policy field")
	}
	return "COALESCE((SELECT (cv.definition->>'" + field + "')::boolean FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=" + tenant + ".id AND cs.product_id=" + tenant + ".plan_id AND cs.ends_at IS NULL)," + plan + "." + field + ")"
}

func SelectedPlanRankSQL(tenant, plan string) string {
	return "COALESCE((SELECT (cv.definition->>'rank')::integer FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.tenant_id=" + tenant + ".id AND cs.product_id=" + tenant + ".plan_id AND cs.ends_at IS NULL)," + plan + ".rank)"
}

// PeriodExpiredSQL returns the SQL condition for a tenant whose confirmed paid
// period has ended at or before the instant that now names. tenant and plan are
// the aliases of the tenants and plans rows and now is a timestamp expression.
//
// Only a calendar-month free period without a provider subscription is exempt:
// maintenance rolls it over. A subscription, including a recurring add-on on a
// free base plan, ends with its confirmed period until a renewal advances it.
func PeriodExpiredSQL(tenant, plan, now string) string {
	return "(" + tenant + ".period_end<=" + now + " AND (NOT " + SelectedPlanBoolSQL(tenant, plan, "free") + " OR " + tenant + ".external_subscription_id<>''))"
}

// BillingPeriod returns the usage period containing at. A recorded Paddle
// renewal period applies to paid subscriptions; otherwise the period is the UTC
// calendar month used by free plans.
func BillingPeriod(ctx context.Context, tx pgx.Tx, tenant string, at time.Time) (time.Time, time.Time, error) {
	var start, end time.Time
	err := tx.QueryRow(ctx, "SELECT starts_at,ends_at FROM billing_periods WHERE tenant_id=$1 AND starts_at<=$2 AND ends_at>$2 ORDER BY starts_at DESC LIMIT 1", tenant, at).Scan(&start, &end)
	if err == nil {
		return start.UTC(), end.UTC(), nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return time.Time{}, time.Time{}, err
	}
	at = at.UTC()
	start = time.Date(at.Year(), at.Month(), 1, 0, 0, 0, 0, time.UTC)
	return start, start.AddDate(0, 1, 0), nil
}
