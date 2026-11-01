// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// retentionBatch bounds each retention DELETE statement.
const retentionBatch = 1000

// lifecycleBatch is the CheckTenantLifecycle request limit.
const lifecycleBatch = 100

type retentionRule struct {
	kind  string
	prune store.PruneBatch
}

var retentionRules = []retentionRule{
	{"call_records", store.PruneCallRecords},
	{"usage_spend", store.PruneUsageSpend},
	{"usage_daily", store.PruneUsageDaily},
	{"audit_events", store.PruneAuditEvents},
	{"provider_oauth_flows", store.PruneOAuthFlows},
	{"idempotency_records", store.PruneIdempotency},
	// Tombstones go last so records pruned in this cycle no longer hold
	// them.
	{"resource_tombstones", store.PruneTombstones},
}

// RunRetention prunes expired records every RetentionInterval until ctx
// ends.
func (dp *DataPlane) RunRetention(ctx context.Context) error {
	return every(ctx, dp.cfg.RetentionInterval, dp.log, "retention", dp.RetentionCycle)
}

// RetentionCycle deletes call records 30 days and budget spend entries 35
// days after the call start, daily aggregates and audit events after 13 months, OAuth flows one day after
// they end, expired request keys, expired console state and resource
// tombstones that no retained record refers to anymore.
func (dp *DataPlane) RetentionCycle(ctx context.Context) error {
	now := dp.now()
	for _, rule := range retentionRules {
		var total int64
		for {
			stepCtx, cancel := context.WithTimeout(ctx, dp.cfg.DependencyTimeout)
			n, err := rule.prune(stepCtx, dp.db.Pool, now, retentionBatch)
			cancel()
			if err != nil {
				return err
			}
			total += n
			if n < retentionBatch || ctx.Err() != nil {
				break
			}
		}
		if total > 0 {
			dp.log.Info("retention pruned records", "kind", rule.kind, "rows", total)
		}
	}
	stepCtx, cancel := context.WithTimeout(ctx, dp.cfg.DependencyTimeout)
	defer cancel()
	return store.DeleteExpiredConsoleState(stepCtx, dp.db.Pool)
}

// RunLifecycle checks tenant lifecycles every LifecycleInterval until ctx
// ends.
func (dp *DataPlane) RunLifecycle(ctx context.Context) error {
	return every(ctx, dp.cfg.LifecycleInterval, dp.log, "tenant_lifecycle", func(ctx context.Context) error {
		_, err := dp.LifecycleCycle(ctx)
		return err
	})
}

// LifecycleCycle asks Delcore, through CheckTenantLifecycle, which tenants
// with retained data have finalized deletion and deletes all of their data.
// Only tenants Delcore reports as finalized (DELETED) are touched; UNKNOWN
// states, pending deletion and errors delete nothing. It returns the
// deleted tenant IDs.
func (dp *DataPlane) LifecycleCycle(ctx context.Context) ([]string, error) {
	var deleted []string
	after := ""
	for {
		stepCtx, cancel := context.WithTimeout(ctx, dp.cfg.DependencyTimeout)
		tenants, err := store.TenantsWithData(stepCtx, dp.db.Pool, after, lifecycleBatch)
		cancel()
		if err != nil || len(tenants) == 0 {
			return deleted, err
		}
		after = tenants[len(tenants)-1]
		finalized, err := dp.cfg.Delcore.CheckTenantLifecycle(ctx, tenants)
		if err != nil {
			// Fail safe: nothing is deleted while Delcore cannot confirm.
			dp.log.Warn("tenant lifecycle check failed; nothing deleted", "tenants", len(tenants), "error", err)
			return deleted, nil
		}
		asked := map[string]bool{}
		for _, t := range tenants {
			asked[t] = true
		}
		for _, tenantID := range finalized {
			if !asked[tenantID] {
				continue
			}
			if err := dp.deleteTenant(ctx, tenantID); err != nil {
				return deleted, err
			}
			deleted = append(deleted, tenantID)
		}
		if len(tenants) < lifecycleBatch || ctx.Err() != nil {
			return deleted, nil
		}
	}
}

func (dp *DataPlane) deleteTenant(ctx context.Context, tenantID string) error {
	stepCtx, cancel := context.WithTimeout(ctx, 4*dp.cfg.DependencyTimeout)
	defer cancel()
	d, err := dp.db.DeleteTenantData(stepCtx, tenantID, func(tx pgx.Tx, resourceIDs []uuid.UUID) error {
		for _, id := range resourceIDs {
			if err := dp.Sync.CredentialsChanging(stepCtx, tx, id); err != nil {
				return err
			}
		}
		return nil
	})
	if err != nil {
		return err
	}
	for _, id := range d.ResourceIDs {
		dp.Sync.invalidateResource(id)
	}
	dp.log.Info("deleted the data of a deleted tenant", "tenant_id", tenantID, "resources", len(d.ResourceIDs),
		"call_records", d.CallRecords, "aggregates", d.Aggregates, "audit_events", d.AuditEvents, "outbox_rows", d.Outbox)
	return nil
}
