// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// OutboxState is the delivery state of an outbox row.
type OutboxState int16

// Outbox states stored in usage_outbox.state.
const (
	OutboxPending  OutboxState = 1
	OutboxPoisoned OutboxState = 2
)

// OutboxRow is one undelivered Delcore traffic event.
type OutboxRow struct {
	ID         uuid.UUID
	EventKey   string
	RequestKey uuid.UUID
	TenantID   string
	ResourceID uuid.UUID
	OccurredAt time.Time
	Bytes      int64
	Attempts   int
	CreateTime time.Time
	ExpiresAt  time.Time
}

// DeleteExpiredOutbox deletes every row, pending or poisoned, whose expiry
// has passed. Their charges are lost and never estimated.
func DeleteExpiredOutbox(ctx context.Context, db DB, now time.Time) (int64, error) {
	tag, err := db.Exec(ctx, `DELETE FROM usage_outbox WHERE expires_at <= $1`, now)
	return tag.RowsAffected(), err
}

// ClaimOutbox leases up to limit due pending rows, oldest first. Rows locked
// by another replica are skipped, and a claimed row is not due again until
// the lease ends, so concurrent workers never send the same row at once. A
// crashed worker's rows become due again after the lease.
func (s *Store) ClaimOutbox(ctx context.Context, now time.Time, limit int, lease time.Duration) ([]OutboxRow, error) {
	var out []OutboxRow
	err := s.InTx(ctx, func(tx pgx.Tx) error {
		rows, err := tx.Query(ctx, `SELECT id, event_key, request_key, tenant_id, resource_id, occurred_at, bytes, attempts,
				create_time, expires_at
			FROM usage_outbox WHERE state = 1 AND next_attempt_at <= $1
			ORDER BY next_attempt_at, id LIMIT $2 FOR UPDATE SKIP LOCKED`, now, limit)
		if err != nil {
			return err
		}
		out, err = pgx.CollectRows(rows, func(row pgx.CollectableRow) (OutboxRow, error) {
			var r OutboxRow
			err := row.Scan(&r.ID, &r.EventKey, &r.RequestKey, &r.TenantID, &r.ResourceID, &r.OccurredAt, &r.Bytes, &r.Attempts,
				&r.CreateTime, &r.ExpiresAt)
			return r, err
		})
		if err != nil || len(out) == 0 {
			return err
		}
		ids := make([]uuid.UUID, len(out))
		for i, r := range out {
			ids[i] = r.ID
		}
		_, err = tx.Exec(ctx, `UPDATE usage_outbox SET next_attempt_at = $2 WHERE id = ANY($1)`, ids, now.Add(lease))
		return err
	})
	return out, err
}

// DeleteOutbox removes a delivered or expired row.
func DeleteOutbox(ctx context.Context, db DB, id uuid.UUID) error {
	_, err := db.Exec(ctx, `DELETE FROM usage_outbox WHERE id = $1`, id)
	return err
}

// RetryOutbox records a failed attempt and schedules the next one.
func RetryOutbox(ctx context.Context, db DB, id uuid.UUID, at, next time.Time, category string) error {
	_, err := db.Exec(ctx, `UPDATE usage_outbox SET attempts = attempts + 1, last_attempt_at = $2, next_attempt_at = $3,
		last_error = $4 WHERE id = $1 AND state = 1`, id, at, next, TruncateUTF8(category, 64))
	return err
}

// PoisonOutbox marks a row that Delcore will never accept. It stays until
// expiry and is never sent again.
func PoisonOutbox(ctx context.Context, db DB, id uuid.UUID, at time.Time, category string) error {
	_, err := db.Exec(ctx, `UPDATE usage_outbox SET state = 2, attempts = attempts + 1, last_attempt_at = $2, last_error = $3
		WHERE id = $1`, id, at, TruncateUTF8(category, 64))
	return err
}

// DeleteOutboxForTenants removes the rows of tenants that Delcore reported
// as deleted.
func DeleteOutboxForTenants(ctx context.Context, db DB, tenantIDs []string) (int64, error) {
	if len(tenantIDs) == 0 {
		return 0, nil
	}
	tag, err := db.Exec(ctx, `DELETE FROM usage_outbox WHERE tenant_id = ANY($1)`, tenantIDs)
	return tag.RowsAffected(), err
}

// OutboxStats describes the outbox for the gauges.
type OutboxStats struct {
	Pending  int64
	Poisoned int64
	// OldestPending is the creation time of the oldest pending row, or zero.
	OldestPending time.Time
}

// GetOutboxStats reads the outbox gauges.
func GetOutboxStats(ctx context.Context, db DB) (OutboxStats, error) {
	var st OutboxStats
	var oldest *time.Time
	err := db.QueryRow(ctx, `SELECT count(*) FILTER (WHERE state = 1), count(*) FILTER (WHERE state = 2),
		min(create_time) FILTER (WHERE state = 1) FROM usage_outbox`).Scan(&st.Pending, &st.Poisoned, &oldest)
	if oldest != nil {
		st.OldestPending = *oldest
	}
	return st, err
}
