// SPDX-License-Identifier: FSL-1.1-ALv2

package worker

import (
	"context"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/jackc/pgx/v5"
)

func (w *Worker) retain(ctx context.Context, tx pgx.Tx, now time.Time) error {
	if w.Config.AuditRetentionSeconds > 0 {
		if _, e := tx.Exec(ctx, "DELETE FROM audit WHERE created_at<$1", now.Add(-time.Duration(w.Config.AuditRetentionSeconds)*time.Second)); e != nil {
			return e
		}
	}
	// Normalized quantities and price attribution are accounting evidence; keep
	// them past raw-usage retention while their accounting retention is required.
	cutoff := now.Add(-time.Duration(max(w.Config.RawUsageRetentionSeconds, w.Config.AccountingRetentionSeconds)) * time.Second)
	if w.Config.AccountingRetentionSeconds <= 0 || w.Config.RawUsageRetentionSeconds <= 0 {
		return nil
	}
	if _, e := tx.Exec(ctx, "SELECT set_config('delcore.retention','on',true)"); e != nil {
		return e
	}
	if _, e := tx.Exec(ctx, `INSERT INTO usage_replay SELECT service_id,event_key,id,fingerprint FROM usage_events WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1) ON CONFLICT DO NOTHING`, cutoff); e != nil {
		return e
	}
	if _, e := tx.Exec(ctx, "DELETE FROM ledger WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)", cutoff); e != nil {
		return e
	}
	if _, e := tx.Exec(ctx, "DELETE FROM usage_events WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)", cutoff); e != nil {
		return e
	}
	if _, e := tx.Exec(ctx, "UPDATE provider_transactions SET amount=0 WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)", cutoff); e != nil {
		return e
	}
	if _, e := tx.Exec(ctx, "DELETE FROM settlements WHERE tenant_id IN (SELECT id FROM tenants WHERE deleted_at<$1)", cutoff); e != nil {
		return e
	}
	// Raw event attributes are not accounting evidence. Clear them at the
	// raw-usage boundary after final deletion, before the accounting cutoff.
	rawCutoff := now.Add(-time.Duration(w.Config.RawUsageRetentionSeconds) * time.Second)
	if cleared, e := events.RemoveAttributes(ctx, tx, rawCutoff); e != nil {
		return e
	} else if cleared > 0 {
		w.Log.InfoContext(ctx, "raw metering attributes removed", "category", "retention", "revisions", cleared)
	}
	// Event identities, revision history, and derived rows follow the later
	// raw-usage and accounting cutoff. Replay identities remain, and tenants
	// with unfinished metering work are kept.
	if removed, e := events.Retain(ctx, tx, cutoff); e != nil {
		return e
	} else if removed > 0 {
		w.Log.InfoContext(ctx, "metering history retained as replay identities", "category", "retention", "tenants", removed)
	}
	if _, e := tx.Exec(ctx, "SELECT set_config('delcore.retention','off',true)"); e != nil {
		return e
	}

	return nil
}
