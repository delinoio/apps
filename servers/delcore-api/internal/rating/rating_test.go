// SPDX-License-Identifier: FSL-1.1-ALv2

package rating

import (
	"errors"
	"testing"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

func d(t *testing.T, s string) accounting.Decimal {
	t.Helper()
	v, err := accounting.Parse(s)
	if err != nil {
		t.Fatal(err)
	}
	return v
}

var base = time.Date(2026, 3, 1, 0, 0, 0, 0, time.UTC)

func at(minutes int) time.Time { return base.Add(time.Duration(minutes) * time.Minute) }

func sumBucket(t *testing.T, group, price string, epoch int, minute int, sum string) Bucket {
	return Bucket{GroupKey: group, PriceID: price, Epoch: epoch, First: at(minute), Last: at(minute), Count: 1, Sum: d(t, sum)}
}

func expect(t *testing.T, got accounting.Decimal, want string) {
	t.Helper()
	if got.Cmp(d(t, want)) != 0 {
		t.Fatalf("got %s, want %s", got, want)
	}
}

func TestFormulaClampsGroupsAndSharesIncludedUsage(t *testing.T) {
	prices := map[string]accounting.Decimal{"p": d(t, "0.5")}
	// The issue example: groups -5 and 20 with 10 included bill 10 units.
	r, err := Compute(Input{
		Aggregation: metering.Sum,
		Groups:      []Group{{Key: "a", Value: d(t, "-5")}, {Key: "b", Value: d(t, "20")}},
		Buckets:     []Bucket{sumBucket(t, "a", "p", 1, 1, "-5"), sumBucket(t, "b", "p", 1, 2, "20")},
		Grants:      []accounting.Decimal{d(t, "10")},
		Prices:      prices,
	})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Quantity, "20")
	expect(t, r.Billable, "10")
	expect(t, r.Charge, "5")
	expect(t, r.Remaining, "0")

	// Included usage is granted once per tenant meter, not per group.
	r, err = Compute(Input{
		Aggregation: metering.Count,
		Groups:      []Group{{Key: "a", Value: d(t, "3")}, {Key: "b", Value: d(t, "4")}},
		Buckets: []Bucket{
			{GroupKey: "a", PriceID: "p", Epoch: 1, First: at(1), Last: at(3), Count: 3, Sum: accounting.Zero()},
			{GroupKey: "b", PriceID: "p", Epoch: 1, First: at(2), Last: at(4), Count: 4, Sum: accounting.Zero()},
		},
		Grants: []accounting.Decimal{d(t, "5")},
		Prices: prices,
	})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Billable, "2")
	expect(t, r.Charge, "1")

	// Negative groups never offset positive groups.
	r, err = Compute(Input{
		Aggregation: metering.Sum,
		Groups:      []Group{{Key: "a", Value: d(t, "-10")}, {Key: "b", Value: d(t, "4")}},
		Buckets:     []Bucket{sumBucket(t, "a", "p", 0, 1, "-10"), sumBucket(t, "b", "p", 0, 2, "4")},
		Prices:      prices,
	})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Billable, "4")
	expect(t, r.Charge, "2")

	// A wholly negative meter bills nothing and keeps its allowance.
	r, err = Compute(Input{
		Aggregation: metering.Sum,
		Groups:      []Group{{Key: "a", Value: d(t, "-3")}},
		Buckets:     []Bucket{sumBucket(t, "a", "p", 1, 1, "-3")},
		Grants:      []accounting.Decimal{d(t, "2")},
		Prices:      prices,
	})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Charge, "0")
	expect(t, r.Remaining, "2")
}

func TestEmptyPeriodKeepsGrantedAllowance(t *testing.T) {
	r, err := Compute(Input{Grants: []accounting.Decimal{d(t, "2"), d(t, "1.5")}})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Granted, "3.5")
	expect(t, r.Remaining, "3.5")
	expect(t, r.Charge, "0")
}

func TestMixedPricesRespectGrantTimingAndOccurrenceOrder(t *testing.T) {
	prices := map[string]accounting.Decimal{"old": d(t, "1"), "new": d(t, "3")}
	// Ten units at the old price after the first grant of four, then five units
	// at the new price after an upgrade grants three more. The old segment may
	// not use the later grant.
	r, err := Compute(Input{
		Aggregation: metering.Sum,
		Groups:      []Group{{Key: "g", Value: d(t, "15")}},
		Buckets:     []Bucket{sumBucket(t, "g", "new", 2, 20, "5"), sumBucket(t, "g", "old", 1, 10, "10")},
		Grants:      []accounting.Decimal{d(t, "4"), d(t, "3")},
		Prices:      prices,
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Segments) != 2 || r.Segments[0].PriceID != "old" || r.Segments[1].PriceID != "new" {
		t.Fatalf("segments are not chronological: %+v", r.Segments)
	}
	expect(t, r.Segments[0].Billable, "6")
	expect(t, r.Segments[1].Included, "3")
	expect(t, r.Segments[1].Billable, "2")
	expect(t, r.Charge, "12")
	expect(t, r.Remaining, "0")

	// A later negative contribution in the same group reduces the earliest
	// positive usage first, so no charge moves to another price.
	r, err = Compute(Input{
		Aggregation: metering.Sum,
		Groups:      []Group{{Key: "g", Value: d(t, "6")}},
		Buckets:     []Bucket{sumBucket(t, "g", "old", 0, 10, "10"), sumBucket(t, "g", "new", 0, 20, "-4")},
		Prices:      prices,
	})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Charge, "6")

	// Unpriced usage is metered but neither rated nor allowed to consume
	// included usage.
	r, err = Compute(Input{
		Aggregation: metering.Sum,
		Groups:      []Group{{Key: "g", Value: d(t, "7")}},
		Buckets:     []Bucket{sumBucket(t, "g", "", 1, 1, "4"), sumBucket(t, "g", "old", 1, 2, "3")},
		Grants:      []accounting.Decimal{d(t, "2")},
		Prices:      prices,
	})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Unrated, "4")
	expect(t, r.Billable, "1")
	expect(t, r.Charge, "1")
	expect(t, r.Remaining, "0")
}

func TestNonAdditiveGroupsUseTheirLatestContribution(t *testing.T) {
	prices := map[string]accounting.Decimal{"old": d(t, "1"), "new": d(t, "2")}
	for _, kind := range []metering.Aggregation{metering.Min, metering.Max, metering.Avg, metering.UniqueCount} {
		r, err := Compute(Input{
			Aggregation: kind,
			Groups:      []Group{{Key: "a", Value: d(t, "4.5")}, {Key: "b", Value: d(t, "-2")}},
			Buckets: []Bucket{
				{GroupKey: "a", PriceID: "old", Epoch: 1, First: at(1), Last: at(5), Count: 2, Sum: d(t, "1")},
				{GroupKey: "a", PriceID: "new", Epoch: 1, First: at(6), Last: at(9), Count: 1, Sum: d(t, "100")},
				{GroupKey: "b", PriceID: "old", Epoch: 1, First: at(2), Last: at(2), Count: 1, Sum: d(t, "-2")},
			},
			Grants: []accounting.Decimal{d(t, "1")},
			Prices: prices,
		})
		if err != nil {
			t.Fatal(kind, err)
		}
		// Group a (4.5) belongs to the new price; group b is clamped to zero.
		expect(t, r.Quantity, "4.5")
		expect(t, r.Billable, "3.5")
		expect(t, r.Charge, "7")
	}
}

func TestInconsistentInputsArePermanent(t *testing.T) {
	prices := map[string]accounting.Decimal{"p": d(t, "1")}
	for name, in := range map[string]Input{
		"sum mismatch":     {Aggregation: metering.Sum, Groups: []Group{{Key: "a", Value: d(t, "2")}}, Buckets: []Bucket{sumBucket(t, "a", "p", 0, 1, "3")}, Prices: prices},
		"group without":    {Aggregation: metering.Sum, Groups: []Group{{Key: "a", Value: d(t, "2")}}, Prices: prices},
		"bucket without":   {Aggregation: metering.Sum, Buckets: []Bucket{sumBucket(t, "a", "p", 0, 1, "3")}, Prices: prices},
		"missing price":    {Aggregation: metering.Sum, Groups: []Group{{Key: "a", Value: d(t, "3")}}, Buckets: []Bucket{sumBucket(t, "a", "q", 0, 1, "3")}, Prices: prices},
		"unknown kind":     {Aggregation: "median", Groups: []Group{{Key: "a", Value: d(t, "3")}}, Buckets: []Bucket{sumBucket(t, "a", "p", 0, 1, "3")}, Prices: prices},
		"epoch overflow":   {Aggregation: metering.Sum, Groups: []Group{{Key: "a", Value: d(t, "3")}}, Buckets: []Bucket{sumBucket(t, "a", "p", 2, 1, "3")}, Grants: []accounting.Decimal{d(t, "1")}, Prices: prices},
		"negative grant":   {Aggregation: metering.Sum, Grants: []accounting.Decimal{d(t, "-1")}, Prices: prices},
		"duplicate groups": {Aggregation: metering.Sum, Groups: []Group{{Key: "a", Value: d(t, "3")}, {Key: "a", Value: d(t, "3")}}, Buckets: []Bucket{sumBucket(t, "a", "p", 0, 1, "3")}, Prices: prices},
	} {
		if _, err := Compute(in); !errors.Is(err, ErrInconsistent) {
			t.Fatalf("%s: %v", name, err)
		}
	}
}

func TestRoundedChargeIsHalfUpAtTheEnd(t *testing.T) {
	for in, want := range map[string]string{"0.125": "0.13", "0.124999": "0.12", "-0.125": "-0.13", "0.006": "0.01", "0.0049": "0", "12": "12"} {
		got, err := RoundedCharge(d(t, in))
		if err != nil {
			t.Fatal(err)
		}
		expect(t, got, want)
	}
	// Two fractional events round once on their exact total, not per event.
	r, err := Compute(Input{
		Aggregation: metering.Sum,
		Groups:      []Group{{Key: "g", Value: d(t, "2")}},
		Buckets:     []Bucket{{GroupKey: "g", PriceID: "p", First: at(1), Last: at(2), Count: 2, Sum: d(t, "2")}},
		Prices:      map[string]accounting.Decimal{"p": d(t, "0.006")},
	})
	if err != nil {
		t.Fatal(err)
	}
	rounded, err := RoundedCharge(r.Charge)
	if err != nil {
		t.Fatal(err)
	}
	expect(t, r.Charge, "0.012")
	expect(t, rounded, "0.01")
}
