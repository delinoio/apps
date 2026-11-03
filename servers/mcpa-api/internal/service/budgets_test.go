// SPDX-License-Identifier: FSL-1.1-ALv2

package service

import (
	"testing"
	"time"

	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
)

func TestNormalizeStoredLimitKeepsPrecision(t *testing.T) {
	for in, want := range map[string]string{
		"":                "",
		"0":               "0",
		"10.50":           "10.5",
		"1.000000":        "1",
		"0.0000001":       "0.0000001",
		"1000000000000":   "1000000000000",
		"0.1400000000001": "0.1400000000001",
		"not-a-number":    "not-a-number",
	} {
		if got := normalizeStoredLimit(in); got != want {
			t.Errorf("normalizeStoredLimit(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestBudgetViewComparesExactly(t *testing.T) {
	period := delcore.Period{Start: time.Unix(0, 0).UTC(), End: time.Unix(3600, 0).UTC()}
	const twoGiB = int64(2) << 30 // spends exactly USD 0.14
	for _, c := range []struct {
		limit     string
		exhausted bool
	}{
		{"0.14", true},
		{"0.13999999999999999999999999999999", true},
		{"0.1400000000000000000000000000001", false},
		{"0.1", true},
		{"0.0000001", true},
		{"1000000000000", false},
		{"", false},
	} {
		b := budgetView(c.limit, twoGiB, period)
		if b.GetLimitUsd() != c.limit || b.GetExhausted() != c.exhausted {
			t.Errorf("limit %q => %q exhausted=%v, want exhausted=%v", c.limit, b.GetLimitUsd(), b.GetExhausted(), c.exhausted)
		}
		if b.GetSpentUsd() != "0.14" {
			t.Errorf("spent_usd %q, want 0.14", b.GetSpentUsd())
		}
	}
}
