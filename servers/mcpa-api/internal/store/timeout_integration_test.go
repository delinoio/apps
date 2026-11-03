// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package store_test

import (
	"context"
	"errors"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"

	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

const testTimeout = 400 * time.Millisecond

// openTimeoutStore opens a store in the given mode on a private schema that
// holds one table, "locks", and returns it with a maintenance store on the
// same schema for setup and lock holding. extra is appended to the query.
func openTimeoutStore(t *testing.T, mode store.Mode, extra string) (s, admin *store.Store) {
	t.Helper()
	raw := os.Getenv("TEST_DATABASE_URL")
	if raw == "" {
		t.Skip("TEST_DATABASE_URL is not set; skipping PostgreSQL integration tests")
	}
	ctx := context.Background()
	schema := "to_" + strings.ReplaceAll(store.NewID().String(), "-", "")
	setup, err := store.Open(ctx, raw, 10*time.Second, store.ModeMaintenance)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := setup.Pool.Exec(ctx, "CREATE SCHEMA "+pgx.Identifier{schema}.Sanitize()); err != nil {
		setup.Close()
		t.Fatal(err)
	}
	t.Cleanup(func() {
		defer setup.Close()
		_, _ = setup.Pool.Exec(context.Background(), "DROP SCHEMA "+pgx.Identifier{schema}.Sanitize()+" CASCADE")
	})
	u, err := url.Parse(raw)
	if err != nil {
		t.Fatal("TEST_DATABASE_URL is not a valid URL")
	}
	q := u.Query()
	q.Set("search_path", schema)
	for _, kv := range strings.Split(extra, "&") {
		if k, v, ok := strings.Cut(kv, "="); ok {
			q.Set(k, v)
		}
	}
	u.RawQuery = q.Encode()
	admin, err = store.Open(ctx, u.String(), 10*time.Second, store.ModeMaintenance)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(admin.Close)
	if _, err := admin.Pool.Exec(ctx, `CREATE TABLE locks (id int PRIMARY KEY, n int NOT NULL DEFAULT 0); INSERT INTO locks (id) VALUES (1)`); err != nil {
		t.Fatal(err)
	}
	s, err = store.Open(ctx, u.String(), testTimeout, mode)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(s.Close)
	return s, admin
}

// holdRowLock locks the row of the locks table in a transaction of the admin
// store until the test ends.
func holdRowLock(t *testing.T, admin *store.Store) {
	t.Helper()
	ctx := context.Background()
	tx, err := admin.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = tx.Rollback(context.Background()) })
	if _, err := tx.Exec(ctx, `SELECT id FROM locks WHERE id = 1 FOR UPDATE`); err != nil {
		t.Fatal(err)
	}
}

// requireDependencyUnavailable checks that err is classified as a dependency
// outage and that the call ended near the timeout rather than waiting for
// the lock holder.
func requireDependencyUnavailable(t *testing.T, err error, elapsed time.Duration) {
	t.Helper()
	if err == nil {
		t.Fatal("call succeeded while the row was locked")
	}
	if ae := apperr.As(err); ae.Reason.String() != "ERROR_REASON_DEPENDENCY_UNAVAILABLE" {
		t.Fatalf("error %v classified as %s, want dependency unavailable", err, ae.Reason)
	}
	if elapsed < testTimeout/2 || elapsed > 4*testTimeout {
		t.Fatalf("call ended after %s, want about %s", elapsed, testTimeout)
	}
}

func TestIntegrationServeBoundsLockWaitOutsideTransaction(t *testing.T) {
	s, admin := openTimeoutStore(t, store.ModeServe, "")
	holdRowLock(t, admin)
	start := time.Now()
	_, err := s.Pool.Exec(context.Background(), `UPDATE locks SET n = 1 WHERE id = 1`)
	// The client deadline and the server's lock_timeout use the same bound,
	// so either may report first.
	requireDependencyUnavailable(t, err, time.Since(start))
}

func TestIntegrationServeBoundsLockWaitInTransaction(t *testing.T) {
	s, admin := openTimeoutStore(t, store.ModeServe, "")
	holdRowLock(t, admin)
	start := time.Now()
	err := s.InTx(context.Background(), func(tx pgx.Tx) error {
		var id int
		return tx.QueryRow(context.Background(), `SELECT id FROM locks WHERE id = 1 FOR UPDATE`).Scan(&id)
	})
	requireDependencyUnavailable(t, err, time.Since(start))
	// The statement context has no deadline here, so the server ended the
	// wait; statement_timeout and lock_timeout share the bound.
	var pgErr *pgconn.PgError
	if !errors.As(err, &pgErr) || (pgErr.Code != "55P03" && pgErr.Code != "57014") {
		t.Fatalf("error %v, want SQLSTATE 55P03 or 57014", err)
	}
}

func TestIntegrationServeBoundsStatementTime(t *testing.T) {
	s, _ := openTimeoutStore(t, store.ModeServe, "")
	start := time.Now()
	var one int
	err := s.Pool.QueryRow(context.Background(), `SELECT 1 FROM pg_sleep(5)`).Scan(&one)
	requireDependencyUnavailable(t, err, time.Since(start))

	// Inside a transaction only the server timeout applies.
	start = time.Now()
	err = s.InTx(context.Background(), func(tx pgx.Tx) error {
		_, err := tx.Exec(context.Background(), `SELECT pg_sleep(5)`)
		return err
	})
	requireDependencyUnavailable(t, err, time.Since(start))
}

func TestIntegrationServeBoundsPoolAcquisition(t *testing.T) {
	s, _ := openTimeoutStore(t, store.ModeServe, "pool_max_conns=1")
	ctx := context.Background()
	conn, err := s.Pool.Acquire(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Release()

	start := time.Now()
	_, err = s.Pool.Exec(ctx, `SELECT 1`)
	requireDependencyUnavailable(t, err, time.Since(start))
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("exec error %v, want context deadline exceeded", err)
	}
	start = time.Now()
	err = s.Pool.QueryRow(ctx, `SELECT 1`).Scan(new(int))
	requireDependencyUnavailable(t, err, time.Since(start))
	start = time.Now()
	_, err = s.Pool.Query(ctx, `SELECT 1`)
	requireDependencyUnavailable(t, err, time.Since(start))
	start = time.Now()
	err = s.InTx(ctx, func(pgx.Tx) error { return nil })
	requireDependencyUnavailable(t, err, time.Since(start))
	start = time.Now()
	_, err = s.Pool.Acquire(ctx)
	requireDependencyUnavailable(t, err, time.Since(start))

	// The pool recovers once the connection returns.
	conn.Release()
	if err := s.Pool.QueryRow(ctx, `SELECT 1`).Scan(new(int)); err != nil {
		t.Fatalf("query after release: %v", err)
	}
}

func TestIntegrationServeReleasesConnectionsOfClosedRows(t *testing.T) {
	s, _ := openTimeoutStore(t, store.ModeServe, "pool_max_conns=1")
	ctx := context.Background()
	for range 5 {
		rows, err := s.Pool.Query(ctx, `SELECT generate_series(1, 3)`)
		if err != nil {
			t.Fatal(err)
		}
		n := 0
		for rows.Next() {
			n++
		}
		rows.Close()
		if n != 3 || rows.Err() != nil {
			t.Fatalf("rows %d err %v", n, rows.Err())
		}
	}
}

func TestIntegrationMaintenanceKeepsLongStatements(t *testing.T) {
	s, _ := openTimeoutStore(t, store.ModeMaintenance, "")
	ctx := context.Background()
	start := time.Now()
	if _, err := s.Pool.Exec(ctx, `SELECT pg_sleep(1.2)`); err != nil {
		t.Fatalf("long statement in maintenance mode: %v", err)
	}
	if time.Since(start) < testTimeout {
		t.Fatalf("statement ended after %s, before the serve timeout", time.Since(start))
	}
	var timeout string
	if err := s.Pool.QueryRow(ctx, `SHOW statement_timeout`).Scan(&timeout); err != nil || timeout != "0" {
		t.Fatalf("statement_timeout = %q, err %v, want 0", timeout, err)
	}
}

func TestIntegrationServeSetsServerTimeouts(t *testing.T) {
	s, _ := openTimeoutStore(t, store.ModeServe, "")
	for _, name := range []string{"statement_timeout", "lock_timeout"} {
		var v string
		if err := s.Pool.QueryRow(context.Background(), "SHOW "+name).Scan(&v); err != nil || v != "400ms" {
			t.Fatalf("%s = %q, err %v, want 400ms", name, v, err)
		}
	}
}

func TestIntegrationDeleteTenantDataKeepsFourTimesTheTimeout(t *testing.T) {
	s, _ := openTimeoutStore(t, store.ModeServe, "")
	ctx := context.Background()
	if _, err := s.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	var inside, lockInside string
	_, err := s.DeleteTenantData(ctx, "tenant-without-data", func(tx pgx.Tx, _ []uuid.UUID) error {
		if err := tx.QueryRow(ctx, `SHOW statement_timeout`).Scan(&inside); err != nil {
			return err
		}
		return tx.QueryRow(ctx, `SHOW lock_timeout`).Scan(&lockInside)
	})
	if err != nil {
		t.Fatal(err)
	}
	if inside != "1600ms" || lockInside != "1600ms" {
		t.Fatalf("deletion timeouts %q and %q, want 4 times the dependency timeout", inside, lockInside)
	}
	// The override ends with the transaction.
	var after string
	if err := s.Pool.QueryRow(ctx, `SHOW statement_timeout`).Scan(&after); err != nil || after != "400ms" {
		t.Fatalf("statement_timeout afterwards = %q, err %v, want 400ms", after, err)
	}
}
