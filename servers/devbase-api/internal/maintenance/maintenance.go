// SPDX-License-Identifier: FSL-1.1-ALv2

// Package maintenance runs the periodic cleanup cycle: final tenant deletion,
// replay expiry and audit retention.
package maintenance

import (
	"context"
	"log/slog"
	"time"

	"github.com/delinoio/apps/servers/devbase-api/internal/api"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"github.com/delinoio/apps/servers/devbase-api/internal/metrics"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

const (
	// AuditRetention is how long audit records are kept.
	AuditRetention = 90 * 24 * time.Hour
	// TenantBatch is the number of known tenants checked with Delcore per call.
	TenantBatch = 100
	purgeBatch  = 10000
)

type Worker struct {
	Pool      *pgxpool.Pool
	Authority delcore.Authority
	Log       *slog.Logger
	Metrics   *metrics.Metrics
	Now       func() time.Time
	Interval  time.Duration
}

func (w *Worker) now() time.Time {
	if w.Now != nil {
		return w.Now().UTC()
	}
	return time.Now().UTC()
}

// Run executes a cycle immediately and then every interval until the context ends.
func (w *Worker) Run(ctx context.Context) {
	ticker := time.NewTicker(w.Interval)
	defer ticker.Stop()
	for {
		w.Cycle(ctx)
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

// Cycle runs one maintenance pass. Failures are logged and counted and the next
// cycle retries them. Only an explicit DELETED tenant state deletes tenant data.
func (w *Worker) Cycle(ctx context.Context) {
	trace := uuid.Must(uuid.NewV7()).String()
	replays, e := w.purge(ctx, "DELETE FROM replays WHERE ctid IN (SELECT ctid FROM replays WHERE expires_at<=$1 LIMIT $2)", w.now())
	if e != nil {
		w.fail(ctx, trace, "replay_purge")
	}
	audits, e := w.purge(ctx, "DELETE FROM audit_events WHERE ctid IN (SELECT ctid FROM audit_events WHERE occurred_at<$1 LIMIT $2)", w.now().Add(-AuditRetention))
	if e != nil {
		w.fail(ctx, trace, "audit_purge")
	}
	tenants, projects := w.cleanup(ctx, trace)
	if w.Metrics != nil {
		w.Metrics.CleanupRuns.Inc()
	}
	w.Log.InfoContext(ctx, "maintenance cycle completed", "trace_id", trace, "expired_replays", replays, "expired_audit_events", audits, "deleted_tenants", tenants, "deleted_projects", projects)
}

func (w *Worker) fail(ctx context.Context, trace, category string) {
	if w.Metrics != nil {
		w.Metrics.CleanupFailures.Inc()
	}
	w.Log.WarnContext(ctx, "tenant cleanup failed", "trace_id", trace, "category", category)
}

func (w *Worker) purge(ctx context.Context, sql string, cutoff time.Time) (int64, error) {
	var total int64
	for {
		tag, e := w.Pool.Exec(ctx, sql, cutoff, purgeBatch)
		if e != nil {
			return total, e
		}
		total += tag.RowsAffected()
		if tag.RowsAffected() < purgeBatch {
			return total, nil
		}
	}
}

// cleanup checks known tenants in batches and deletes the data of finalized ones.
func (w *Worker) cleanup(ctx context.Context, trace string) (tenants, projects int) {
	after := "00000000-0000-0000-0000-000000000000"
	for {
		batch, e := w.knownTenants(ctx, after)
		if e != nil {
			w.fail(ctx, trace, "tenant_list")
			return
		}
		if len(batch) == 0 {
			return
		}
		after = batch[len(batch)-1]
		states, e := w.Authority.TenantStates(ctx, batch)
		if e != nil {
			// Unknown tenants, pending deletion and state-check failures never authorize cleanup.
			w.fail(ctx, trace, "state_check")
			continue
		}
		for _, tenant := range batch {
			if states[tenant] != delcore.StateDeleted {
				continue
			}
			n, e := w.deleteTenant(ctx, tenant, trace)
			if e != nil {
				w.fail(ctx, trace, "tenant_delete")
				continue
			}
			tenants++
			projects += n
		}
	}
}

func (w *Worker) knownTenants(ctx context.Context, after string) ([]string, error) {
	rows, e := w.Pool.Query(ctx, "SELECT DISTINCT tenant_id::text FROM projects WHERE tenant_id>$1::uuid ORDER BY 1 LIMIT $2", after, TenantBatch)
	if e != nil {
		return nil, e
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var id string
		if e = rows.Scan(&id); e != nil {
			return nil, e
		}
		out = append(out, id)
	}
	return out, rows.Err()
}

// deleteTenant removes a finalized tenant's projects, users and keys, and scrubs
// the replay records, in one transaction.
func (w *Worker) deleteTenant(ctx context.Context, tenant, trace string) (int, error) {
	tx, e := w.Pool.BeginTx(ctx, pgx.TxOptions{IsoLevel: pgx.ReadCommitted})
	if e != nil {
		return 0, e
	}
	defer tx.Rollback(ctx)
	if _, e = tx.Exec(ctx, "SELECT pg_advisory_xact_lock(hashtextextended($1,0))", "projects:"+tenant); e != nil {
		return 0, e
	}
	rows, e := tx.Query(ctx, "SELECT id::text FROM projects WHERE tenant_id=$1 FOR UPDATE", tenant)
	if e != nil {
		return 0, e
	}
	var ids []string
	for rows.Next() {
		var id string
		if e = rows.Scan(&id); e != nil {
			rows.Close()
			return 0, e
		}
		ids = append(ids, id)
	}
	rows.Close()
	if e = rows.Err(); e != nil {
		return 0, e
	}
	now := w.now().Truncate(time.Microsecond)
	for _, id := range ids {
		if _, e = tx.Exec(ctx, "INSERT INTO audit_events(id,occurred_at,action,actor,target_id,trace_id) VALUES($1,$2,'tenant.cleanup','system:cleanup',$3,$4)", uuid.Must(uuid.NewV7()).String(), now, id, trace); e != nil {
			return 0, e
		}
	}
	if _, e = tx.Exec(ctx, "DELETE FROM projects WHERE tenant_id=$1", tenant); e != nil {
		return 0, e
	}
	// Scrub only after the cascade. An already authorized child mutation holds its row
	// lock without the project lock, so the cascade waits for it to commit. A scrub that
	// ran earlier would miss the replay record that mutation inserts.
	if e = api.ScrubTenant(ctx, tx, tenant); e != nil {
		return 0, e
	}
	return len(ids), tx.Commit(ctx)
}
