// SPDX-License-Identifier: FSL-1.1-ALv2

package worker

import (
	"context"
	"github.com/jackc/pgx/v5"
	"time"
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
	if _, e := tx.Exec(ctx, "DELETE FROM ai_usage_prices WHERE usage_id IN (SELECT u.id FROM usage_events u JOIN tenants t ON t.id=u.tenant_id WHERE t.deleted_at<$1)", cutoff); e != nil {
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
	if _, e := tx.Exec(ctx, "SELECT set_config('delcore.retention','off',true)"); e != nil {
		return e
	}

	return nil
}
