// SPDX-License-Identifier: FSL-1.1-ALv2

package pricing

import (
	"math"
	"math/big"
	"math/rand"
	"strings"
	"testing"
)

func TestUnitPriceMatchesPerByte(t *testing.T) {
	want, _ := new(big.Rat).SetString(UnitPricePerByte)
	if want.Cmp(PerByte()) != 0 {
		t.Fatalf("UnitPricePerByte %s != 0.07/2^30 %s", UnitPricePerByte, PerByte().FloatString(40))
	}
}

func TestEstimates(t *testing.T) {
	if got := EstimateUSD(1 << 30); got != "0.07" {
		t.Fatalf("1 GiB = %s", got)
	}
	if got := GiB(3 << 29); got != "1.5" {
		t.Fatalf("1.5 GiB = %s", got)
	}
	if got := EstimateUSD(0); got != "0" {
		t.Fatalf("zero = %s", got)
	}
	if got := EstimateUSD(10 << 30); got != "0.7" {
		t.Fatalf("10 GiB = %s", got)
	}
}

func TestExactEstimates(t *testing.T) {
	cases := []struct {
		bytes int64
		gib   string
		usd   string
	}{
		{0, "0", "0"},
		{1, "0.000000000931322574615478515625", "0.00000000006519258022308349609375"},
		{1000, "0.000000931322574615478515625", "0.00000006519258022308349609375"},
		{1 << 30, "1", "0.07"},
		{3 << 29, "1.5", "0.105"},
		{10 << 30, "10", "0.7"},
	}
	for _, c := range cases {
		if got := GiB(c.bytes); got != c.gib {
			t.Errorf("GiB(%d) = %s, want %s", c.bytes, got, c.gib)
		}
		if got := EstimateUSD(c.bytes); got != c.usd {
			t.Errorf("EstimateUSD(%d) = %s, want %s", c.bytes, got, c.usd)
		}
	}
	if got := EstimateUSD(1); got != UnitPricePerByte {
		t.Errorf("EstimateUSD(1) = %s, want UnitPricePerByte %s", got, UnitPricePerByte)
	}
}

// The exact strings must parse back to exactly the rational value, for the
// largest byte count as well, so no rounding happens at any width.
func TestExactEstimatesRoundTrip(t *testing.T) {
	rng := rand.New(rand.NewSource(660))
	counts := []int64{1, 2, 3, 999, 1000, 1<<30 - 1, 1<<30 + 1, math.MaxInt64, math.MaxInt64 - 1}
	for i := 0; i < 500; i++ {
		counts = append(counts, rng.Int63())
	}
	for _, n := range counts {
		gibStr, usdStr := GiB(n), EstimateUSD(n)
		wantGiB := new(big.Rat).Quo(new(big.Rat).SetInt64(n), gib)
		gotGiB, ok := new(big.Rat).SetString(gibStr)
		if !ok || gotGiB.Cmp(wantGiB) != 0 {
			t.Fatalf("GiB(%d) = %s is not exact (want %s)", n, gibStr, wantGiB.FloatString(40))
		}
		gotUSD, ok := new(big.Rat).SetString(usdStr)
		if !ok || gotUSD.Cmp(SpendUSD(n)) != 0 {
			t.Fatalf("EstimateUSD(%d) = %s is not exact (want %s)", n, usdStr, SpendUSD(n).FloatString(40))
		}
		// estimated_usd == total_gib * 0.07.
		if new(big.Rat).Mul(gotGiB, usdPerGiB).Cmp(gotUSD) != 0 {
			t.Fatalf("estimated_usd %s != total_gib %s * 0.07", usdStr, gibStr)
		}
	}
}

func TestParseLimit(t *testing.T) {
	ok := map[string]string{
		"0": "0", "10": "10", "10.50": "10.5", "0.000001": "0.000001", "1.000000": "1",
		// Limits are never rounded, whatever their scale or magnitude.
		"0.0000001":     "0.0000001",
		"1000000000000": "1000000000000",
		"1234567890123": "1234567890123",
		"1.0000001":     "1.0000001",
		"0.10":          "0.1",
		"0.000":         "0",
		"7.0":           "7",
	}
	for in, want := range ok {
		got, r, err := ParseLimit(in)
		if err != nil || got != want {
			t.Errorf("%q => %q, %v", in, got, err)
			continue
		}
		if wantRat, _ := new(big.Rat).SetString(want); r.Cmp(wantRat) != 0 {
			t.Errorf("%q parsed as %s, want %s", in, r.FloatString(10), want)
		}
	}
	for _, in := range []string{"", "-1", "+1", "01", "00", "1.", ".5", "1e3", "1E3", "0x10", "1,5", " 1", "1 ", "\t1", "NaN", "Inf", "-0", "1.5.2", "0.1\n"} {
		if _, _, err := ParseLimit(in); err == nil {
			t.Errorf("accepted %q", in)
		}
	}
}

func TestParseLimitLengthBound(t *testing.T) {
	// 64 characters: 32 integer digits, a dot and 31 fraction digits.
	long64 := "1" + strings.Repeat("0", 31) + "." + strings.Repeat("0", 30) + "1"
	if len(long64) != MaxLimitLength {
		t.Fatalf("fixture has %d characters", len(long64))
	}
	got, r, err := ParseLimit(long64)
	if err != nil || got != long64 {
		t.Fatalf("64-character limit => %q, %v", got, err)
	}
	if want, _ := new(big.Rat).SetString(long64); r.Cmp(want) != 0 {
		t.Fatalf("64-character limit parsed inexactly: %s", r.FloatString(40))
	}
	if _, _, err := ParseLimit(long64 + "0"); err == nil {
		t.Fatal("accepted a 65-character limit")
	}
	// The bound applies to the raw input, even when trailing fraction zeros
	// would shorten the canonical form.
	if _, _, err := ParseLimit("1." + strings.Repeat("0", 63)); err == nil {
		t.Fatal("accepted a 65-character limit that canonicalizes to 1")
	}
	if got, _, err := ParseLimit("1." + strings.Repeat("0", 62)); err != nil || got != "1" {
		t.Fatalf("64-character limit of trailing zeros => %q, %v", got, err)
	}
	if _, _, err := ParseLimit(strings.Repeat("9", 65)); err == nil {
		t.Fatal("accepted 65 integer digits")
	}
}

func TestCanonicalDecimal(t *testing.T) {
	for in, want := range map[string]string{"": "", "10.500": "10.5", "5": "5", "0.0": "0", "100": "100", "100.0": "100"} {
		got, ok := CanonicalDecimal(in)
		if in == "" {
			if ok {
				t.Errorf("accepted the empty string")
			}
			continue
		}
		if !ok || got != want {
			t.Errorf("%q => %q, %v; want %q", in, got, ok, want)
		}
	}
}
