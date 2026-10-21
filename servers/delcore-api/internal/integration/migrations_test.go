// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/migrations"
)

// migratedTo returns a store in a fresh schema with the first version
// migrations applied exactly as Store.Migrate records them, so a test can
// write rows that an older schema held before upgrading.
func migratedTo(t *testing.T, version int) *postgres.Store {
	t.Helper()
	dsn := os.Getenv("TEST_DATABASE_URL")
	if dsn == "" {
		t.Fatal("TEST_DATABASE_URL is required for integration tests")
	}
	ctx := context.Background()
	root, err := postgres.Open(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	schema := "test_" + strings.ReplaceAll(key(), "-", "")
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
	db, err := postgres.Open(ctx, u.String())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		db.Pool.Close()
		root.Pool.Exec(context.Background(), "DROP SCHEMA "+schema+" CASCADE")
		root.Pool.Close()
	})
	if _, err = db.Pool.Exec(ctx, "CREATE TABLE delcore_schema(version integer PRIMARY KEY, digest text NOT NULL, applied_at timestamptz NOT NULL)"); err != nil {
		t.Fatal(err)
	}
	names, err := migrations.Files.ReadDir(".")
	if err != nil {
		t.Fatal(err)
	}
	number := 0
	for _, name := range names {
		if name.IsDir() || number == version {
			continue
		}
		number++
		data, err := migrations.Files.ReadFile(name.Name())
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(data)
		if _, err = db.Pool.Exec(ctx, string(data)); err != nil {
			t.Fatalf("migration %d: %v", number, err)
		}
		if _, err = db.Pool.Exec(ctx, "INSERT INTO delcore_schema VALUES($1,$2,now())", number, hex.EncodeToString(sum[:])); err != nil {
			t.Fatal(err)
		}
	}
	if number != version {
		t.Fatalf("applied %d migrations, want %d", number, version)
	}
	return db
}

// Migration 9 backfills the failure time only for metering jobs whose last
// write recorded the failure. A job that RetryJob returned to pending work
// before the upgrade keeps its category, but its updated_at is the retry
// request time, so it must not become a failure time.
func TestMigrationNineBackfillsOnlyRecordedFailureTimes(t *testing.T) {
	db := migratedTo(t, 8)
	ctx := context.Background()
	failedAt := time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC)
	retriedAt := failedAt.Add(time.Hour)
	failed, retried, transient := key(), key(), key()
	for _, row := range []struct {
		id, state string
		attempts  int
		updated   time.Time
	}{
		// The worker recorded a final failure.
		{failed, "failed", 3, failedAt},
		// The worker recorded a failure and an operator retried the job.
		{retried, "pending", 0, retriedAt},
		// The worker recorded a transient failure and will retry it.
		{transient, "pending", 1, failedAt},
	} {
		if _, err := db.Pool.Exec(ctx, `INSERT INTO jobs(id,kind,dedup_key,payload,state,attempts,available_at,error_category,created_at,updated_at)
 VALUES($1::uuid,'metering',$1::text,'{}',$2,$3,$4,'metering_transient',$4,$4)`, row.id, row.state, row.attempts, row.updated); err != nil {
			t.Fatal(err)
		}
	}
	if err := db.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	if err := db.CheckSchema(ctx); err != nil {
		t.Fatal(err)
	}
	for id, want := range map[string]*time.Time{failed: &failedAt, retried: nil, transient: &failedAt} {
		var got *time.Time
		if err := db.Pool.QueryRow(ctx, "SELECT failed_at FROM jobs WHERE id=$1", id).Scan(&got); err != nil {
			t.Fatal(err)
		}
		if (got == nil) != (want == nil) || (got != nil && !got.Equal(*want)) {
			t.Fatalf("job %s failed_at = %v, want %v", id, got, want)
		}
	}
}
