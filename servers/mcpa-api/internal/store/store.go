// SPDX-License-Identifier: FSL-1.1-ALv2

// Package store owns PostgreSQL access, migrations and schema checks.
package store

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"regexp"
	"sort"
	"strconv"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/delinoio/apps/servers/mcpa-api/migrations"
)

// ErrNotFound means the requested row does not exist or is not visible.
var ErrNotFound = errors.New("store: not found")

// DB is satisfied by *pgxpool.Pool and pgx.Tx.
type DB interface {
	Exec(ctx context.Context, sql string, args ...any) (pgconn.CommandTag, error)
	Query(ctx context.Context, sql string, args ...any) (pgx.Rows, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

// IsUniqueViolation reports whether err violates the named unique index or
// constraint.
func IsUniqueViolation(err error, constraint string) bool {
	var pgErr *pgconn.PgError
	return errors.As(err, &pgErr) && pgErr.Code == "23505" && pgErr.ConstraintName == constraint
}

// ResourceNameIndex enforces unique live resource names per tenant.
const ResourceNameIndex = "resources_tenant_name_idx"

// Store wraps the connection pool.
type Store struct {
	Pool *pgxpool.Pool
}

// Open connects to PostgreSQL and verifies connectivity.
func Open(ctx context.Context, databaseURL string, timeout time.Duration) (*Store, error) {
	cfg, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		// The URL can contain a password; never include it in the error.
		return nil, errors.New("store: DATABASE_URL is not a valid PostgreSQL URL")
	}
	cfg.ConnConfig.ConnectTimeout = timeout
	pool, err := pgxpool.NewWithConfig(ctx, cfg)
	if err != nil {
		return nil, fmt.Errorf("store: connect: %w", err)
	}
	pingCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	if err := pool.Ping(pingCtx); err != nil {
		pool.Close()
		return nil, fmt.Errorf("store: ping: %w", err)
	}
	return &Store{Pool: pool}, nil
}

// Close releases the pool.
func (s *Store) Close() { s.Pool.Close() }

// InTx runs fn in a read-committed transaction.
func (s *Store) InTx(ctx context.Context, fn func(tx pgx.Tx) error) error {
	return pgx.BeginFunc(ctx, s.Pool, fn)
}

// NewID returns a UUID v7.
func NewID() uuid.UUID {
	id, err := uuid.NewV7()
	if err != nil {
		// NewV7 only fails when the random source fails; that is fatal.
		panic(err)
	}
	return id
}

// Migration is one embedded SQL migration.
type Migration struct {
	Version int
	Name    string
	SQL     string
}

var migrationName = regexp.MustCompile(`^(\d{4})_([a-z0-9_]+)\.sql$`)

// Migrations returns the embedded migrations in order. Numbers must be
// contiguous from 1.
func Migrations() ([]Migration, error) {
	return loadMigrations(migrations.FS)
}

func loadMigrations(fsys fs.FS) ([]Migration, error) {
	entries, err := fs.ReadDir(fsys, ".")
	if err != nil {
		return nil, err
	}
	var out []Migration
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		m := migrationName.FindStringSubmatch(e.Name())
		if m == nil {
			if len(e.Name()) > 4 && e.Name()[len(e.Name())-4:] == ".sql" {
				return nil, fmt.Errorf("store: migration %q must be named NNNN_name.sql", e.Name())
			}
			continue
		}
		v, _ := strconv.Atoi(m[1])
		body, err := fs.ReadFile(fsys, e.Name())
		if err != nil {
			return nil, err
		}
		out = append(out, Migration{Version: v, Name: m[2], SQL: string(body)})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Version < out[j].Version })
	want := 1
	for _, m := range out {
		if m.Version > want {
			return nil, fmt.Errorf("store: migration numbers must be contiguous from 0001; %04d is missing before %04d", want, m.Version)
		}
		if m.Version != want {
			return nil, fmt.Errorf("store: migration %04d is duplicated", m.Version)
		}
		want++
	}
	if len(out) == 0 {
		return nil, errors.New("store: no migrations embedded")
	}
	return out, nil
}

// ExpectedVersion is the newest embedded migration number.
func ExpectedVersion() (int, error) {
	ms, err := Migrations()
	if err != nil {
		return 0, err
	}
	return ms[len(ms)-1].Version, nil
}

const migrationsTable = "mcpa_schema_migrations"

// migrateLockKey serializes concurrent `migrate` runs.
const migrateLockKey int64 = 0x6d637061_6d696772 // "mcpamigr"

// Migrate applies pending migrations, each in its own transaction, under a
// session advisory lock. It returns the versions it applied.
func (s *Store) Migrate(ctx context.Context) ([]int, error) {
	ms, err := Migrations()
	if err != nil {
		return nil, err
	}
	conn, err := s.Pool.Acquire(ctx)
	if err != nil {
		return nil, err
	}
	defer conn.Release()
	if _, err := conn.Exec(ctx, "SELECT pg_advisory_lock($1)", migrateLockKey); err != nil {
		return nil, fmt.Errorf("store: migration lock: %w", err)
	}
	defer func() {
		// Use a fresh context so the lock is released even after cancellation.
		_, _ = conn.Exec(context.Background(), "SELECT pg_advisory_unlock($1)", migrateLockKey)
	}()
	if _, err := conn.Exec(ctx, `CREATE TABLE IF NOT EXISTS `+migrationsTable+` (
		version integer PRIMARY KEY,
		name text NOT NULL,
		applied_at timestamptz NOT NULL DEFAULT now()
	)`); err != nil {
		return nil, fmt.Errorf("store: create migrations table: %w", err)
	}
	applied, err := appliedMigrations(ctx, conn)
	if err != nil {
		return nil, err
	}
	if err := compareApplied(ms, applied, false); err != nil {
		return nil, err
	}
	var done []int
	for _, m := range ms {
		if _, ok := applied[m.Version]; ok {
			continue
		}
		err := pgx.BeginFunc(ctx, conn, func(tx pgx.Tx) error {
			if _, err := tx.Exec(ctx, m.SQL); err != nil {
				return err
			}
			_, err := tx.Exec(ctx, `INSERT INTO `+migrationsTable+` (version, name) VALUES ($1, $2)`, m.Version, m.Name)
			return err
		})
		if err != nil {
			return done, fmt.Errorf("store: migration %04d_%s: %w", m.Version, m.Name, err)
		}
		done = append(done, m.Version)
	}
	return done, nil
}

func appliedMigrations(ctx context.Context, db DB) (map[int]string, error) {
	rows, err := db.Query(ctx, `SELECT version, name FROM `+migrationsTable)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := map[int]string{}
	for rows.Next() {
		var v int
		var name string
		if err := rows.Scan(&v, &name); err != nil {
			return nil, err
		}
		out[v] = name
	}
	return out, rows.Err()
}

// SchemaError describes a schema that does not match the binary.
type SchemaError struct{ Reason string }

func (e *SchemaError) Error() string { return "schema mismatch: " + e.Reason }

func compareApplied(ms []Migration, applied map[int]string, requireAll bool) error {
	known := map[int]string{}
	for _, m := range ms {
		known[m.Version] = m.Name
	}
	for v, name := range applied {
		want, ok := known[v]
		if !ok {
			return &SchemaError{Reason: fmt.Sprintf("database has migration %04d that this binary does not know; deploy a newer binary", v)}
		}
		if want != name {
			return &SchemaError{Reason: fmt.Sprintf("migration %04d is %q in the database but %q in this binary", v, name, want)}
		}
	}
	if requireAll {
		for _, m := range ms {
			if _, ok := applied[m.Version]; !ok {
				return &SchemaError{Reason: fmt.Sprintf("migration %04d_%s is not applied; run mcpa-api migrate", m.Version, m.Name)}
			}
		}
	}
	return nil
}

// CheckSchema returns nil only when every embedded migration, and no other,
// is applied.
func (s *Store) CheckSchema(ctx context.Context) error {
	ms, err := Migrations()
	if err != nil {
		return err
	}
	var exists bool
	if err := s.Pool.QueryRow(ctx, `SELECT to_regclass($1) IS NOT NULL`, migrationsTable).Scan(&exists); err != nil {
		return err
	}
	if !exists {
		return &SchemaError{Reason: "schema is not initialized; run mcpa-api migrate"}
	}
	applied, err := appliedMigrations(ctx, s.Pool)
	if err != nil {
		return err
	}
	return compareApplied(ms, applied, true)
}
