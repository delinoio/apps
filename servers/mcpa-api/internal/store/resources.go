// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"errors"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// Resource is one customer resource with its routing settings.
type Resource struct {
	ID              uuid.UUID
	TenantID        string
	Name            string
	Description     string
	CreatedByUserID string
	CreateTime      time.Time
	UpdateTime      time.Time
	LoadBalancing   mcpav1.LoadBalancing
	ModelAllowlist  []string
	// DeleteTime is set on a read-only tombstone.
	DeleteTime *time.Time
}

// Deleted reports whether the resource is a tombstone.
func (r Resource) Deleted() bool { return r.DeleteTime != nil }

const resourceColumns = `r.id, r.tenant_id, r.name, r.description, r.created_by_user_id, r.create_time, r.update_time,
	COALESCE(rt.load_balancing, 1), COALESCE(rt.model_allowlist, '{}'), r.delete_time`

func scanResource(row pgx.Row, extra ...any) (Resource, error) {
	var r Resource
	var lb int16
	dest := append([]any{&r.ID, &r.TenantID, &r.Name, &r.Description, &r.CreatedByUserID, &r.CreateTime, &r.UpdateTime, &lb, &r.ModelAllowlist, &r.DeleteTime}, extra...)
	if err := row.Scan(dest...); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return Resource{}, ErrNotFound
		}
		return Resource{}, err
	}
	r.LoadBalancing = mcpav1.LoadBalancing(lb)
	if r.ModelAllowlist == nil {
		r.ModelAllowlist = []string{}
	}
	return r, nil
}

// InsertResource creates a resource and its default routing settings.
func InsertResource(ctx context.Context, tx pgx.Tx, r Resource) (Resource, error) {
	if r.ModelAllowlist == nil {
		r.ModelAllowlist = []string{}
	}
	err := tx.QueryRow(ctx, `INSERT INTO resources (id, tenant_id, name, description, created_by_user_id)
		VALUES ($1, $2, $3, $4, $5) RETURNING create_time, update_time`,
		r.ID, r.TenantID, r.Name, r.Description, r.CreatedByUserID).Scan(&r.CreateTime, &r.UpdateTime)
	if err != nil {
		return Resource{}, err
	}
	if _, err := tx.Exec(ctx, `INSERT INTO resource_routing (resource_id, load_balancing, model_allowlist) VALUES ($1, $2, $3)`,
		r.ID, int16(r.LoadBalancing), r.ModelAllowlist); err != nil {
		return Resource{}, err
	}
	return r, nil
}

// GetResource returns a resource, including a tombstone. Callers check
// Deleted before allowing mutations.
func GetResource(ctx context.Context, db DB, id uuid.UUID) (Resource, error) {
	return scanResource(db.QueryRow(ctx, `SELECT `+resourceColumns+`
		FROM resources r LEFT JOIN resource_routing rt ON rt.resource_id = r.id
		WHERE r.id = $1`, id))
}

// LockResource returns a resource, including a tombstone, and locks its row
// until the end of the transaction. Every mutation locks the resource first
// so it cannot race with deletion or ACL invariant checks.
func LockResource(ctx context.Context, tx pgx.Tx, id uuid.UUID) (Resource, error) {
	return scanResource(tx.QueryRow(ctx, `SELECT `+resourceColumns+`
		FROM resources r LEFT JOIN resource_routing rt ON rt.resource_id = r.id
		WHERE r.id = $1 FOR UPDATE OF r`, id))
}

// ResourceWithRoles is a resource with the caller's matching ACL roles.
type ResourceWithRoles struct {
	Resource
	Roles []mcpav1.ResourceRole
}

// ListResourcesForPrincipal lists live resources of a tenant on which the
// user, directly or through one of groupIDs, holds an ACL entry.
func ListResourcesForPrincipal(ctx context.Context, db DB, tenantID, userID string, groupIDs []string, after uuid.UUID, limit int) ([]ResourceWithRoles, error) {
	if groupIDs == nil {
		groupIDs = []string{}
	}
	rows, err := db.Query(ctx, `SELECT `+resourceColumns+`, roles.roles
		FROM resources r
		LEFT JOIN resource_routing rt ON rt.resource_id = r.id
		JOIN LATERAL (
			SELECT array_agg(a.role) AS roles FROM resource_acl a
			WHERE a.resource_id = r.id
			  AND ((a.principal_kind = 1 AND a.principal_id = $2) OR (a.principal_kind = 2 AND a.principal_id = ANY($3)))
		) roles ON roles.roles IS NOT NULL
		WHERE r.tenant_id = $1 AND r.delete_time IS NULL AND r.id > $4
		ORDER BY r.id LIMIT $5`, tenantID, userID, groupIDs, after, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []ResourceWithRoles
	for rows.Next() {
		var roles []int16
		r, err := scanResource(rows, &roles)
		if err != nil {
			return nil, err
		}
		item := ResourceWithRoles{Resource: r}
		for _, role := range roles {
			item.Roles = append(item.Roles, mcpav1.ResourceRole(role))
		}
		out = append(out, item)
	}
	return out, rows.Err()
}

// ResourceIDsForPrincipal returns resource IDs of a tenant, including
// tombstones, with the caller's matching ACL roles, without pagination. Usage
// and error views use it to bound queries to resources the caller can see.
func ResourceIDsForPrincipal(ctx context.Context, db DB, tenantID, userID string, groupIDs []string) (map[uuid.UUID][]mcpav1.ResourceRole, error) {
	if groupIDs == nil {
		groupIDs = []string{}
	}
	rows, err := db.Query(ctx, `SELECT a.resource_id, a.role FROM resource_acl a
		JOIN resources r ON r.id = a.resource_id
		WHERE r.tenant_id = $1
		  AND ((a.principal_kind = 1 AND a.principal_id = $2) OR (a.principal_kind = 2 AND a.principal_id = ANY($3)))`,
		tenantID, userID, groupIDs)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := map[uuid.UUID][]mcpav1.ResourceRole{}
	for rows.Next() {
		var id uuid.UUID
		var role int16
		if err := rows.Scan(&id, &role); err != nil {
			return nil, err
		}
		out[id] = append(out[id], mcpav1.ResourceRole(role))
	}
	return out, rows.Err()
}

// UpdateResource changes the name and description.
func UpdateResource(ctx context.Context, tx pgx.Tx, id uuid.UUID, name, description string) error {
	_, err := tx.Exec(ctx, `UPDATE resources SET name = $2, description = $3, update_time = now() WHERE id = $1 AND delete_time IS NULL`, id, name, description)
	return err
}

// UpdateRouting replaces the routing settings.
func UpdateRouting(ctx context.Context, tx pgx.Tx, id uuid.UUID, lb mcpav1.LoadBalancing, allowlist []string) error {
	if allowlist == nil {
		allowlist = []string{}
	}
	if _, err := tx.Exec(ctx, `INSERT INTO resource_routing (resource_id, load_balancing, model_allowlist, update_time)
		VALUES ($1, $2, $3, now())
		ON CONFLICT (resource_id) DO UPDATE SET load_balancing = EXCLUDED.load_balancing,
			model_allowlist = EXCLUDED.model_allowlist, update_time = now()`, id, int16(lb), allowlist); err != nil {
		return err
	}
	_, err := tx.Exec(ctx, `UPDATE resources SET update_time = now() WHERE id = $1`, id)
	return err
}

// DeletedResource lists what DeleteResource removed.
type DeletedResource struct {
	CredentialIDs []uuid.UUID
	CallKeyIDs    []uuid.UUID
}

// DeleteResource turns the resource into a read-only tombstone and
// immediately deletes its credentials, OAuth flows, settings, budget and call
// keys. The ACL stays so managers can still read retained usage, error and
// audit records. Retained records are not touched.
func DeleteResource(ctx context.Context, tx pgx.Tx, id uuid.UUID) (DeletedResource, error) {
	var out DeletedResource
	collect := func(sql string, dst *[]uuid.UUID) error {
		rows, err := tx.Query(ctx, sql, id)
		if err != nil {
			return err
		}
		ids, err := pgx.CollectRows(rows, pgx.RowTo[uuid.UUID])
		if err != nil {
			return err
		}
		*dst = ids
		return nil
	}
	if err := collect(`DELETE FROM credentials WHERE resource_id = $1 RETURNING id`, &out.CredentialIDs); err != nil {
		return out, err
	}
	if err := collect(`DELETE FROM call_keys WHERE resource_id = $1 RETURNING id`, &out.CallKeyIDs); err != nil {
		return out, err
	}
	for _, sql := range []string{
		`DELETE FROM provider_oauth_flows WHERE resource_id = $1`,
		`DELETE FROM resource_routing WHERE resource_id = $1`,
		`DELETE FROM resource_budgets WHERE resource_id = $1`,
		`UPDATE resources SET delete_time = now(), update_time = now() WHERE id = $1 AND delete_time IS NULL`,
	} {
		if _, err := tx.Exec(ctx, sql, id); err != nil {
			return out, err
		}
	}
	return out, nil
}
