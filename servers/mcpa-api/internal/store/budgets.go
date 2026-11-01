// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"errors"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// GetResourceBudgetLimit returns the canonical limit or "" without a limit.
func GetResourceBudgetLimit(ctx context.Context, db DB, resourceID uuid.UUID) (string, error) {
	return scanLimit(db.QueryRow(ctx, `SELECT limit_usd::text FROM resource_budgets WHERE resource_id = $1`, resourceID))
}

// GetTenantBudgetLimit returns the canonical limit or "" without a limit.
func GetTenantBudgetLimit(ctx context.Context, db DB, tenantID string) (string, error) {
	return scanLimit(db.QueryRow(ctx, `SELECT limit_usd::text FROM tenant_budgets WHERE tenant_id = $1`, tenantID))
}

func scanLimit(row pgx.Row) (string, error) {
	var s string
	err := row.Scan(&s)
	if errors.Is(err, pgx.ErrNoRows) {
		return "", nil
	}
	return s, err
}

// SetResourceBudgetLimit stores a limit, or removes it when limit is "".
func SetResourceBudgetLimit(ctx context.Context, tx pgx.Tx, resourceID uuid.UUID, limit, userID string) error {
	if limit == "" {
		_, err := tx.Exec(ctx, `DELETE FROM resource_budgets WHERE resource_id = $1`, resourceID)
		return err
	}
	_, err := tx.Exec(ctx, `INSERT INTO resource_budgets (resource_id, limit_usd, updated_by_user_id) VALUES ($1, $2::numeric, $3)
		ON CONFLICT (resource_id) DO UPDATE SET limit_usd = EXCLUDED.limit_usd, updated_by_user_id = EXCLUDED.updated_by_user_id, update_time = now()`,
		resourceID, limit, userID)
	return err
}

// SetTenantBudgetLimit stores a limit, or removes it when limit is "".
func SetTenantBudgetLimit(ctx context.Context, tx pgx.Tx, tenantID, limit, userID string) error {
	if limit == "" {
		_, err := tx.Exec(ctx, `DELETE FROM tenant_budgets WHERE tenant_id = $1`, tenantID)
		return err
	}
	_, err := tx.Exec(ctx, `INSERT INTO tenant_budgets (tenant_id, limit_usd, updated_by_user_id) VALUES ($1, $2::numeric, $3)
		ON CONFLICT (tenant_id) DO UPDATE SET limit_usd = EXCLUDED.limit_usd, updated_by_user_id = EXCLUDED.updated_by_user_id, update_time = now()`,
		tenantID, limit, userID)
	return err
}
