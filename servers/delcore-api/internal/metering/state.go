// SPDX-License-Identifier: FSL-1.1-ALv2

package metering

import (
	"fmt"
	"math"
	"math/big"
	"sort"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
)

type Aggregate struct {
	MeterKey      string
	GroupKey      string
	GroupValues   map[string]Value
	Kind          Aggregation
	Count         int64
	Sum           accounting.Decimal
	Value         accounting.Decimal
	DistinctCount int64
}

// Quantity is the value used by queries, limits, and rating. Average is already rounded to six places.
func (a Aggregate) Quantity() accounting.Decimal { return a.Value }

type Change struct {
	MeterKey string
	GroupKey string
	Before   *Aggregate
	After    *Aggregate
}

type RevisionKind string

const (
	Insert  RevisionKind = "insert"
	Replace RevisionKind = "replace"
	Cancel  RevisionKind = "cancel"
)

type Revision struct {
	Kind    RevisionKind
	EventID string
	Event   Event
}

// State is a pure snapshot. Callers persist active events and aggregate values in their own transaction.
type State struct {
	definitions []Definition
	active      map[string]Event
	groups      map[string]map[string]Aggregate
}

// Build reconstructs exact state from the current revision of each event.
func Build(definitions []Definition, active map[string]Event) (State, error) {
	defs, err := validateDefinitions(definitions)
	if err != nil {
		return State{}, err
	}
	defs = cloneDefinitions(defs)
	copyActive := make(map[string]Event, len(active))
	for id, event := range active {
		if id == "" {
			return State{}, fmt.Errorf("%w: event identity is required", ErrEvent)
		}
		copyActive[id], err = event.canonical()
		if err != nil {
			return State{}, err
		}
	}
	state := State{definitions: defs, active: copyActive, groups: make(map[string]map[string]Aggregate)}
	type bucket struct {
		definition Definition
		groupKey   string
		values     map[string]Value
		count      int64
		sum        accounting.Decimal
		distinct   map[Value]int64
	}
	buckets := make(map[string]map[string]*bucket)
	ids := make([]string, 0, len(copyActive))
	for id := range copyActive {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	byKey := make(map[string]Definition, len(defs))
	for _, d := range defs {
		byKey[d.Key] = d
	}
	for _, id := range ids {
		contributions, err := matchValidated(defs, copyActive[id])
		if err != nil {
			return State{}, fmt.Errorf("%w: event %q: %v", ErrEvent, id, err)
		}
		for _, contribution := range contributions {
			meterGroups := buckets[contribution.MeterKey]
			if meterGroups == nil {
				meterGroups = make(map[string]*bucket)
				buckets[contribution.MeterKey] = meterGroups
			}
			b := meterGroups[contribution.GroupKey]
			if b == nil {
				b = &bucket{definition: byKey[contribution.MeterKey], groupKey: contribution.GroupKey,
					values: contribution.GroupValues, sum: accounting.Zero(), distinct: make(map[Value]int64)}
				meterGroups[contribution.GroupKey] = b
			}
			if b.count == math.MaxInt64 {
				return State{}, fmt.Errorf("%w: aggregate count overflow", ErrEvent)
			}
			b.count++
			if b.definition.Aggregation != Count {
				if RetainsValues(b.definition.Aggregation) {
					b.distinct[contribution.Value]++
				}
				if b.definition.Aggregation != UniqueCount {
					n, _ := contribution.Value.decimal()
					b.sum = b.sum.Add(n)
				}
			}
		}
	}
	for meterKey, meterGroups := range buckets {
		state.groups[meterKey] = make(map[string]Aggregate, len(meterGroups))
		for groupKey, b := range meterGroups {
			aggregate := Aggregate{MeterKey: meterKey, GroupKey: groupKey, GroupValues: cloneValues(b.values),
				Kind: b.definition.Aggregation, Count: b.count, Sum: b.sum, DistinctCount: int64(len(b.distinct))}
			switch b.definition.Aggregation {
			case Count:
				aggregate.Value = integerDecimal(b.count)
			case Sum:
				aggregate.Value = b.sum
			case UniqueCount:
				aggregate.Value = integerDecimal(aggregate.DistinctCount)
			case Avg:
				aggregate.Value = roundedAverage(b.sum, b.count)
			case Min, Max:
				var extremum accounting.Decimal
				first := true
				for value := range b.distinct {
					n, _ := value.decimal()
					if first || (b.definition.Aggregation == Min && n.Cmp(extremum) < 0) ||
						(b.definition.Aggregation == Max && n.Cmp(extremum) > 0) {
						extremum = n
						first = false
					}
				}
				aggregate.Value = extremum
			}
			state.groups[meterKey][groupKey] = aggregate
		}
	}
	return state, nil
}

func integerDecimal(n int64) accounting.Decimal { return accounting.FromMinor(big.NewInt(n), 0) }

// RetainsValues reports whether an aggregation must keep its distinct value
// multiset to support exact removal. Count, sum, and average keep only their
// exact count and sum, so the distinct-value bound does not apply to them.
func RetainsValues(kind Aggregation) bool {
	return kind == Min || kind == Max || kind == UniqueCount
}

// RoundedAverage is the six-place half-up average used by queries, limits, and rating.
// count must be positive.
func RoundedAverage(sum accounting.Decimal, count int64) accounting.Decimal {
	return roundedAverage(sum, count)
}

// roundedAverage rounds a signed exact quotient half away from zero at six decimal places.
func roundedAverage(sum accounting.Decimal, count int64) accounting.Decimal {
	exact, _ := new(big.Rat).SetString(sum.String())
	scaled := exact.Mul(exact, new(big.Rat).SetFrac(big.NewInt(1_000_000), big.NewInt(count)))
	quotient, remainder := new(big.Int), new(big.Int)
	quotient.QuoRem(scaled.Num(), scaled.Denom(), remainder)
	if new(big.Int).Mul(new(big.Int).Abs(remainder), big.NewInt(2)).Cmp(scaled.Denom()) >= 0 {
		quotient.Add(quotient, big.NewInt(int64(scaled.Sign())))
	}
	return accounting.FromMinor(quotient, 6)
}

func cloneValues(values map[string]Value) map[string]Value {
	out := make(map[string]Value, len(values))
	for name, value := range values {
		out[name] = value
	}
	return out
}

func cloneDefinitions(defs []Definition) []Definition {
	out := make([]Definition, len(defs))
	for i, d := range defs {
		d.GroupBy = append([]string(nil), d.GroupBy...)
		d.Filters = append([]Filter(nil), d.Filters...)
		for j := range d.Filters {
			d.Filters[j].Values = append([]Value(nil), d.Filters[j].Values...)
		}
		out[i] = d
	}
	return out
}

// Aggregates returns detached group values so callers cannot change the snapshot.
func (s State) Aggregates() map[string]map[string]Aggregate {
	out := make(map[string]map[string]Aggregate, len(s.groups))
	for meter, groups := range s.groups {
		out[meter] = make(map[string]Aggregate, len(groups))
		for key, aggregate := range groups {
			aggregate.GroupValues = cloneValues(aggregate.GroupValues)
			out[meter][key] = aggregate
		}
	}
	return out
}

type GroupCardinality struct {
	DistinctValues int64
}

type MeterCardinality struct {
	Groups map[string]GroupCardinality
}

// Cardinality includes every active event supplied to Build, including accepted work not yet materialized.
func (s State) Cardinality() map[string]MeterCardinality {
	out := make(map[string]MeterCardinality, len(s.definitions))
	for _, definition := range s.definitions {
		meter := MeterCardinality{Groups: make(map[string]GroupCardinality)}
		for key, aggregate := range s.groups[definition.Key] {
			meter.Groups[key] = GroupCardinality{DistinctValues: aggregate.DistinctCount}
		}
		out[definition.Key] = meter
	}
	return out
}

type Limits struct {
	MaxGroupsPerMeter         int64
	MaxDistinctValuesPerGroup int64
}

// CheckLimits checks an admission snapshot. Zero means no limit for that dimension.
func (s State) CheckLimits(limits Limits) error {
	if limits.MaxGroupsPerMeter < 0 || limits.MaxDistinctValuesPerGroup < 0 {
		return fmt.Errorf("%w: negative limit", ErrLimit)
	}
	for meter, groups := range s.groups {
		if limits.MaxGroupsPerMeter > 0 && int64(len(groups)) > limits.MaxGroupsPerMeter {
			return fmt.Errorf("%w: meter %q group count", ErrLimit, meter)
		}
		for key, aggregate := range groups {
			if limits.MaxDistinctValuesPerGroup > 0 && aggregate.DistinctCount > limits.MaxDistinctValuesPerGroup {
				return fmt.Errorf("%w: meter %q group %q distinct count", ErrLimit, meter, key)
			}
		}
	}
	return nil
}

// Apply returns a new snapshot and exact before/after values for changed groups.
func (s State) Apply(revision Revision) (State, []Change, error) {
	if revision.EventID == "" {
		return State{}, nil, fmt.Errorf("%w: event identity is required", ErrRevision)
	}
	_, exists := s.active[revision.EventID]
	switch revision.Kind {
	case Insert:
		if exists {
			return State{}, nil, fmt.Errorf("%w: event already exists", ErrRevision)
		}
	case Replace, Cancel:
		if !exists {
			return State{}, nil, fmt.Errorf("%w: event does not exist", ErrRevision)
		}
	default:
		return State{}, nil, fmt.Errorf("%w: unknown revision kind", ErrRevision)
	}
	active := make(map[string]Event, len(s.active)+1)
	for id, event := range s.active {
		active[id] = event
	}
	if revision.Kind == Cancel {
		delete(active, revision.EventID)
	} else {
		active[revision.EventID] = revision.Event
	}
	next, err := Build(s.definitions, active)
	if err != nil {
		return State{}, nil, err
	}
	var changes []Change
	for _, d := range s.definitions {
		keys := make(map[string]bool)
		for key := range s.groups[d.Key] {
			keys[key] = true
		}
		for key := range next.groups[d.Key] {
			keys[key] = true
		}
		ordered := make([]string, 0, len(keys))
		for key := range keys {
			ordered = append(ordered, key)
		}
		sort.Strings(ordered)
		for _, key := range ordered {
			before, hadBefore := s.groups[d.Key][key]
			after, hasAfter := next.groups[d.Key][key]
			if hadBefore && hasAfter && sameAggregate(before, after) {
				continue
			}
			change := Change{MeterKey: d.Key, GroupKey: key}
			if hadBefore {
				before.GroupValues = cloneValues(before.GroupValues)
				change.Before = &before
			}
			if hasAfter {
				after.GroupValues = cloneValues(after.GroupValues)
				change.After = &after
			}
			changes = append(changes, change)
		}
	}
	return next, changes, nil
}

func sameAggregate(a, b Aggregate) bool {
	return a.Count == b.Count && a.DistinctCount == b.DistinctCount &&
		a.Sum.Cmp(b.Sum) == 0 && a.Value.Cmp(b.Value) == 0
}
