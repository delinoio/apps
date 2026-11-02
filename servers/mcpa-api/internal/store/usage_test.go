// SPDX-License-Identifier: FSL-1.1-ALv2

package store

import (
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
)

func TestTruncateUTF8(t *testing.T) {
	cases := []struct {
		in   string
		n    int
		want string
	}{
		{"gpt-4o", 200, "gpt-4o"},
		{"abcdef", 3, "abc"},
		{"ab\x00c", 10, "abc"},
		{"a\xffb", 10, "ab"},
		{"한국어", 4, "한"},
		{"한국어", 6, "한국"},
	}
	for _, tc := range cases {
		if got := TruncateUTF8(tc.in, tc.n); got != tc.want {
			t.Fatalf("TruncateUTF8(%q, %d) = %q, want %q", tc.in, tc.n, got, tc.want)
		}
	}
	long := strings.Repeat("é", 300)
	if got := TruncateUTF8(long, MaxModelBytes); len(got) > MaxModelBytes || !utf8.ValidString(got) {
		t.Fatalf("long model truncated to %d bytes", len(got))
	}
}

func TestEventKey(t *testing.T) {
	id := uuid.MustParse("019a0000-0000-7000-8000-000000000042")
	if got := EventKey(id, 3); got != "mcpa:019a0000-0000-7000-8000-000000000042:3" {
		t.Fatalf("event key %q", got)
	}
}

func TestSpendRetentionOutlivesBillingPeriods(t *testing.T) {
	// Delcore billing periods last at most one month (31 days); the partial
	// first day of a period must stay measurable until the period ends.
	if SpendRetention <= 31*24*time.Hour || SpendRetention != 35*24*time.Hour {
		t.Fatalf("spend retention %s", SpendRetention)
	}
}

func TestMigrationsAreContiguous(t *testing.T) {
	ms, err := Migrations()
	if err != nil {
		t.Fatal(err)
	}
	version, err := ExpectedVersion()
	if err != nil || version != len(ms) || version != 4 || ms[1].Name != "data_plane" || ms[2].Name != "usage_spend" || ms[3].Name != "mcp_oauth" {
		t.Fatalf("expected version %d with %d migrations: %v", version, len(ms), err)
	}
}
