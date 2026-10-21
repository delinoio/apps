// SPDX-License-Identifier: FSL-1.1-ALv2

package postgres

import (
	"context"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// System audit actors. Request handlers attribute records to the calling user
// or service; these actors attribute work that no request performs.
const (
	// ActorCatalog applies the development YAML catalog at startup.
	ActorCatalog = "system:catalog"
	// ActorMetering processes accepted metering work asynchronously.
	ActorMetering = "system:metering"
)

// Audit appends one audit record in the caller's transaction. An empty tenant
// stores no tenant. The reason must contain only safe, bounded diagnostics.
func Audit(ctx context.Context, tx pgx.Tx, actor, tenant, action, reason, correlation string, at time.Time) error {
	var t any
	if tenant != "" {
		t = tenant
	}
	_, err := tx.Exec(ctx, "INSERT INTO audit(id,actor_id,tenant_id,action,reason,correlation_id,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)", uuid.Must(uuid.NewV7()).String(), actor, t, action, reason, correlation, at)
	return err
}
