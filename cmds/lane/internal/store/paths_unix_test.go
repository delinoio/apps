//go:build !windows

// SPDX-License-Identifier: Apache-2.0
package store

import (
	"os"
	"path/filepath"
	"testing"
)

func TestPlatformRootsAndPermissions(t *testing.T) {
	for _, tc := range []struct{ platform, home, xdg, want string }{{"darwin", "/Users/example", "/ignored", "/Users/example/Library/Application Support/io.delino.lane"}, {"linux", "/home/example", "", "/home/example/.local/share/io.delino.lane"}, {"linux", "/home/example", "/data", "/data/io.delino.lane"}} {
		got, e := dataRoot(tc.platform, tc.home, tc.xdg)
		if e != nil || got != tc.want {
			t.Fatalf("%s: %q %v", tc.platform, got, e)
		}
	}
	if _, e := dataRoot("linux", "/home/u", "relative"); e == nil {
		t.Fatal("relative XDG root accepted")
	}
	s := testStore(t)
	for _, p := range []string{s.Root(), filepath.Join(s.Root(), "spaces", s.SpaceID())} {
		st, e := os.Stat(p)
		if e != nil || st.Mode().Perm() != 0700 {
			t.Fatal("directory permissions", e)
		}
	}
	st, e := os.Stat(filepath.Join(s.Root(), "registry.sqlite"))
	if e != nil || st.Mode().Perm() != 0600 {
		t.Fatal("database permissions", e)
	}
}
