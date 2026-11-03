// SPDX-License-Identifier: FSL-1.1-ALv2

// Package storetest opens migrated PostgreSQL stores for integration tests.
package storetest

import (
	"context"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"

	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

// Open connects to the disposable TEST_DATABASE_URL database, creates a
// schema private to the test, applies the migrations in it and drops it at
// cleanup. Tests that share one database therefore do not interfere. It
// skips the test when TEST_DATABASE_URL is not set.
func Open(t testing.TB) *store.Store {
	t.Helper()
	raw := os.Getenv("TEST_DATABASE_URL")
	if raw == "" {
		t.Skip("TEST_DATABASE_URL is not set; skipping PostgreSQL integration tests")
	}
	ctx := context.Background()
	schema := "it_" + strings.ReplaceAll(store.NewID().String(), "-", "")
	admin, err := store.Open(ctx, raw, 10*time.Second, store.ModeMaintenance)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := admin.Pool.Exec(ctx, "CREATE SCHEMA "+pgx.Identifier{schema}.Sanitize()); err != nil {
		admin.Close()
		t.Fatal(err)
	}
	t.Cleanup(func() {
		defer admin.Close()
		if _, err := admin.Pool.Exec(context.Background(), "DROP SCHEMA "+pgx.Identifier{schema}.Sanitize()+" CASCADE"); err != nil {
			t.Errorf("drop test schema: %v", err)
		}
	})
	u, err := url.Parse(raw)
	if err != nil {
		t.Fatal("TEST_DATABASE_URL is not a valid URL")
	}
	q := u.Query()
	q.Set("search_path", schema)
	u.RawQuery = q.Encode()
	s, err := store.Open(ctx, u.String(), 10*time.Second, store.ModeServe)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(s.Close)
	if _, err := s.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	return s
}
