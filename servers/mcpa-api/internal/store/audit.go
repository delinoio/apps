// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"encoding/json"
	"errors"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// AuditEvent is one customer-visible audit record. Details hold bounded,
// secret-free attributes.
type AuditEvent struct {
	ID          uuid.UUID
	TenantID    string
	ResourceID  uuid.UUID // uuid.Nil for tenant-level events
	Action      mcpav1.AuditAction
	ActorKind   mcpav1.ActorKind
	ActorUserID string
	TargetID    string
	Details     map[string]string
	OccurTime   time.Time
}

// InsertAudit writes an audit event in the caller's transaction so it
// commits atomically with the audited change.
func InsertAudit(ctx context.Context, db DB, e AuditEvent) (AuditEvent, error) {
	if e.ID == uuid.Nil {
		e.ID = NewID()
	}
	if e.Details == nil {
		e.Details = map[string]string{}
	}
	details, err := json.Marshal(e.Details)
	if err != nil {
		return e, err
	}
	var resourceID *uuid.UUID
	if e.ResourceID != uuid.Nil {
		resourceID = &e.ResourceID
	}
	err = db.QueryRow(ctx, `INSERT INTO audit_events (id, tenant_id, resource_id, action, actor_kind, actor_user_id, target_id, details)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING occur_time`,
		e.ID, e.TenantID, resourceID, int16(e.Action), int16(e.ActorKind), e.ActorUserID, e.TargetID, details).Scan(&e.OccurTime)
	return e, err
}

// ListAudit returns a resource's events newest first. before is the cursor
// (uuid.Nil for the first page).
func ListAudit(ctx context.Context, db DB, resourceID, before uuid.UUID, limit int) ([]AuditEvent, error) {
	rows, err := db.Query(ctx, `SELECT id, tenant_id, resource_id, action, actor_kind, actor_user_id, target_id, details, occur_time
		FROM audit_events WHERE resource_id = $1 AND ($2 = '00000000-0000-0000-0000-000000000000'::uuid OR id < $2)
		ORDER BY id DESC LIMIT $3`, resourceID, before, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []AuditEvent
	for rows.Next() {
		var e AuditEvent
		var rid *uuid.UUID
		var action, actor int16
		var details []byte
		if err := rows.Scan(&e.ID, &e.TenantID, &rid, &action, &actor, &e.ActorUserID, &e.TargetID, &details, &e.OccurTime); err != nil {
			return nil, err
		}
		if rid != nil {
			e.ResourceID = *rid
		}
		e.Action = mcpav1.AuditAction(action)
		e.ActorKind = mcpav1.ActorKind(actor)
		if err := json.Unmarshal(details, &e.Details); err != nil {
			return nil, err
		}
		out = append(out, e)
	}
	return out, rows.Err()
}

// IdempotencyTTL is how long request keys are remembered.
const IdempotencyTTL = 30 * 24 * time.Hour

// IdempotencyRecord is a remembered request.
type IdempotencyRecord struct {
	Fingerprint []byte
	ResultID    uuid.UUID
}

// ClaimIdempotency records a request key for a new result. When the key was
// already used and has not expired, it returns the existing record and
// claimed=false; the caller must compare fingerprints. A concurrent first use
// blocks until the other transaction finishes.
func ClaimIdempotency(ctx context.Context, tx pgx.Tx, operation, principalID string, requestKey uuid.UUID, fingerprint []byte, resultID uuid.UUID) (IdempotencyRecord, bool, error) {
	// Expired records are replaced so a key can be reused after 30 days.
	if _, err := tx.Exec(ctx, `DELETE FROM idempotency_records
		WHERE operation = $1 AND principal_id = $2 AND request_key = $3 AND expire_time <= now()`, operation, principalID, requestKey); err != nil {
		return IdempotencyRecord{}, false, err
	}
	tag, err := tx.Exec(ctx, `INSERT INTO idempotency_records (operation, principal_id, request_key, fingerprint, result_id, expire_time)
		VALUES ($1, $2, $3, $4, $5, now() + $6::interval) ON CONFLICT DO NOTHING`,
		operation, principalID, requestKey, fingerprint, resultID, IdempotencyTTL.String())
	if err != nil {
		return IdempotencyRecord{}, false, err
	}
	if tag.RowsAffected() == 1 {
		return IdempotencyRecord{Fingerprint: fingerprint, ResultID: resultID}, true, nil
	}
	var rec IdempotencyRecord
	err = tx.QueryRow(ctx, `SELECT fingerprint, result_id FROM idempotency_records
		WHERE operation = $1 AND principal_id = $2 AND request_key = $3`, operation, principalID, requestKey).Scan(&rec.Fingerprint, &rec.ResultID)
	if errors.Is(err, pgx.ErrNoRows) {
		return IdempotencyRecord{}, false, errors.New("store: idempotency record vanished; retry")
	}
	return rec, false, err
}

// DeleteExpiredIdempotency removes expired request keys.
func DeleteExpiredIdempotency(ctx context.Context, db DB) (int64, error) {
	tag, err := db.Exec(ctx, `DELETE FROM idempotency_records WHERE expire_time <= now()`)
	return tag.RowsAffected(), err
}
