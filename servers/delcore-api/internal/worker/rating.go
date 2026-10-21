// SPDX-License-Identifier: FSL-1.1-ALv2

package worker

import (
	"context"
	"errors"
	"fmt"
	"strconv"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/internal/rating"
	"github.com/jackc/pgx/v5"
)

// rateMetering re-rates every meter period whose aggregates changed in one
// applied transition and appends the difference from the previously charged
// amount to the ledger. It runs in the transaction that materializes the
// transition and completes its job, so a rollback leaves neither aggregate nor
// ledger effects and a committed transition is never rated twice. The ledger
// source key names the event, the applied revision, and the meter.
func rateMetering(ctx context.Context, tx pgx.Tx, now time.Time, a events.Applied) (int, error) {
	seen := map[string]bool{}
	entries := 0
	for _, change := range a.Changes {
		if seen[change.MeterID] {
			continue
		}
		seen[change.MeterID] = true
		p := postgres.MeterPeriod{TenantID: a.TenantID, ServiceID: a.ServiceID, MeterID: change.MeterID, MeterKey: change.MeterKey, Start: a.PeriodStart, End: a.PeriodEnd}
		// An original event's charge is usage; any later revision, and every
		// decrease, is a correction, as with quantity reporting.
		kind := postgres.LedgerUsage
		if a.ToRevision > 1 {
			kind = postgres.LedgerCorrection
		}
		source := "rating:" + a.EventID + ":" + strconv.Itoa(int(a.ToRevision)) + ":" + change.MeterID
		delta, err := postgres.ReconcileMeterPeriod(ctx, tx, p, kind, source, now)
		if errors.Is(err, rating.ErrInconsistent) {
			return entries, fmt.Errorf("%w: %v", events.ErrPermanent, err)
		}
		if err != nil {
			return entries, err
		}
		if delta.Cmp(accounting.Zero()) != 0 {
			entries++
		}
	}
	return entries, nil
}
