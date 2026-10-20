// SPDX-License-Identifier: FSL-1.1-ALv2

// Package store owns the PostgreSQL pool, explicit migrations and schema compatibility checks.
package store

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"time"

	"github.com/delinoio/apps/servers/devbase-api/migrations"
	"github.com/jackc/pgx/v5/pgxpool"
)

const (
	migrationLock = 41830001
	instanceLock  = 41830002
)

type Store struct{ Pool *pgxpool.Pool }

func Open(ctx context.Context, url string) (*Store, error) {
	p, e := pgxpool.New(ctx, url)
	if e != nil {
		return nil, errors.New("database configuration failed")
	}
	if e = p.Ping(ctx); e != nil {
		p.Close()
		return nil, errors.New("database unavailable")
	}
	return &Store{p}, nil
}

func (s *Store) Close() { s.Pool.Close() }

// CheckSchema fails unless the recorded migrations match this binary's embedded
// migrations in version and digest.
func (s *Store) CheckSchema(ctx context.Context) error {
	files, e := inventory()
	if e != nil {
		return e
	}
	rows, e := s.Pool.Query(ctx, "SELECT version,digest FROM devbase_schema ORDER BY version")
	if e != nil {
		return fmt.Errorf("schema unavailable: run migrate: %w", e)
	}
	defer rows.Close()
	applied := 0
	for rows.Next() {
		var version int
		var digest string
		if e = rows.Scan(&version, &digest); e != nil {
			return e
		}
		if version < 1 || version > len(files) {
			return fmt.Errorf("incompatible schema version %d; expected %d", version, migrations.Version)
		}
		if files[version-1].digest != digest {
			return errors.New("migration checksum mismatch")
		}
		applied++
	}
	if e = rows.Err(); e != nil {
		return e
	}
	if applied != migrations.Version {
		return fmt.Errorf("incompatible schema version %d; expected %d", applied, migrations.Version)
	}
	return nil
}

type migration struct {
	sql    string
	digest string
}

func inventory() ([]migration, error) {
	names, e := migrations.Files.ReadDir(".")
	if e != nil {
		return nil, e
	}
	var out []migration
	for _, name := range names {
		if name.IsDir() {
			continue
		}
		data, e := migrations.Files.ReadFile(name.Name())
		if e != nil {
			return nil, e
		}
		sum := sha256.Sum256(data)
		out = append(out, migration{string(data), hex.EncodeToString(sum[:])})
	}
	if len(out) != migrations.Version {
		return nil, errors.New("migration inventory mismatch")
	}
	return out, nil
}

// Migrate applies missing migrations in order under an advisory lock. It is safe to repeat.
func (s *Store) Migrate(ctx context.Context) error {
	files, e := inventory()
	if e != nil {
		return e
	}
	tx, e := s.Pool.Begin(ctx)
	if e != nil {
		return e
	}
	defer tx.Rollback(ctx)
	if _, e = tx.Exec(ctx, "SELECT pg_advisory_xact_lock($1)", migrationLock); e != nil {
		return e
	}
	if _, e = tx.Exec(ctx, "CREATE TABLE IF NOT EXISTS devbase_schema(version integer PRIMARY KEY, digest text NOT NULL, applied_at timestamptz NOT NULL)"); e != nil {
		return e
	}
	var version int
	if e = tx.QueryRow(ctx, "SELECT COALESCE(max(version),0) FROM devbase_schema").Scan(&version); e != nil {
		return e
	}
	if version > len(files) {
		return errors.New("database schema is newer than this binary")
	}
	for i, m := range files {
		number := i + 1
		if number <= version {
			var old string
			if e = tx.QueryRow(ctx, "SELECT digest FROM devbase_schema WHERE version=$1", number).Scan(&old); e != nil {
				return e
			}
			if old != m.digest {
				return errors.New("migration checksum mismatch")
			}
			continue
		}
		if _, e = tx.Exec(ctx, m.sql); e != nil {
			return fmt.Errorf("migration %d failed: %w", number, e)
		}
		if _, e = tx.Exec(ctx, "INSERT INTO devbase_schema VALUES($1,$2,$3)", number, m.digest, time.Now().UTC()); e != nil {
			return e
		}
	}
	return tx.Commit(ctx)
}

// AcquireInstance holds a dedicated session lock for the process lifetime so a
// second server cannot run against the same database. The caller must keep the
// connection open and release it on shutdown.
func (s *Store) AcquireInstance(ctx context.Context) (*pgxpool.Conn, error) {
	c, e := s.Pool.Acquire(ctx)
	if e != nil {
		return nil, e
	}
	var ok bool
	if e = c.QueryRow(ctx, "SELECT pg_try_advisory_lock($1)", instanceLock).Scan(&ok); e != nil || !ok {
		c.Release()
		return nil, errors.New("devbase instance lock unavailable")
	}
	return c, nil
}
