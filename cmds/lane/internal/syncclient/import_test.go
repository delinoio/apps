// SPDX-License-Identifier: Apache-2.0
package syncclient

import (
	"context"
	"database/sql"
	"path/filepath"
	"testing"
)

type namedStore struct {
	*testStore
	space string
}

func (s namedStore) SpaceID() string { return s.space }
func TestStandaloneImportOnceAcrossDeletionAndReconnect(t *testing.T) {
	src, dst := openTestStore(t), openTestStore(t)
	dst.root = src.root
	registry, e := sql.Open("sqlite", filepath.Join(src.root, "registry.sqlite"))
	if e != nil {
		t.Fatal(e)
	}
	defer registry.Close()
	if _, e = registry.Exec("CREATE TABLE spaces(id TEXT PRIMARY KEY,account_key TEXT); INSERT INTO spaces VALUES('local','local'),('account','account-key'),('replacement','account-key')"); e != nil {
		t.Fatal(e)
	}
	enqueue(t, src, "req", version("task", "v"))
	source, target := namedStore{src, "local"}, namedStore{dst, "account"}
	if e = ImportStandalone(context.Background(), source, target); e != nil {
		t.Fatal(e)
	}
	if count(t, dst, "SELECT count(*) FROM versions") != 1 || count(t, dst, "SELECT count(*) FROM outbox") != 1 {
		t.Fatal("import lost history/outbox")
	}
	if _, e = dst.db.Exec("DELETE FROM outbox; DELETE FROM heads; DELETE FROM versions; DELETE FROM entities"); e != nil {
		t.Fatal(e)
	}
	if e = ImportStandalone(context.Background(), source, target); e != nil {
		t.Fatal(e)
	}
	if count(t, dst, "SELECT count(*) FROM entities") != 0 {
		t.Fatal("reimport resurrected account data")
	}
	fresh := openTestStore(t)
	fresh.root = src.root
	if e = ImportStandalone(context.Background(), source, namedStore{fresh, "replacement"}); e != nil {
		t.Fatal(e)
	}
	if count(t, fresh, "SELECT count(*) FROM entities") != 0 {
		t.Fatal("new account space resurrected standalone content")
	}
	if e = ImportStandalone(context.Background(), target, namedStore{fresh, "replacement"}); e != ErrAccountMismatch {
		t.Fatalf("retained account import: %v", e)
	}
}
func TestFailedImportResumesSameDestination(t *testing.T) {
	src, dst := openTestStore(t), openTestStore(t)
	dst.root = src.root
	registry, e := sql.Open("sqlite", filepath.Join(src.root, "registry.sqlite"))
	if e != nil {
		t.Fatal(e)
	}
	defer registry.Close()
	if _, e = registry.Exec("CREATE TABLE spaces(id TEXT PRIMARY KEY,account_key TEXT); INSERT INTO spaces VALUES('local','local'),('account','account-key')"); e != nil {
		t.Fatal(e)
	}
	enqueue(t, src, "req", version("task", "v"))
	if _, e = dst.db.Exec("CREATE TRIGGER full_disk BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'storage full'); END"); e != nil {
		t.Fatal(e)
	}
	source, target := namedStore{src, "local"}, namedStore{dst, "account"}
	if e = ImportStandalone(context.Background(), source, target); e == nil {
		t.Fatal("expected write failure")
	}
	if count(t, dst, "SELECT count(*) FROM entities") != 0 {
		t.Fatal("partial import")
	}
	if _, e = dst.db.Exec("DROP TRIGGER full_disk"); e != nil {
		t.Fatal(e)
	}
	if e = ImportStandalone(context.Background(), source, target); e != nil {
		t.Fatal(e)
	}
	if count(t, dst, "SELECT count(*) FROM entities") != 1 {
		t.Fatal("failed import did not resume")
	}
}
