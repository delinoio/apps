// SPDX-License-Identifier: FSL-1.1-ALv2

// Package rating computes the exact usage charge of one tenant meter billing
// period from its materialized group aggregates. It is pure: callers load the
// aggregates, contribution buckets, allowance grants, and unit prices in their
// own transaction and reconcile the result against the ledger.
//
// For one unit price and one allowance grant the charge is
//
//	max(0, sum(max(0, group_aggregate)) - shared_included_usage) * unit_price
//
// Each group is clamped before summing, so a negative group never offsets a
// positive one, and included usage is shared once per tenant meter period
// instead of being granted to each group.
//
// Price history and allowance grants split a period into segments. A segment
// holds the contributions that share the price attributed at acceptance and
// the allowance epoch of their occurrence time. Segments are ordered by epoch,
// then by their earliest occurrence, then by price ID. Additive aggregations
// (count and sum) split each group's clamped quantity across its segments,
// filling the earliest positive segment contributions first. Non-additive
// aggregations (min, max, avg, unique_count) cannot be decomposed, so the whole
// clamped group quantity belongs to the segment of the group's latest
// contribution. Included usage is consumed in segment order; a segment can use
// only grants applied at or before the occurrences in its epoch. Each segment's
// billable remainder is charged at its own unit price. Contributions without
// an applicable usage price are metered but neither consume included usage nor
// produce a charge.
package rating

import (
	"errors"
	"fmt"
	"math/big"
	"sort"
	"time"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

// ErrInconsistent reports stored metering state that cannot be rated, such as
// contributions that disagree with the materialized aggregates. Retrying the
// same state cannot succeed.
var ErrInconsistent = errors.New("inconsistent rating input")

// Group is one materialized group aggregate. Value is the exposed result:
// the count, sum, minimum, maximum, rounded average, or distinct count.
type Group struct {
	Key   string
	Value accounting.Decimal
}

// Bucket summarizes the applied contributions of one group that share a price
// attribution and an allowance epoch.
type Bucket struct {
	GroupKey string
	// PriceID is the usage price attributed when the event was accepted, or
	// empty when no usage price applied at its occurrence.
	PriceID string
	// Epoch is the number of allowance grants applied at or before the
	// occurrence of these contributions. Grants[:Epoch] are available to them.
	Epoch int
	First time.Time
	Last  time.Time
	Count int64
	Sum   accounting.Decimal
}

// Input is everything rating needs for one tenant meter billing period.
type Input struct {
	Aggregation metering.Aggregation
	Groups      []Group
	Buckets     []Bucket
	// Grants holds allowance grant amounts in applied order.
	Grants []accounting.Decimal
	// Prices maps every attributed price ID to its unit price.
	Prices map[string]accounting.Decimal
}

// Segment is the rated share of one price attribution and allowance epoch.
type Segment struct {
	PriceID  string
	Epoch    int
	Quantity accounting.Decimal
	Included accounting.Decimal
	Billable accounting.Decimal
	Charge   accounting.Decimal
}

// Result is the exact, unrounded rating of one tenant meter billing period.
type Result struct {
	// Quantity is the sum of clamped group aggregates.
	Quantity accounting.Decimal
	// Unrated is the part of Quantity without an applicable usage price.
	Unrated accounting.Decimal
	// Granted is the included usage of every grant of the period.
	Granted accounting.Decimal
	// Consumed is the included usage used by rated quantity.
	Consumed accounting.Decimal
	// Remaining is Granted minus Consumed.
	Remaining accounting.Decimal
	// Billable is the rated quantity beyond included usage.
	Billable accounting.Decimal
	// Charge is the exact charge before final currency rounding.
	Charge   accounting.Decimal
	Segments []Segment
}

// Additive reports whether an aggregation decomposes into a sum over events.
func Additive(kind metering.Aggregation) bool {
	return kind == metering.Count || kind == metering.Sum
}

func maxZero(d accounting.Decimal) accounting.Decimal {
	if d.Cmp(accounting.Zero()) < 0 {
		return accounting.Zero()
	}
	return d
}

func minDecimal(a, b accounting.Decimal) accounting.Decimal {
	if a.Cmp(b) < 0 {
		return a
	}
	return b
}

type segmentKey struct {
	epoch int
	price string
}

// Compute rates one tenant meter billing period. It returns ErrInconsistent
// when buckets and groups disagree or a price is missing.
func Compute(in Input) (Result, error) {
	zero := accounting.Zero()
	r := Result{Quantity: zero, Unrated: zero, Granted: zero, Consumed: zero, Remaining: zero, Billable: zero, Charge: zero}
	cumulative := make([]accounting.Decimal, len(in.Grants)+1)
	cumulative[0] = zero
	for i, amount := range in.Grants {
		if amount.Cmp(zero) < 0 {
			return Result{}, fmt.Errorf("%w: negative allowance grant", ErrInconsistent)
		}
		cumulative[i+1] = cumulative[i].Add(amount)
	}
	r.Granted = cumulative[len(in.Grants)]
	r.Remaining = r.Granted
	if len(in.Groups) == 0 && len(in.Buckets) == 0 {
		return r, nil
	}
	switch in.Aggregation {
	case metering.Count, metering.Sum, metering.Min, metering.Max, metering.Avg, metering.UniqueCount:
	default:
		return Result{}, fmt.Errorf("%w: unknown aggregation %q", ErrInconsistent, in.Aggregation)
	}

	// Order segments deterministically: allowance epoch, earliest occurrence,
	// then price ID.
	first := map[segmentKey]time.Time{}
	byGroup := map[string][]Bucket{}
	for _, b := range in.Buckets {
		if b.Epoch < 0 || b.Epoch > len(in.Grants) || b.Count <= 0 {
			return Result{}, fmt.Errorf("%w: invalid contribution bucket", ErrInconsistent)
		}
		if b.PriceID != "" {
			if _, ok := in.Prices[b.PriceID]; !ok {
				return Result{}, fmt.Errorf("%w: attributed price is missing", ErrInconsistent)
			}
		}
		k := segmentKey{b.Epoch, b.PriceID}
		if at, ok := first[k]; !ok || b.First.Before(at) {
			first[k] = b.First
		}
		byGroup[b.GroupKey] = append(byGroup[b.GroupKey], b)
	}
	keys := make([]segmentKey, 0, len(first))
	for k := range first {
		keys = append(keys, k)
	}
	sort.Slice(keys, func(i, j int) bool {
		a, b := keys[i], keys[j]
		if a.epoch != b.epoch {
			return a.epoch < b.epoch
		}
		if !first[a].Equal(first[b]) {
			return first[a].Before(first[b])
		}
		return a.price < b.price
	})
	index := make(map[segmentKey]int, len(keys))
	quantities := make([]accounting.Decimal, len(keys))
	for i, k := range keys {
		index[k] = i
		quantities[i] = zero
	}

	groups := make(map[string]bool, len(in.Groups))
	for _, g := range in.Groups {
		if groups[g.Key] {
			return Result{}, fmt.Errorf("%w: duplicate group", ErrInconsistent)
		}
		groups[g.Key] = true
		buckets := byGroup[g.Key]
		if len(buckets) == 0 {
			return Result{}, fmt.Errorf("%w: group has no applied contributions", ErrInconsistent)
		}
		clamped := maxZero(g.Value)
		r.Quantity = r.Quantity.Add(clamped)
		if !Additive(in.Aggregation) {
			// The latest contribution decides the segment; ties fall back to the
			// later epoch and then the price ID.
			latest := buckets[0]
			for _, b := range buckets[1:] {
				if b.Last.After(latest.Last) || b.Last.Equal(latest.Last) && (b.Epoch > latest.Epoch || b.Epoch == latest.Epoch && b.PriceID > latest.PriceID) {
					latest = b
				}
			}
			i := index[segmentKey{latest.Epoch, latest.PriceID}]
			quantities[i] = quantities[i].Add(clamped)
			continue
		}
		sort.Slice(buckets, func(i, j int) bool {
			return index[segmentKey{buckets[i].Epoch, buckets[i].PriceID}] < index[segmentKey{buckets[j].Epoch, buckets[j].PriceID}]
		})
		total := zero
		for _, b := range buckets {
			total = total.Add(bucketValue(in.Aggregation, b))
		}
		if total.Cmp(g.Value) != 0 {
			return Result{}, fmt.Errorf("%w: contributions differ from the materialized aggregate", ErrInconsistent)
		}
		// Fill the earliest positive contributions first; later negative
		// contributions reduce the group total without moving charges to a
		// different price.
		left := clamped
		for _, b := range buckets {
			if left.Cmp(zero) == 0 {
				break
			}
			value := bucketValue(in.Aggregation, b)
			if value.Cmp(zero) <= 0 {
				continue
			}
			share := minDecimal(value, left)
			left = left.Sub(share)
			i := index[segmentKey{b.Epoch, b.PriceID}]
			quantities[i] = quantities[i].Add(share)
		}
	}
	for key := range byGroup {
		if !groups[key] {
			return Result{}, fmt.Errorf("%w: contributions have no materialized group", ErrInconsistent)
		}
	}

	for i, k := range keys {
		s := Segment{PriceID: k.price, Epoch: k.epoch, Quantity: quantities[i], Included: zero, Billable: zero, Charge: zero}
		if k.price == "" {
			r.Unrated = r.Unrated.Add(s.Quantity)
			r.Segments = append(r.Segments, s)
			continue
		}
		available := maxZero(cumulative[k.epoch].Sub(r.Consumed))
		s.Included = minDecimal(s.Quantity, available)
		s.Billable = s.Quantity.Sub(s.Included)
		s.Charge = s.Billable.Mul(in.Prices[k.price])
		r.Consumed = r.Consumed.Add(s.Included)
		r.Billable = r.Billable.Add(s.Billable)
		r.Charge = r.Charge.Add(s.Charge)
		r.Segments = append(r.Segments, s)
	}
	r.Remaining = r.Granted.Sub(r.Consumed)
	return r, nil
}

func bucketValue(kind metering.Aggregation, b Bucket) accounting.Decimal {
	if kind == metering.Count {
		return accounting.FromMinor(big.NewInt(b.Count), 0)
	}
	return b.Sum
}

// RoundedCharge applies the final half-up USD minor-unit rounding to an exact
// charge.
func RoundedCharge(charge accounting.Decimal) (accounting.Decimal, error) {
	units, err := charge.MinorUnits(accounting.USDMinorUnits)
	if err != nil {
		return accounting.Decimal{}, err
	}
	return accounting.FromMinor(units, accounting.USDMinorUnits), nil
}
