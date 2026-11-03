// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"strconv"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// PruneBatch deletes up to limit rows of one retention rule. Callers repeat
// it until it returns fewer than limit rows so each statement stays short.
type PruneBatch func(ctx context.Context, db DB, now time.Time, limit int) (int64, error)

func pruneBatch(table, where string) PruneBatch {
	return func(ctx context.Context, db DB, now time.Time, limit int) (int64, error) {
		tag, err := db.Exec(ctx, `DELETE FROM `+table+` WHERE ctid IN (SELECT ctid FROM `+table+` WHERE `+where+` LIMIT $2)`, now, limit)
		return tag.RowsAffected(), err
	}
}

// Retention rules of the contract. Each uses $1 as the current time.
var (
	// PruneCallRecords deletes call records 30 days after the call start.
	PruneCallRecords = pruneBatch("call_records", `expire_time <= $1`)
	// PruneUsageSpend deletes budget spend entries 35 days after the call
	// start (SpendRetention).
	PruneUsageSpend = pruneBatch("usage_spend", `start_time <= $1::timestamptz - interval '35 days'`)
	// PruneUsageDaily deletes daily aggregates older than 13 months.
	PruneUsageDaily = pruneBatch("usage_daily", `day < (($1::timestamptz - interval '13 months') AT TIME ZONE 'UTC')::date`)
	// PruneAuditEvents deletes audit events older than 13 months.
	PruneAuditEvents = pruneBatch("audit_events", `occur_time < $1::timestamptz - interval '13 months'`)
	// PruneOAuthFlows deletes provider OAuth flows one day after they
	// completed, failed or expired.
	PruneOAuthFlows = pruneBatch("provider_oauth_flows",
		`(state <> 1 AND update_time < $1::timestamptz - interval '1 day') OR expire_time < $1::timestamptz - interval '1 day'`)
	// PruneIdempotency deletes request-key records 30 days after creation.
	PruneIdempotency = pruneBatch("idempotency_records", `expire_time <= $1`)
	// PruneTombstones deletes resource tombstones (and, by cascade, their
	// ACL) once no retained usage, error, audit or outbox record refers to
	// them. Until then they stay read-only so former roles can read the
	// retained records.
	PruneTombstones = pruneBatch("resources", `delete_time IS NOT NULL AND delete_time <= $1
		AND NOT EXISTS (SELECT 1 FROM call_records c WHERE c.resource_id = resources.id)
		AND NOT EXISTS (SELECT 1 FROM usage_daily u WHERE u.resource_id = resources.id)
		AND NOT EXISTS (SELECT 1 FROM audit_events a WHERE a.resource_id = resources.id)
		AND NOT EXISTS (SELECT 1 FROM usage_outbox o WHERE o.resource_id = resources.id)`)
)

// TenantsWithData lists tenant IDs that own retained mcpa data, in ID order
// after the cursor. Every call record, aggregate, outbox row and resource
// audit event belongs to a resource row (live or tombstone), so resources,
// tenant budgets, group snapshots and tenant-level audit events cover all
// retained tenant data.
func TenantsWithData(ctx context.Context, db DB, after string, limit int) ([]string, error) {
	rows, err := db.Query(ctx, `SELECT tenant_id FROM (
			SELECT tenant_id FROM resources
			UNION SELECT tenant_id FROM tenant_budgets
			UNION SELECT tenant_id FROM tenant_group_snapshots
			UNION SELECT tenant_id FROM audit_events WHERE resource_id IS NULL
			UNION SELECT tenant_id FROM usage_outbox
		) t WHERE tenant_id > $1 ORDER BY tenant_id LIMIT $2`, after, limit)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, pgx.RowTo[string])
}

// TenantDeletion counts what DeleteTenantData removed.
type TenantDeletion struct {
	ResourceIDs []uuid.UUID
	CallRecords int64
	Aggregates  int64
	AuditEvents int64
	Outbox      int64
}

// DeleteTenantData deletes every retained row of a tenant in one
// transaction. Callers must only call it after Delcore reported the tenant
// as DELETED. When deleted is not nil it runs in that transaction with the
// IDs of the deleted resources; an error rolls the deletion back.
func (s *Store) DeleteTenantData(ctx context.Context, tenantID string, deleted func(tx pgx.Tx, resourceIDs []uuid.UUID) error) (TenantDeletion, error) {
	var d TenantDeletion
	err := s.InTx(ctx, func(tx pgx.Tx) error {
		if s.timeout > 0 {
			// One deletion runs several statements and cascades, so it keeps
			// the 4*DependencyTimeout bound of the caller instead of the
			// per-statement dependency timeout that serve mode sets for
			// every other call. The setting ends with the transaction.
			ms := strconv.FormatInt(timeoutMillis(4*s.timeout), 10)
			if _, err := tx.Exec(ctx, `SELECT set_config('statement_timeout', $1, true), set_config('lock_timeout', $1, true)`, ms); err != nil {
				return err
			}
		}
		count := func(dst *int64, sql string) error {
			tag, err := tx.Exec(ctx, sql, tenantID)
			if dst != nil {
				*dst = tag.RowsAffected()
			}
			return err
		}
		steps := []struct {
			dst *int64
			sql string
		}{
			{&d.Outbox, `DELETE FROM usage_outbox WHERE tenant_id = $1`},
			{&d.CallRecords, `DELETE FROM call_records WHERE tenant_id = $1`},
			{nil, `DELETE FROM usage_spend WHERE tenant_id = $1`},
			{&d.Aggregates, `DELETE FROM usage_daily WHERE tenant_id = $1`},
			{&d.AuditEvents, `DELETE FROM audit_events WHERE tenant_id = $1`},
			{nil, `DELETE FROM tenant_budgets WHERE tenant_id = $1`},
			{nil, `DELETE FROM tenant_group_snapshots WHERE tenant_id = $1`},
		}
		for _, step := range steps {
			if err := count(step.dst, step.sql); err != nil {
				return err
			}
		}
		// Cascades to routing, ACL, credentials, OAuth flows, call keys and
		// resource budgets.
		rows, err := tx.Query(ctx, `DELETE FROM resources WHERE tenant_id = $1 RETURNING id`, tenantID)
		if err != nil {
			return err
		}
		if d.ResourceIDs, err = pgx.CollectRows(rows, pgx.RowTo[uuid.UUID]); err != nil {
			return err
		}
		if deleted != nil {
			return deleted(tx, d.ResourceIDs)
		}
		return nil
	})
	return d, err
}
