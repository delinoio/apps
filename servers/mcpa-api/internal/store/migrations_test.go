// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"strings"
	"testing"
	"testing/fstest"
)

func migrationFS(names ...string) fstest.MapFS {
	fsys := fstest.MapFS{}
	for _, n := range names {
		fsys[n] = &fstest.MapFile{Data: []byte("SELECT 1;")}
	}
	return fsys
}

func TestLoadMigrationsRequiresContiguousNumbers(t *testing.T) {
	ms, err := loadMigrations(migrationFS("0001_core.sql", "0002_data_plane.sql", "0003_mcp_oauth.sql"))
	if err != nil || len(ms) != 3 || ms[1].Version != 2 || ms[2].Name != "mcp_oauth" {
		t.Fatalf("migrations = %+v, %v", ms, err)
	}
	if _, err := loadMigrations(migrationFS("0001_core.sql", "0003_mcp_oauth.sql")); err == nil || !strings.Contains(err.Error(), "0002 is missing") {
		t.Fatalf("gap: %v", err)
	}
	if _, err := loadMigrations(migrationFS("0002_x.sql")); err == nil {
		t.Fatal("missing 0001 accepted")
	}
}

func TestEmbeddedMigrations(t *testing.T) {
	ms, err := Migrations()
	if err != nil {
		t.Fatal(err)
	}
	v, err := ExpectedVersion()
	if err != nil {
		t.Fatal(err)
	}
	if v != 3 || ms[len(ms)-1].Name != "mcp_oauth" {
		t.Fatalf("expected version %d, last %+v", v, ms[len(ms)-1])
	}
}
