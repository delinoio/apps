// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"context"
	"math/big"

	"connectrpc.com/connect"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/pricing"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// budgetView builds a Budget from a limit and the billed bytes of a period.
func budgetView(limit string, billedBytes int64, period delcore.Period) *mcpav1.Budget {
	spent := pricing.SpendUSD(billedBytes)
	b := &mcpav1.Budget{
		LimitUsd:    limit,
		SpentUsd:    pricing.Format(spent, 6),
		PeriodStart: ts(period.Start),
		PeriodEnd:   ts(period.End),
	}
	if limit != "" {
		if l, ok := new(big.Rat).SetString(limit); ok {
			b.Exhausted = spent.Cmp(l) >= 0
		}
	}
	return b
}

func (s *Service) period(ctx context.Context, tenantID string) (delcore.Period, error) {
	p, err := s.dc.BillingPeriod(ctx, tenantID)
	if err != nil {
		return delcore.Period{}, apperr.DependencyUnavailable(err)
	}
	return p, nil
}

func parseLimit(raw string) (string, error) {
	if raw == "" {
		return "", nil
	}
	canonical, _, err := pricing.ParseLimit(raw)
	if err != nil {
		return "", apperr.InvalidInput("limit_usd: " + err.Error())
	}
	return canonical, nil
}

func (s *Service) resourceBudget(ctx context.Context, r store.Resource, db store.DB, period delcore.Period) (*mcpav1.Budget, error) {
	limit, err := store.GetResourceBudgetLimit(ctx, db, r.ID)
	if err != nil {
		return nil, dbErr(err)
	}
	var billed int64
	if spend := s.getHooks().Spend; spend != nil {
		if billed, err = spend.ResourceBilledBytes(ctx, r.ID, period.Start, period.End); err != nil {
			return nil, dbErr(err)
		}
	}
	return budgetView(normalizeStoredLimit(limit), billed, period), nil
}

func (s *Service) tenantBudget(ctx context.Context, tenantID string, db store.DB, period delcore.Period) (*mcpav1.Budget, error) {
	limit, err := store.GetTenantBudgetLimit(ctx, db, tenantID)
	if err != nil {
		return nil, dbErr(err)
	}
	var billed int64
	if spend := s.getHooks().Spend; spend != nil {
		if billed, err = spend.TenantBilledBytes(ctx, tenantID, period.Start, period.End); err != nil {
			return nil, dbErr(err)
		}
	}
	return budgetView(normalizeStoredLimit(limit), billed, period), nil
}

// normalizeStoredLimit renders a PostgreSQL numeric in canonical form. It only
// trims trailing fraction zeros (numeric keeps the scale it was stored with)
// and never rounds the stored limit.
func normalizeStoredLimit(v string) string {
	if canonical, ok := pricing.CanonicalDecimal(v); ok {
		return canonical
	}
	return v
}

// GetResourceBudget returns the resource budget and current spend.
func (s *Service) GetResourceBudget(ctx context.Context, req *connect.Request[mcpav1.GetResourceBudgetRequest]) (*connect.Response[mcpav1.GetResourceBudgetResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	period, err := s.period(ctx, acc.Resource.TenantID)
	if err != nil {
		return nil, err
	}
	b, err := s.resourceBudget(ctx, acc.Resource, s.db.Pool, period)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.GetResourceBudgetResponse{Metadata: apperr.Metadata(ctx), Budget: b}), nil
}

// SetResourceBudget sets or removes the resource budget.
func (s *Service) SetResourceBudget(ctx context.Context, req *connect.Request[mcpav1.SetResourceBudgetRequest]) (*connect.Response[mcpav1.SetResourceBudgetResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	limit, err := parseLimit(req.Msg.GetLimitUsd())
	if err != nil {
		return nil, err
	}
	acc, err := s.authz.Resource(ctx, p, req.Msg.GetResourceId(), roleManage, authz.AccessOptions{})
	if err != nil {
		return nil, err
	}
	period, err := s.period(ctx, acc.Resource.TenantID)
	if err != nil {
		return nil, err
	}
	err = s.mutate(ctx, p, acc.Resource.ID, roleManage, func(tx pgx.Tx, r store.Resource, _ mcpav1.ResourceRole) error {
		if err := store.SetResourceBudgetLimit(ctx, tx, r.ID, limit, p.UserID); err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: r.TenantID, ResourceID: r.ID, Action: mcpav1.AuditAction_AUDIT_ACTION_RESOURCE_BUDGET_CHANGED, TargetID: r.ID.String(),
			Details: map[string]string{"limit_usd": limit},
		})
	})
	if err != nil {
		return nil, err
	}
	b, err := s.resourceBudget(ctx, acc.Resource, s.db.Pool, period)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.SetResourceBudgetResponse{Metadata: apperr.Metadata(ctx), Budget: b}), nil
}

// GetTenantBudget returns the tenant budget. It requires the
// manage_tenant_budget capability.
func (s *Service) GetTenantBudget(ctx context.Context, req *connect.Request[mcpav1.GetTenantBudgetRequest]) (*connect.Response[mcpav1.GetTenantBudgetResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	tenant := req.Msg.GetTenantId()
	if err := s.authz.Capability(ctx, p, tenant, delcore.CapabilityManageTenantBudget); err != nil {
		return nil, err
	}
	period, err := s.period(ctx, tenant)
	if err != nil {
		return nil, err
	}
	b, err := s.tenantBudget(ctx, tenant, s.db.Pool, period)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.GetTenantBudgetResponse{Metadata: apperr.Metadata(ctx), Budget: b}), nil
}

// SetTenantBudget sets or removes the tenant budget.
func (s *Service) SetTenantBudget(ctx context.Context, req *connect.Request[mcpav1.SetTenantBudgetRequest]) (*connect.Response[mcpav1.SetTenantBudgetResponse], error) {
	p, err := principal(ctx)
	if err != nil {
		return nil, err
	}
	tenant := req.Msg.GetTenantId()
	limit, err := parseLimit(req.Msg.GetLimitUsd())
	if err != nil {
		return nil, err
	}
	if err := s.authz.Capability(ctx, p, tenant, delcore.CapabilityManageTenantBudget); err != nil {
		return nil, err
	}
	period, err := s.period(ctx, tenant)
	if err != nil {
		return nil, err
	}
	err = s.db.InTx(ctx, func(tx pgx.Tx) error {
		if err := store.SetTenantBudgetLimit(ctx, tx, tenant, limit, p.UserID); err != nil {
			return err
		}
		return WriteAudit(ctx, tx, p, AuditInput{
			TenantID: tenant, Action: mcpav1.AuditAction_AUDIT_ACTION_TENANT_BUDGET_CHANGED, TargetID: tenant,
			Details: map[string]string{"limit_usd": limit},
		})
	})
	if err != nil {
		return nil, dbErr(err)
	}
	b, err := s.tenantBudget(ctx, tenant, s.db.Pool, period)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&mcpav1.SetTenantBudgetResponse{Metadata: apperr.Metadata(ctx), Budget: b}), nil
}
