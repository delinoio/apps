// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"errors"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// CallKey is the stored metadata of a call key. SecretHash is the SHA-256
// digest of the full secret and is never returned to callers.
type CallKey struct {
	ID           uuid.UUID
	ResourceID   uuid.UUID
	TenantID     string
	Name         string
	SecretHash   []byte
	IssuerUserID string
	CreateTime   time.Time
	ExpireTime   *time.Time
	LastUsedTime *time.Time
	RevokeTime   *time.Time
}

const callKeyColumns = `id, resource_id, tenant_id, name, secret_hash, issuer_user_id, create_time, expire_time, last_used_time, revoke_time`

func scanCallKey(row pgx.Row) (CallKey, error) {
	var k CallKey
	err := row.Scan(&k.ID, &k.ResourceID, &k.TenantID, &k.Name, &k.SecretHash, &k.IssuerUserID, &k.CreateTime, &k.ExpireTime, &k.LastUsedTime, &k.RevokeTime)
	if errors.Is(err, pgx.ErrNoRows) {
		return CallKey{}, ErrNotFound
	}
	return k, err
}

// InsertCallKey stores a new key.
func InsertCallKey(ctx context.Context, tx pgx.Tx, k CallKey) (CallKey, error) {
	err := tx.QueryRow(ctx, `INSERT INTO call_keys (id, resource_id, tenant_id, name, secret_hash, issuer_user_id, expire_time)
		VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING create_time`,
		k.ID, k.ResourceID, k.TenantID, k.Name, k.SecretHash, k.IssuerUserID, k.ExpireTime).Scan(&k.CreateTime)
	return k, err
}

// GetCallKey returns one key of a resource.
func GetCallKey(ctx context.Context, db DB, resourceID, id uuid.UUID) (CallKey, error) {
	return scanCallKey(db.QueryRow(ctx, `SELECT `+callKeyColumns+` FROM call_keys WHERE resource_id = $1 AND id = $2`, resourceID, id))
}

// LookupCallKey returns a key by ID for call-path verification. The caller
// must compare the presented secret with SecretHash in constant time.
func LookupCallKey(ctx context.Context, db DB, id uuid.UUID) (CallKey, error) {
	return scanCallKey(db.QueryRow(ctx, `SELECT `+callKeyColumns+` FROM call_keys WHERE id = $1`, id))
}

// ListCallKeys lists a resource's keys after the cursor. A non-empty
// issuerUserID restricts the list to that issuer.
func ListCallKeys(ctx context.Context, db DB, resourceID uuid.UUID, issuerUserID string, after uuid.UUID, limit int) ([]CallKey, error) {
	rows, err := db.Query(ctx, `SELECT `+callKeyColumns+` FROM call_keys
		WHERE resource_id = $1 AND ($2 = '' OR issuer_user_id = $2) AND id > $3
		ORDER BY id LIMIT $4`, resourceID, issuerUserID, after, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []CallKey
	for rows.Next() {
		k, err := scanCallKey(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, k)
	}
	return out, rows.Err()
}

// RevokeCallKey sets the revocation time once. Revoking a revoked key keeps
// the first revocation time and reports changed=false.
func RevokeCallKey(ctx context.Context, tx pgx.Tx, resourceID, id uuid.UUID) (CallKey, bool, error) {
	k, err := scanCallKey(tx.QueryRow(ctx, `UPDATE call_keys SET revoke_time = now()
		WHERE resource_id = $1 AND id = $2 AND revoke_time IS NULL RETURNING `+callKeyColumns, resourceID, id))
	if err == nil {
		return k, true, nil
	}
	if !errors.Is(err, ErrNotFound) {
		return CallKey{}, false, err
	}
	k, err = GetCallKey(ctx, tx, resourceID, id)
	return k, false, err
}

// TouchCallKey records a use at most once per minute per key.
func TouchCallKey(ctx context.Context, db DB, id uuid.UUID, at time.Time) error {
	// The explicit cast keeps PostgreSQL from inferring $2 as an interval
	// from the subtraction.
	_, err := db.Exec(ctx, `UPDATE call_keys SET last_used_time = $2::timestamptz
		WHERE id = $1 AND (last_used_time IS NULL OR last_used_time <= $2::timestamptz - interval '1 minute')`, id, at)
	return err
}
