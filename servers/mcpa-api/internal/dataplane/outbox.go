// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"log/slog"
	"strconv"
	"strings"
	"time"

	"github.com/prometheus/client_golang/prometheus"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/obs"
	"github.com/delinoio/apps/servers/mcpa-api/internal/pricing"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// Outbox delivery limits.
const (
	// OutboxBatchSize is the number of rows sent per cycle.
	OutboxBatchSize = 100
	// OutboxMaxBackoff caps the exponential retry backoff.
	OutboxMaxBackoff = time.Hour
	// outboxLease keeps a claimed row from being claimed again while its
	// cycle runs. A crashed worker's rows become due after the lease.
	outboxLease = 5 * time.Minute
)

// Metrics holds the data-plane worker collectors.
type Metrics struct {
	outboxPending   prometheus.Gauge
	outboxPoisoned  prometheus.Gauge
	outboxOldestAge prometheus.Gauge
	outboxFailures  *prometheus.CounterVec
}

// NewMetrics registers the collectors on m.Registry. A nil m disables
// metrics.
func NewMetrics(m *obs.Metrics) (*Metrics, error) {
	out := &Metrics{
		outboxPending: prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "mcpa_outbox_pending", Help: "Pending usage outbox rows.",
		}),
		outboxPoisoned: prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "mcpa_outbox_poisoned", Help: "Poisoned usage outbox rows kept until expiry.",
		}),
		outboxOldestAge: prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "mcpa_outbox_oldest_pending_age_seconds", Help: "Age of the oldest pending usage outbox row.",
		}),
		outboxFailures: prometheus.NewCounterVec(prometheus.CounterOpts{
			Name: "mcpa_outbox_delivery_failures_total", Help: "Failed or lost usage outbox deliveries by category.",
		}, []string{"category"}),
	}
	if m == nil {
		return out, nil
	}
	for _, c := range []prometheus.Collector{out.outboxPending, out.outboxPoisoned, out.outboxOldestAge, out.outboxFailures} {
		if err := m.Registry.Register(c); err != nil {
			return nil, err
		}
	}
	return out, nil
}

// Delivery failure categories. They are bounded metric label values.
const (
	categoryUnavailable   = "unavailable"
	categoryUnauthorized  = "unauthenticated"
	categoryRejected      = "rejected"
	categoryExpired       = "expired"
	categoryTenantDeleted = "tenant_deleted"
	categoryStoreError    = "store_error"
)

// reasonCategory returns the lowercase short name of a Delcore reason, for
// example "no_matching_meter".
func reasonCategory(r delcorev1.ErrorReason) string {
	return strings.ToLower(strings.TrimPrefix(r.String(), "ERROR_REASON_"))
}

// poisonReasons are the Delcore reasons that will never succeed on retry.
func poisonReason(r delcorev1.ErrorReason) bool {
	switch r {
	case delcorev1.ErrorReason_ERROR_REASON_INVALID_INPUT,
		delcorev1.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE,
		delcorev1.ErrorReason_ERROR_REASON_EVENT_IDENTITY_CONFLICT,
		delcorev1.ErrorReason_ERROR_REASON_REPORTING_WINDOW_CLOSED:
		return true
	default:
		return false
	}
}

// classifyDelivery returns the failure category of a failed IngestEvent and
// whether the row must be poisoned. The reason is read only from a Delcore
// ErrorDetail.
func classifyDelivery(err error) (string, bool) {
	if r := delcore.ReasonOf(err); r != delcorev1.ErrorReason_ERROR_REASON_UNSPECIFIED {
		return reasonCategory(r), poisonReason(r)
	}
	var de *delcore.Error
	if errors.As(err, &de) {
		switch de.Kind {
		case delcore.KindUnavailable:
			return categoryUnavailable, false
		case delcore.KindUnauthenticated:
			return categoryUnauthorized, false
		}
		return categoryRejected, false
	}
	return categoryUnavailable, false
}

// Backoff returns the delay before attempt number attempts+1: the outbox
// interval doubled per failed attempt, capped at one hour.
func Backoff(interval time.Duration, attempts int) time.Duration {
	d := interval
	for i := 1; i < attempts; i++ {
		d *= 2
		if d >= OutboxMaxBackoff || d <= 0 {
			return OutboxMaxBackoff
		}
	}
	return min(d, OutboxMaxBackoff)
}

// OutboxWorker delivers usage outbox rows to Delcore.
type OutboxWorker struct {
	db       *store.Store
	dc       Delcore
	log      *slog.Logger
	now      func() time.Time
	metrics  *Metrics
	interval time.Duration
	timeout  time.Duration
}

// Run delivers every MCPA_OUTBOX_INTERVAL until ctx ends.
func (w *OutboxWorker) Run(ctx context.Context) error {
	return every(ctx, w.interval, w.log, "outbox", func(ctx context.Context) error {
		_, err := w.Cycle(ctx)
		return err
	})
}

// CycleResult counts what one delivery cycle did.
type CycleResult struct {
	Delivered     int
	Retried       int
	Poisoned      int
	Expired       int64
	TenantDeleted int64
}

// Cycle runs one delivery cycle: it deletes expired rows, claims up to 100
// due pending rows oldest first, drops rows of tenants Delcore reports as
// DELETED and sends the others with IngestEvent. It then refreshes the
// gauges.
func (w *OutboxWorker) Cycle(ctx context.Context) (CycleResult, error) {
	var res CycleResult
	defer w.updateGauges(ctx)
	expired, err := w.deleteExpired(ctx)
	res.Expired = expired
	if err != nil {
		return res, err
	}
	claimCtx, cancel := context.WithTimeout(ctx, w.timeout)
	rows, err := w.db.ClaimOutbox(claimCtx, w.now(), OutboxBatchSize, outboxLease)
	cancel()
	if err != nil {
		w.metrics.outboxFailures.WithLabelValues(categoryStoreError).Inc()
		return res, err
	}
	if len(rows) == 0 {
		return res, nil
	}
	deleted := w.deletedTenants(ctx, rows)
	if len(deleted) > 0 {
		list := make([]string, 0, len(deleted))
		for t := range deleted {
			list = append(list, t)
		}
		n, err := store.DeleteOutboxForTenants(ctx, w.db.Pool, list)
		if err != nil {
			return res, err
		}
		res.TenantDeleted = n
		w.metrics.outboxFailures.WithLabelValues(categoryTenantDeleted).Add(float64(n))
		w.log.Info("dropped usage outbox rows of deleted tenants", "rows", n, "tenants", len(list))
	}
	for _, row := range rows {
		if ctx.Err() != nil {
			// Unsent rows become due again when their lease ends.
			return res, nil
		}
		if deleted[row.TenantID] {
			continue
		}
		w.deliver(ctx, row, &res)
	}
	return res, nil
}

func (w *OutboxWorker) deleteExpired(ctx context.Context) (int64, error) {
	ctx, cancel := context.WithTimeout(ctx, w.timeout)
	defer cancel()
	n, err := store.DeleteExpiredOutbox(ctx, w.db.Pool, w.now())
	if err != nil {
		w.metrics.outboxFailures.WithLabelValues(categoryStoreError).Inc()
		return 0, err
	}
	if n > 0 {
		w.metrics.outboxFailures.WithLabelValues(categoryExpired).Add(float64(n))
		w.log.Error("usage outbox rows expired undelivered; their charges are lost", "alert", true, "rows", n)
	}
	return n, nil
}

// deletedTenants asks Delcore for the states of the batch's tenants. When
// the check fails, no row is dropped and delivery proceeds.
func (w *OutboxWorker) deletedTenants(ctx context.Context, rows []store.OutboxRow) map[string]bool {
	seen := map[string]bool{}
	var ids []string
	for _, r := range rows {
		if !seen[r.TenantID] {
			seen[r.TenantID] = true
			ids = append(ids, r.TenantID)
		}
	}
	states, err := w.dc.GetTenantStates(ctx, ids)
	if err != nil {
		w.log.Warn("tenant state check of outbox rows failed; delivering without it", "error", err)
		return nil
	}
	out := map[string]bool{}
	for id, state := range states {
		if state == delcorev1.TenantState_TENANT_STATE_DELETED {
			out[id] = true
		}
	}
	return out
}

func (w *OutboxWorker) deliver(ctx context.Context, row store.OutboxRow, res *CycleResult) {
	// A fresh clock reading right before the send: an expired row is
	// deleted without sending and its charge is never estimated.
	if !w.now().Before(row.ExpiresAt) {
		if err := store.DeleteOutbox(ctx, w.db.Pool, row.ID); err != nil {
			w.log.Warn("deleting an expired outbox row failed", "event_key", row.EventKey, "error", err)
			return
		}
		res.Expired++
		w.metrics.outboxFailures.WithLabelValues(categoryExpired).Inc()
		w.log.Error("usage outbox row expired undelivered; its charge is lost", "alert", true, "event_key", row.EventKey)
		return
	}
	event := &delcorev1.Event{
		TenantId:   row.TenantID,
		EventKey:   row.EventKey,
		EventType:  pricing.EventType,
		OccurredAt: delcore.Timestamp(row.OccurredAt),
		Attributes: map[string]*delcorev1.AttributeValue{
			pricing.AttributeBytes:      {Value: &delcorev1.AttributeValue_Number{Number: strconv.FormatInt(row.Bytes, 10)}},
			pricing.AttributeResourceID: {Value: &delcorev1.AttributeValue_Text{Text: row.ResourceID.String()}},
		},
	}
	sendCtx, cancel := context.WithTimeout(ctx, w.timeout)
	_, err := w.dc.IngestEvent(sendCtx, row.RequestKey.String(), event)
	cancel()
	now := w.now()
	storeCtx, cancelStore := context.WithTimeout(context.WithoutCancel(ctx), w.timeout)
	defer cancelStore()
	if err == nil {
		if err := store.DeleteOutbox(storeCtx, w.db.Pool, row.ID); err != nil {
			// The row is resent after its lease; Delcore replays identical
			// events safely.
			w.log.Warn("deleting a delivered outbox row failed", "event_key", row.EventKey, "error", err)
		}
		res.Delivered++
		return
	}
	category, poison := classifyDelivery(err)
	w.metrics.outboxFailures.WithLabelValues(category).Inc()
	if poison {
		if err := store.PoisonOutbox(storeCtx, w.db.Pool, row.ID, now, category); err != nil {
			w.log.Warn("poisoning an outbox row failed", "event_key", row.EventKey, "error", err)
			return
		}
		res.Poisoned++
		w.log.Error("usage outbox row poisoned; Delcore will not accept it", "alert", true, "event_key", row.EventKey, "category", category)
		return
	}
	next := now.Add(Backoff(w.interval, row.Attempts+1))
	if err := store.RetryOutbox(storeCtx, w.db.Pool, row.ID, now, next, category); err != nil {
		w.log.Warn("scheduling an outbox retry failed", "event_key", row.EventKey, "error", err)
		return
	}
	res.Retried++
	w.log.Warn("usage outbox delivery failed; retrying", "event_key", row.EventKey, "category", category,
		"attempts", row.Attempts+1, "next_attempt_in", next.Sub(now).String())
}

func (w *OutboxWorker) updateGauges(ctx context.Context) {
	ctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), w.timeout)
	defer cancel()
	st, err := store.GetOutboxStats(ctx, w.db.Pool)
	if err != nil {
		w.log.Warn("reading outbox gauges failed", "error", err)
		return
	}
	w.metrics.outboxPending.Set(float64(st.Pending))
	w.metrics.outboxPoisoned.Set(float64(st.Poisoned))
	age := 0.0
	if !st.OldestPending.IsZero() {
		age = max(w.now().Sub(st.OldestPending).Seconds(), 0)
	}
	w.metrics.outboxOldestAge.Set(age)
	if st.Poisoned > 0 {
		w.log.Warn("poisoned usage outbox rows are waiting for expiry", "rows", st.Poisoned)
	}
}
