// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"os"
	"testing"

	"github.com/delinoio/apps/servers/devbase-api/internal/store"
)

func TestMigrationsAreRepeatableAndGuarded(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	if e := f.db.CheckSchema(ctx); e != nil {
		t.Fatal(e)
	}
	if e := f.db.Migrate(ctx); e != nil {
		t.Fatal("repeated migration failed:", e)
	}
	// A modified applied migration is rejected by both migrate and the startup check.
	if _, e := f.db.Pool.Exec(ctx, "UPDATE devbase_schema SET digest='tampered' WHERE version=1"); e != nil {
		t.Fatal(e)
	}
	if e := f.db.CheckSchema(ctx); e == nil {
		t.Fatal("tampered schema accepted")
	}
	if e := f.db.Migrate(ctx); e == nil {
		t.Fatal("migration ran over a checksum mismatch")
	}
	// A newer database than this binary is incompatible.
	if _, e := f.db.Pool.Exec(ctx, "UPDATE devbase_schema SET digest=(SELECT digest FROM devbase_schema WHERE version=1) WHERE version=1; INSERT INTO devbase_schema VALUES(3,'future',now())"); e != nil {
		t.Fatal(e)
	}
	if e := f.db.CheckSchema(ctx); e == nil {
		t.Fatal("newer schema accepted")
	}
	if e := f.db.Migrate(ctx); e == nil {
		t.Fatal("older binary migrated a newer schema")
	}
}

func TestMissingSchemaIsReportedNotReady(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	if _, e := f.db.Pool.Exec(ctx, "DROP SCHEMA public CASCADE; CREATE SCHEMA public"); e != nil {
		t.Fatal(e)
	}
	if e := f.db.CheckSchema(ctx); e == nil {
		t.Fatal("empty database reported compatible")
	}
}

func TestOnlyOneInstanceMayServe(t *testing.T) {
	f := setup(t)
	ctx := context.Background()
	first, e := f.db.AcquireInstance(ctx)
	if e != nil {
		t.Fatal(e)
	}
	second, e := store.Open(ctx, os.Getenv("TEST_DATABASE_URL"))
	if e != nil {
		t.Fatal(e)
	}
	defer second.Close()
	if _, e = second.AcquireInstance(ctx); e == nil {
		t.Fatal("second instance acquired the lock")
	}
	first.Conn().Close(ctx)
	first.Release()
}
