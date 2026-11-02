// SPDX-License-Identifier: FSL-1.1-ALv2

package pricing

import (
	"math/big"
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

func TestParseLimit(t *testing.T) {
	ok := map[string]string{"0": "0", "10": "10", "10.50": "10.5", "0.000001": "0.000001", "1.000000": "1"}
	for in, want := range ok {
		got, _, err := ParseLimit(in)
		if err != nil || got != want {
			t.Errorf("%q => %q, %v", in, got, err)
		}
	}
	for _, in := range []string{"", "-1", "01", "1.", ".5", "1e3", "1.0000001", "1234567890123", " 1", "NaN"} {
		if _, _, err := ParseLimit(in); err == nil {
			t.Errorf("accepted %q", in)
		}
	}
}
