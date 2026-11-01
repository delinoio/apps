// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
)

// CallRecordRetention is how long call records are kept after the call start.
const CallRecordRetention = 30 * 24 * time.Hour

// MaxModelBytes bounds stored model names. Longer names are truncated.
const MaxModelBytes = 200

// MaxErrorClassBytes bounds the stored error class.
const MaxErrorClassBytes = 64

// Checkpoint is one metering checkpoint of a call. RequestBytes and
// ResponseBytes are the cumulative totals after the checkpoint; the deltas
// are the bytes not billed by earlier checkpoints.
type Checkpoint struct {
	CallID              uuid.UUID
	TenantID            string
	ResourceID          uuid.UUID
	CallKeyID           uuid.UUID
	CredentialID        uuid.UUID // uuid.Nil before a credential is selected
	Provider            mcpav1.Provider
	Model               string
	Format              mcpav1.EndpointFormat
	Streaming           bool
	StartTime           time.Time
	Seq                 int64
	DeltaRequestBytes   int64
	DeltaResponseBytes  int64
	RequestBytes        int64
	ResponseBytes       int64
	Final               bool
	Status              mcpav1.CallStatus
	ProviderStatusClass mcpav1.StatusClass
	ErrorClass          string
	LatencyMS           int64
	// OutboxID and RequestKey identify the outbox row inserted for a non-zero
	// delta. Both must be UUID v7 values.
	OutboxID   uuid.UUID
	RequestKey uuid.UUID
}

// EventKey returns the Delcore event key of a checkpoint.
func EventKey(callID uuid.UUID, seq int64) string {
	return fmt.Sprintf("mcpa:%s:%d", callID, seq)
}

// TruncateUTF8 makes s valid UTF-8 without NUL characters, which
// PostgreSQL text rejects, and shortens it to at most n bytes without
// splitting a rune.
func TruncateUTF8(s string, n int) string {
	s = strings.ReplaceAll(strings.ToValidUTF8(s, ""), "\x00", "")
	if len(s) <= n {
		return s
	}
	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}
	return s[:n]
}

// aggKey identifies a daily aggregate bucket.
type aggKey struct {
	model    string
	provider mcpav1.Provider
	status   mcpav1.CallStatus
}

// ApplyCheckpoint commits a checkpoint in one transaction: it upserts the
// call record, moves the call's bytes into the right daily aggregate bucket
// and inserts one outbox row when the delta is non-zero. A checkpoint whose
// sequence number is not newer than the committed one was already applied
// and is skipped, so a resent checkpoint bills its bytes exactly once. It
// reports whether the checkpoint was applied now.
func (s *Store) ApplyCheckpoint(ctx context.Context, c Checkpoint) (bool, error) {
	if c.Seq < 1 {
		return false, errors.New("store: checkpoint sequence must start at 1")
	}
	if c.DeltaRequestBytes < 0 || c.DeltaResponseBytes < 0 {
		return false, errors.New("store: checkpoint deltas must not be negative")
	}
	c.Model = TruncateUTF8(strings.TrimSpace(c.Model), MaxModelBytes)
	c.ErrorClass = TruncateUTF8(c.ErrorClass, MaxErrorClassBytes)
	var credentialID *uuid.UUID
	if c.CredentialID != uuid.Nil {
		credentialID = &c.CredentialID
	}
	status := mcpav1.CallStatus_CALL_STATUS_UNSPECIFIED
	if c.Final {
		status = c.Status
	}
	applied := false
	err := s.InTx(ctx, func(tx pgx.Tx) error {
		var prevSeq, prevReq, prevResp int64
		var prevModel string
		var prevProvider, prevStatus int16
		var prevFinal bool
		err := tx.QueryRow(ctx, `SELECT checkpoint_seq, request_bytes, response_bytes, model, provider, status, final
			FROM call_records WHERE id = $1 FOR UPDATE`, c.CallID).
			Scan(&prevSeq, &prevReq, &prevResp, &prevModel, &prevProvider, &prevStatus, &prevFinal)
		exists := true
		if errors.Is(err, pgx.ErrNoRows) {
			exists = false
		} else if err != nil {
			return err
		}
		if exists && (prevSeq >= c.Seq || prevFinal) {
			return nil
		}
		if exists && (prevReq+c.DeltaRequestBytes != c.RequestBytes || prevResp+c.DeltaResponseBytes != c.ResponseBytes) {
			// The engine resends a failed checkpoint unchanged before newer
			// bytes, so cumulative totals always continue the committed ones.
			return fmt.Errorf("store: checkpoint %d of call %s does not continue the committed totals", c.Seq, c.CallID)
		}
		if exists {
			_, err = tx.Exec(ctx, `UPDATE call_records SET credential_id = COALESCE($2, credential_id), provider = $3, model = $4,
				streaming = $5, status = $6, provider_status_class = $7, error_class = $8, request_bytes = $9, response_bytes = $10,
				latency_ms = $11, checkpoint_seq = $12, final = $13, update_time = now() WHERE id = $1`,
				c.CallID, credentialID, int16(c.Provider), c.Model, c.Streaming, int16(status), int16(c.ProviderStatusClass),
				c.ErrorClass, c.RequestBytes, c.ResponseBytes, c.LatencyMS, c.Seq, c.Final)
		} else {
			_, err = tx.Exec(ctx, `INSERT INTO call_records (id, tenant_id, resource_id, call_key_id, credential_id, provider, model,
					format, streaming, status, provider_status_class, error_class, request_bytes, response_bytes, latency_ms,
					checkpoint_seq, final, start_time, expire_time)
				VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)`,
				c.CallID, c.TenantID, c.ResourceID, c.CallKeyID, credentialID, int16(c.Provider), c.Model, int16(c.Format),
				c.Streaming, int16(status), int16(c.ProviderStatusClass), c.ErrorClass, c.RequestBytes, c.ResponseBytes,
				c.LatencyMS, c.Seq, c.Final, c.StartTime, c.StartTime.Add(CallRecordRetention))
		}
		if err != nil {
			return err
		}

		// Daily aggregates: bytes already counted for this call live in the
		// bucket of the previous checkpoint. When the bucket changes (the
		// provider became known, or the call finished), they move.
		day := c.StartTime.UTC().Truncate(24 * time.Hour)
		next := aggKey{model: c.Model, provider: c.Provider, status: status}
		callCount := int64(0)
		if c.Final {
			callCount = 1
		}
		if exists {
			prev := aggKey{model: prevModel, provider: mcpav1.Provider(prevProvider), status: mcpav1.CallStatus(prevStatus)}
			if prev != next && (prevReq > 0 || prevResp > 0) {
				if err := addUsage(ctx, tx, day, c, prev, 0, -prevReq, -prevResp); err != nil {
					return err
				}
				if err := addUsage(ctx, tx, day, c, next, callCount, c.RequestBytes, c.ResponseBytes); err != nil {
					return err
				}
			} else if err := addUsage(ctx, tx, day, c, next, callCount, c.DeltaRequestBytes, c.DeltaResponseBytes); err != nil {
				return err
			}
		} else if err := addUsage(ctx, tx, day, c, next, callCount, c.DeltaRequestBytes, c.DeltaResponseBytes); err != nil {
			return err
		}

		if delta := c.DeltaRequestBytes + c.DeltaResponseBytes; delta > 0 {
			if _, err := tx.Exec(ctx, `INSERT INTO usage_outbox (id, event_key, request_key, tenant_id, resource_id, occurred_at, bytes,
					next_attempt_at, create_time, expires_at)
				VALUES ($1, $2, $3, $4, $5, $6, $7, now(), now(), now() + interval '30 days')
				ON CONFLICT (event_key) DO NOTHING`,
				c.OutboxID, EventKey(c.CallID, c.Seq), c.RequestKey, c.TenantID, c.ResourceID,
				c.StartTime.UTC().Truncate(time.Microsecond), delta); err != nil {
				return err
			}
		}
		applied = true
		return nil
	})
	return applied, err
}

// addUsage adds signed amounts to one aggregate bucket and removes buckets
// that became empty.
func addUsage(ctx context.Context, tx pgx.Tx, day time.Time, c Checkpoint, k aggKey, calls, req, resp int64) error {
	if calls == 0 && req == 0 && resp == 0 {
		return nil
	}
	if req < 0 || resp < 0 {
		_, err := tx.Exec(ctx, `UPDATE usage_daily SET request_bytes = request_bytes + $8, response_bytes = response_bytes + $9,
				call_count = call_count + $10, update_time = now()
			WHERE day = $1 AND tenant_id = $2 AND resource_id = $3 AND model = $4 AND provider = $5 AND call_key_id = $6 AND status = $7`,
			day, c.TenantID, c.ResourceID, k.model, int16(k.provider), c.CallKeyID, int16(k.status), req, resp, calls)
		if err != nil {
			return err
		}
		_, err = tx.Exec(ctx, `DELETE FROM usage_daily
			WHERE day = $1 AND tenant_id = $2 AND resource_id = $3 AND model = $4 AND provider = $5 AND call_key_id = $6 AND status = $7
				AND call_count = 0 AND request_bytes = 0 AND response_bytes = 0`,
			day, c.TenantID, c.ResourceID, k.model, int16(k.provider), c.CallKeyID, int16(k.status))
		return err
	}
	_, err := tx.Exec(ctx, `INSERT INTO usage_daily (id, day, tenant_id, resource_id, model, provider, call_key_id, status,
			call_count, request_bytes, response_bytes)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
		ON CONFLICT ON CONSTRAINT usage_daily_key DO UPDATE SET call_count = usage_daily.call_count + EXCLUDED.call_count,
			request_bytes = usage_daily.request_bytes + EXCLUDED.request_bytes,
			response_bytes = usage_daily.response_bytes + EXCLUDED.response_bytes, update_time = now()`,
		NewID(), day, c.TenantID, c.ResourceID, k.model, int16(k.provider), c.CallKeyID, int16(k.status), calls, req, resp)
	return err
}

// UsageFilter selects call records and aggregates. ResourceIDs is required
// and already authorized; the other fields are optional.
type UsageFilter struct {
	TenantID    string
	ResourceIDs []uuid.UUID
	Models      []string
	Providers   []mcpav1.Provider
	CallKeyIDs  []uuid.UUID
	Statuses    []mcpav1.CallStatus
	// Start and End bound the call start time: Start inclusive, End exclusive.
	Start *time.Time
	End   *time.Time
}

func int16s[T ~int32](values []T) []int16 {
	out := make([]int16, 0, len(values))
	for _, v := range values {
		out = append(out, int16(v))
	}
	return out
}

// filterArgs returns the shared filter arguments in the order $1..$8 used by
// the usage queries.
func filterArgs(f UsageFilter) []any {
	models := f.Models
	if models == nil {
		models = []string{}
	}
	keys := f.CallKeyIDs
	if keys == nil {
		keys = []uuid.UUID{}
	}
	return []any{f.TenantID, f.ResourceIDs, models, int16s(f.Providers), keys, int16s(f.Statuses), f.Start, f.End}
}

// CallRecord is the stored metadata of one call.
type CallRecord struct {
	ID                  uuid.UUID
	TenantID            string
	ResourceID          uuid.UUID
	CallKeyID           uuid.UUID
	Provider            mcpav1.Provider
	Model               string
	Format              mcpav1.EndpointFormat
	Streaming           bool
	Status              mcpav1.CallStatus
	ProviderStatusClass mcpav1.StatusClass
	ErrorClass          string
	RequestBytes        int64
	ResponseBytes       int64
	LatencyMS           int64
	StartTime           time.Time
	Final               bool
}

// ListCallRecords returns call records newest first. before is the cursor
// (uuid.Nil for the first page).
func ListCallRecords(ctx context.Context, db DB, f UsageFilter, before uuid.UUID, limit int) ([]CallRecord, error) {
	args := append(filterArgs(f), before, limit)
	rows, err := db.Query(ctx, `SELECT id, tenant_id, resource_id, call_key_id, provider, model, format, streaming, status,
			provider_status_class, error_class, request_bytes, response_bytes, latency_ms, start_time, final
		FROM call_records
		WHERE tenant_id = $1 AND resource_id = ANY($2)
			AND (cardinality($3::text[]) = 0 OR model = ANY($3))
			AND (cardinality($4::smallint[]) = 0 OR provider = ANY($4))
			AND (cardinality($5::uuid[]) = 0 OR call_key_id = ANY($5))
			AND (cardinality($6::smallint[]) = 0 OR status = ANY($6))
			AND ($7::timestamptz IS NULL OR start_time >= $7)
			AND ($8::timestamptz IS NULL OR start_time < $8)
			AND ($9 = '00000000-0000-0000-0000-000000000000'::uuid OR id < $9)
		ORDER BY id DESC LIMIT $10`, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []CallRecord
	for rows.Next() {
		var r CallRecord
		var provider, format, status, class int16
		if err := rows.Scan(&r.ID, &r.TenantID, &r.ResourceID, &r.CallKeyID, &provider, &r.Model, &format, &r.Streaming, &status,
			&class, &r.ErrorClass, &r.RequestBytes, &r.ResponseBytes, &r.LatencyMS, &r.StartTime, &r.Final); err != nil {
			return nil, err
		}
		r.Provider = mcpav1.Provider(provider)
		r.Format = mcpav1.EndpointFormat(format)
		r.Status = mcpav1.CallStatus(status)
		r.ProviderStatusClass = mcpav1.StatusClass(class)
		out = append(out, r)
	}
	return out, rows.Err()
}

// UsageRow is one daily aggregate bucket.
type UsageRow struct {
	ID            uuid.UUID
	Day           time.Time
	ResourceID    uuid.UUID
	Model         string
	Provider      mcpav1.Provider
	CallKeyID     uuid.UUID
	Status        mcpav1.CallStatus
	CallCount     int64
	RequestBytes  int64
	ResponseBytes int64
}

// UsageTotals sums the aggregates that match a filter.
type UsageTotals struct {
	CallCount     int64
	RequestBytes  int64
	ResponseBytes int64
}

// usageWhere selects aggregate buckets. Aggregates are daily, so the time
// range selects every UTC day that overlaps [Start, End).
const usageWhere = `tenant_id = $1 AND resource_id = ANY($2)
	AND (cardinality($3::text[]) = 0 OR model = ANY($3))
	AND (cardinality($4::smallint[]) = 0 OR provider = ANY($4))
	AND (cardinality($5::uuid[]) = 0 OR call_key_id = ANY($5))
	AND (cardinality($6::smallint[]) = 0 OR status = ANY($6))
	AND ($7::timestamptz IS NULL OR day >= ($7::timestamptz AT TIME ZONE 'UTC')::date)
	AND ($8::timestamptz IS NULL OR day < (($8::timestamptz - interval '1 microsecond') AT TIME ZONE 'UTC')::date + 1)`

// SumUsage totals the aggregates of a filter.
func SumUsage(ctx context.Context, db DB, f UsageFilter) (UsageTotals, error) {
	var t UsageTotals
	err := db.QueryRow(ctx, `SELECT COALESCE(sum(call_count), 0)::bigint, COALESCE(sum(request_bytes), 0)::bigint,
			COALESCE(sum(response_bytes), 0)::bigint
		FROM usage_daily WHERE `+usageWhere, filterArgs(f)...).Scan(&t.CallCount, &t.RequestBytes, &t.ResponseBytes)
	return t, err
}

// ListUsageRows returns aggregate buckets, newest day first. after is the
// cursor (uuid.Nil for the first page).
func ListUsageRows(ctx context.Context, db DB, f UsageFilter, after uuid.UUID, limit int) ([]UsageRow, error) {
	args := append(filterArgs(f), after, limit)
	rows, err := db.Query(ctx, `SELECT id, day, resource_id, model, provider, call_key_id, status, call_count, request_bytes, response_bytes
		FROM usage_daily
		WHERE `+usageWhere+`
			AND ($9 = '00000000-0000-0000-0000-000000000000'::uuid
				OR (day, id) < (SELECT c.day, c.id FROM usage_daily c WHERE c.id = $9))
		ORDER BY day DESC, id DESC LIMIT $10`, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []UsageRow
	for rows.Next() {
		var r UsageRow
		var provider, status int16
		if err := rows.Scan(&r.ID, &r.Day, &r.ResourceID, &r.Model, &provider, &r.CallKeyID, &status, &r.CallCount,
			&r.RequestBytes, &r.ResponseBytes); err != nil {
			return nil, err
		}
		r.Provider = mcpav1.Provider(provider)
		r.Status = mcpav1.CallStatus(status)
		out = append(out, r)
	}
	return out, rows.Err()
}

// BilledBytes returns the bytes committed by checkpoints for calls that
// started in [start, end), for one resource (resourceID set) or a whole
// tenant. UTC days fully inside the period come from the daily aggregates;
// the partial first and last days come from call records, which are kept
// for 30 days.
func BilledBytes(ctx context.Context, db DB, tenantID string, resourceID *uuid.UUID, start, end time.Time) (int64, error) {
	if !start.Before(end) {
		return 0, nil
	}
	start, end = start.UTC(), end.UTC()
	firstFull := start.Truncate(24 * time.Hour)
	if !firstFull.Equal(start) {
		firstFull = firstFull.Add(24 * time.Hour)
	}
	lastFull := end.Truncate(24 * time.Hour) // exclusive
	var total int64
	scope := `($2::uuid IS NULL OR resource_id = $2)`
	if firstFull.Before(lastFull) {
		var full int64
		if err := db.QueryRow(ctx, `SELECT COALESCE(sum(request_bytes + response_bytes), 0)::bigint FROM usage_daily
			WHERE tenant_id = $1 AND `+scope+` AND day >= $3::date AND day < $4::date`,
			tenantID, resourceID, firstFull, lastFull).Scan(&full); err != nil {
			return 0, err
		}
		total += full
		// Partial days at both ends.
		for _, r := range [][2]time.Time{{start, firstFull}, {lastFull, end}} {
			n, err := recordBytes(ctx, db, tenantID, resourceID, r[0], r[1])
			if err != nil {
				return 0, err
			}
			total += n
		}
		return total, nil
	}
	return recordBytes(ctx, db, tenantID, resourceID, start, end)
}

func recordBytes(ctx context.Context, db DB, tenantID string, resourceID *uuid.UUID, start, end time.Time) (int64, error) {
	if !start.Before(end) {
		return 0, nil
	}
	var n int64
	err := db.QueryRow(ctx, `SELECT COALESCE(sum(request_bytes + response_bytes), 0)::bigint FROM call_records
		WHERE tenant_id = $1 AND ($2::uuid IS NULL OR resource_id = $2) AND start_time >= $3 AND start_time < $4`,
		tenantID, resourceID, start, end).Scan(&n)
	return n, err
}
