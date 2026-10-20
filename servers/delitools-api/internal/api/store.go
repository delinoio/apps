// SPDX-License-Identifier: FSL-1.1-ALv2
package api

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	pb "github.com/delinoio/apps/protos/delitools/v1"
	"github.com/delinoio/apps/servers/delitools-api/migrations"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"google.golang.org/protobuf/encoding/protojson"
	"time"
)

type Store struct{ Pool *pgxpool.Pool }

func Open(ctx context.Context, dsn string) (*Store, error) {
	p, e := pgxpool.New(ctx, dsn)
	if e != nil {
		return nil, e
	}
	if e = p.Ping(ctx); e != nil {
		p.Close()
		return nil, e
	}
	return &Store{p}, nil
}
func (s *Store) Migrate(ctx context.Context) error {
	tx, e := s.Pool.Begin(ctx)
	if e != nil {
		return e
	}
	defer tx.Rollback(ctx)
	if _, e = tx.Exec(ctx, "SELECT pg_advisory_xact_lock(41824701)"); e != nil {
		return e
	}
	if _, e = tx.Exec(ctx, "CREATE TABLE IF NOT EXISTS delitools_schema(version integer PRIMARY KEY,digest text NOT NULL)"); e != nil {
		return e
	}
	var version int
	if e = tx.QueryRow(ctx, "SELECT COALESCE(max(version),0) FROM delitools_schema").Scan(&version); e != nil {
		return e
	}
	if version > migrations.Version {
		return errors.New("schema is newer than binary")
	}
	for i, name := range []string{"001_initial.sql", "002_catalog_history.sql", "003_retention_indexes.sql", "004_request_failure.sql", "005_request_attempt_index.sql"} {
		revision := i + 1
		data, err := migrations.Files.ReadFile(name)
		if err != nil {
			return err
		}
		sum := sha256.Sum256(data)
		digest := hex.EncodeToString(sum[:])
		if revision <= version {
			var old string
			if e = tx.QueryRow(ctx, "SELECT digest FROM delitools_schema WHERE version=$1", revision).Scan(&old); e != nil {
				return e
			}
			if old != digest {
				return errors.New("migration checksum mismatch")
			}
		} else {
			if _, e = tx.Exec(ctx, string(data)); e != nil {
				return e
			}
			if _, e = tx.Exec(ctx, "INSERT INTO delitools_schema VALUES($1,$2)", revision, digest); e != nil {
				return e
			}
		}
	}
	return tx.Commit(ctx)
}
func (s *Store) CheckSchema(ctx context.Context) error {
	var v int
	if e := s.Pool.QueryRow(ctx, "SELECT max(version) FROM delitools_schema").Scan(&v); e != nil {
		return e
	}
	if v != migrations.Version {
		return fmt.Errorf("incompatible schema %d", v)
	}
	return nil
}
func (s *Store) AcquireInstance(ctx context.Context) (*pgxpool.Conn, error) {
	c, e := s.Pool.Acquire(ctx)
	if e != nil {
		return nil, e
	}
	var ok bool
	if e = c.QueryRow(ctx, "SELECT pg_try_advisory_lock(41824702)").Scan(&ok); e != nil || !ok {
		c.Release()
		return nil, errors.New("instance lock unavailable")
	}
	return c, nil
}
func (s *Store) Settings(ctx context.Context) (*pb.Settings, error) {
	var raw []byte
	if e := s.Pool.QueryRow(ctx, "SELECT value FROM settings WHERE singleton").Scan(&raw); e != nil {
		return nil, e
	}
	v := &pb.Settings{}
	if e := protojson.Unmarshal(raw, v); e != nil {
		return nil, e
	}
	return v, ValidateSettings(v)
}
func ValidateSettings(s *pb.Settings) error {
	if s == nil || s.MaxInputBytes == 0 || s.MaxTranslationItems == 0 || s.ExecutionTimeoutSeconds == 0 || s.TenantRequestsPerMinute == 0 || s.TenantConcurrency == 0 || s.MaxAiRetries == nil || s.PriceSyncSeconds == 0 || s.DeliveryRetrySeconds == 0 || s.MaxBackoffSeconds < s.DeliveryRetrySeconds || s.AlertBacklog == 0 || s.AlertExpirySeconds == 0 || time.Duration(s.AlertExpirySeconds)*time.Second >= Retention {
		return errors.New("missing or invalid operator settings")
	}
	seen := map[string]bool{}
	for _, m := range s.TranslationModels {
		if len(m) == 0 || len(m) > 200 || seen[m] {
			return errors.New("invalid model allowlist")
		}
		seen[m] = true
	}
	return nil
}
func (s *Store) BootstrapSettings(ctx context.Context, v *pb.Settings) error {
	if e := ValidateSettings(v); e != nil {
		return e
	}
	b, _ := protojson.Marshal(v)
	_, e := s.Pool.Exec(ctx, "INSERT INTO settings VALUES(true,$1) ON CONFLICT DO NOTHING", b)
	return e
}
func (s *Store) Recover(ctx context.Context) error {
	return begin(ctx, s, func(tx pgx.Tx) error {
		// No provider execution is possible before a durable attempt exists.
		if _, e := tx.Exec(ctx, "DELETE FROM requests WHERE state=$1 AND NOT EXISTS(SELECT 1 FROM attempts WHERE request_id=requests.id)", pb.RequestState_REQUEST_STATE_PROCESSING); e != nil {
			return e
		}
		_, e := tx.Exec(ctx, "UPDATE requests SET state=$1 WHERE state=$2", pb.RequestState_REQUEST_STATE_UNKNOWN, pb.RequestState_REQUEST_STATE_PROCESSING)
		return e
	})
}
func (s *Store) Expire(ctx context.Context, now time.Time) error {
	tx, e := s.Pool.Begin(ctx)
	if e != nil {
		return e
	}
	defer tx.Rollback(ctx)
	for _, table := range []string{"requests", "attempts", "audit", "operational_logs"} {
		if _, e = tx.Exec(ctx, "DELETE FROM "+table+" WHERE created_at<=$1", now.Add(-Retention)); e != nil {
			return e
		}
	}
	return tx.Commit(ctx)
}
func (s *Store) Log(ctx context.Context, category, id string, now time.Time) {
	_, _ = s.Pool.Exec(ctx, "INSERT INTO operational_logs VALUES($1,$2,$3,$4)", ID(), category, id, now)
}
func begin(ctx context.Context, s *Store, fn func(pgx.Tx) error) error {
	tx, e := s.Pool.Begin(ctx)
	if e != nil {
		return e
	}
	defer tx.Rollback(ctx)
	if e = fn(tx); e != nil {
		return e
	}
	return tx.Commit(ctx)
}
