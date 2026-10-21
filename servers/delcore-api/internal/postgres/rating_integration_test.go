// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package postgres

import (
	"context"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

// migratedStore opens a fresh migrated schema in TEST_DATABASE_URL.
func migratedStore(t *testing.T) *Store {
	t.Helper()
	dsn := os.Getenv("TEST_DATABASE_URL")
	if dsn == "" {
		t.Fatal("TEST_DATABASE_URL is required for integration tests")
	}
	ctx := context.Background()
	root, err := Open(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	schema := "test_" + strings.ReplaceAll(uuid.Must(uuid.NewV7()).String(), "-", "")
	if _, err = root.Pool.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatal(err)
	}
	u, err := url.Parse(dsn)
	if err != nil {
		t.Fatal(err)
	}
	q := u.Query()
	q.Set("search_path", schema)
	u.RawQuery = q.Encode()
	db, err := Open(ctx, u.String())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		db.Pool.Close()
		root.Pool.Exec(context.Background(), "DROP SCHEMA "+schema+" CASCADE")
		root.Pool.Close()
	})
	if err = db.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	return db
}

func explain(t *testing.T, tx pgx.Tx, query string, args ...any) string {
	t.Helper()
	var plan string
	if err := tx.QueryRow(context.Background(), "EXPLAIN (FORMAT JSON) "+query, args...).Scan(&plan); err != nil {
		t.Fatal(err)
	}
	return plan
}

// Rating runs on every applied metering transition and paid authorization.
// The contribution scan must not read allowance_grants for each contribution,
// and the grant lookup must not scan every tenant's grants.
func TestRatingQueriesAvoidPerContributionGrantScans(t *testing.T) {
	db := migratedStore(t)
	ctx := context.Background()
	tx, err := db.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer tx.Rollback(ctx)
	tenant, meter, service := uuid.NewString(), uuid.NewString(), uuid.NewString()
	start := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)

	plan := explain(t, tx, contributionBucketsQuery, tenant, meter, start, []time.Time{start})
	if strings.Contains(plan, "allowance_grants") {
		t.Fatalf("contribution bucket plan reads allowance_grants per contribution: %s", plan)
	}
	if _, err = tx.Exec(ctx, "SET LOCAL enable_seqscan=off"); err != nil {
		t.Fatal(err)
	}
	plan = explain(t, tx, allowanceGrantsQuery, tenant, service, "units", start)
	if !strings.Contains(plan, "allowance_grants_period") {
		t.Fatalf("allowance grant lookup has no period index: %s", plan)
	}
}

// The bucket query derives the allowance epoch with width_bucket over the
// ordered grant times. It must equal the number of grants applied at or
// before the occurrence, counting equal and duplicate times.
func TestRatingAllowanceEpochCountsGrantsAtOrBeforeOccurrence(t *testing.T) {
	db := migratedStore(t)
	ctx := context.Background()
	base := time.Date(2026, 1, 1, 10, 0, 0, 0, time.UTC)
	grants := []time.Time{base, base, base.Add(time.Microsecond), base.Add(time.Hour)}
	for _, tc := range []struct {
		grants     []time.Time
		occurrence time.Time
		want       int
	}{
		{nil, base, 0},
		{grants, base.Add(-time.Microsecond), 0},
		{grants, base, 2},
		{grants, base.Add(time.Microsecond), 3},
		{grants, base.Add(time.Minute), 3},
		{grants, base.Add(time.Hour), 4},
		{grants, base.Add(48 * time.Hour), 4},
	} {
		list := tc.grants
		if list == nil {
			list = []time.Time{}
		}
		var got int
		if err := db.Pool.QueryRow(ctx, "SELECT width_bucket($1::timestamptz,$2::timestamptz[])", tc.occurrence, list).Scan(&got); err != nil {
			t.Fatal(err)
		}
		if got != tc.want {
			t.Fatalf("epoch of %s over %d grants = %d, want %d", tc.occurrence, len(tc.grants), got, tc.want)
		}
	}
}
