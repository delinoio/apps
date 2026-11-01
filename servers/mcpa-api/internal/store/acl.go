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

// ACLEntry is one per-resource role assignment.
type ACLEntry struct {
	ResourceID      uuid.UUID
	PrincipalKind   mcpav1.PrincipalKind
	PrincipalID     string
	Role            mcpav1.ResourceRole
	CreatedByUserID string
	CreateTime      time.Time
}

// ListACL returns every ACL entry of a resource ordered by kind and ID.
func ListACL(ctx context.Context, db DB, resourceID uuid.UUID) ([]ACLEntry, error) {
	rows, err := db.Query(ctx, `SELECT resource_id, principal_kind, principal_id, role, created_by_user_id, create_time
		FROM resource_acl WHERE resource_id = $1 ORDER BY principal_kind, principal_id`, resourceID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []ACLEntry
	for rows.Next() {
		var e ACLEntry
		var kind, role int16
		if err := rows.Scan(&e.ResourceID, &kind, &e.PrincipalID, &role, &e.CreatedByUserID, &e.CreateTime); err != nil {
			return nil, err
		}
		e.PrincipalKind = mcpav1.PrincipalKind(kind)
		e.Role = mcpav1.ResourceRole(role)
		out = append(out, e)
	}
	return out, rows.Err()
}

// PutACL inserts or updates one entry. An update keeps the original creator
// and creation time.
func PutACL(ctx context.Context, tx pgx.Tx, e ACLEntry) (ACLEntry, error) {
	err := tx.QueryRow(ctx, `INSERT INTO resource_acl (resource_id, principal_kind, principal_id, role, created_by_user_id)
		VALUES ($1, $2, $3, $4, $5)
		ON CONFLICT (resource_id, principal_kind, principal_id) DO UPDATE SET role = EXCLUDED.role
		RETURNING created_by_user_id, create_time`,
		e.ResourceID, int16(e.PrincipalKind), e.PrincipalID, int16(e.Role), e.CreatedByUserID).Scan(&e.CreatedByUserID, &e.CreateTime)
	return e, err
}

// RemoveACL deletes one entry and reports whether it existed.
func RemoveACL(ctx context.Context, tx pgx.Tx, resourceID uuid.UUID, kind mcpav1.PrincipalKind, principalID string) (bool, error) {
	tag, err := tx.Exec(ctx, `DELETE FROM resource_acl WHERE resource_id = $1 AND principal_kind = $2 AND principal_id = $3`,
		resourceID, int16(kind), principalID)
	return tag.RowsAffected() == 1, err
}

// Group is one Delcore group in a tenant snapshot.
type Group struct {
	ID      string
	Name    string
	UserIDs []string
}

// SnapshotRefreshTime returns when the tenant's group snapshot was last
// refreshed, or the zero time when it was never refreshed.
func SnapshotRefreshTime(ctx context.Context, db DB, tenantID string) (time.Time, error) {
	var t time.Time
	err := db.QueryRow(ctx, `SELECT refresh_time FROM tenant_group_snapshots WHERE tenant_id = $1`, tenantID).Scan(&t)
	if errors.Is(err, pgx.ErrNoRows) {
		return time.Time{}, nil
	}
	return t, err
}

// ReplaceGroupSnapshot stores the complete group list of a tenant.
func ReplaceGroupSnapshot(ctx context.Context, tx pgx.Tx, tenantID string, groups []Group, at time.Time) error {
	// The snapshot row lock serializes concurrent refreshes of one tenant.
	if _, err := tx.Exec(ctx, `INSERT INTO tenant_group_snapshots (tenant_id, refresh_time) VALUES ($1, $2)
		ON CONFLICT (tenant_id) DO UPDATE SET refresh_time = EXCLUDED.refresh_time`, tenantID, at); err != nil {
		return err
	}
	if _, err := tx.Exec(ctx, `DELETE FROM tenant_groups WHERE tenant_id = $1`, tenantID); err != nil {
		return err
	}
	for _, g := range groups {
		if _, err := tx.Exec(ctx, `INSERT INTO tenant_groups (tenant_id, group_id, name) VALUES ($1, $2, $3)
			ON CONFLICT DO NOTHING`, tenantID, g.ID, g.Name); err != nil {
			return err
		}
		if len(g.UserIDs) > 0 {
			if _, err := tx.Exec(ctx, `INSERT INTO tenant_group_members (tenant_id, group_id, user_id)
				SELECT $1, $2, u FROM unnest($3::text[]) AS u ON CONFLICT DO NOTHING`, tenantID, g.ID, g.UserIDs); err != nil {
				return err
			}
		}
	}
	return nil
}

// UserGroupIDs returns the snapshot groups of a user in a tenant.
func UserGroupIDs(ctx context.Context, db DB, tenantID, userID string) ([]string, error) {
	rows, err := db.Query(ctx, `SELECT group_id FROM tenant_group_members WHERE tenant_id = $1 AND user_id = $2 ORDER BY group_id`, tenantID, userID)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, pgx.RowTo[string])
}

// SnapshotGroups returns the snapshot groups of a tenant without members.
func SnapshotGroups(ctx context.Context, db DB, tenantID string) ([]Group, error) {
	rows, err := db.Query(ctx, `SELECT group_id, name FROM tenant_groups WHERE tenant_id = $1 ORDER BY group_id`, tenantID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Group
	for rows.Next() {
		var g Group
		if err := rows.Scan(&g.ID, &g.Name); err != nil {
			return nil, err
		}
		out = append(out, g)
	}
	return out, rows.Err()
}
