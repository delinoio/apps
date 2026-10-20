// SPDX-License-Identifier: FSL-1.1-ALv2

package metering

import (
	"encoding/json"
	"errors"
	"reflect"
	"testing"
)

func number(t *testing.T, text string) Value {
	t.Helper()
	v, err := Number(text)
	if err != nil {
		t.Fatal(err)
	}
	return v
}

func TestFlatValuesAndCanonicalGroups(t *testing.T) {
	for _, raw := range []any{nil, 1.5, map[string]any{"nested": true}, []string{"nested"}} {
		if _, err := ParseValue(raw); !errors.Is(err, ErrEvent) {
			t.Fatalf("accepted non-flat or imprecise value %T: %v", raw, err)
		}
	}
	if got, err := ParseValue(json.Number("1.200")); err != nil || got != number(t, "1.2") {
		t.Fatalf("number was not canonical: %v, %v", got, err)
	}
	first, err := GroupKey(map[string]Value{"region": String("a|b"), "tier": number(t, "1.00")})
	if err != nil {
		t.Fatal(err)
	}
	second, err := GroupKey(map[string]Value{"tier": number(t, "1"), "region": String("a|b")})
	if err != nil || first != second {
		t.Fatalf("group order or decimal spelling changed the key: %q %q %v", first, second, err)
	}
	other, _ := GroupKey(map[string]Value{"region": String("a|b"), "tier": String("1")})
	if first == other {
		t.Fatal("string and number groups collided")
	}
	if _, err := GroupKey(map[string]Value{"": String("x")}); !errors.Is(err, ErrEvent) {
		t.Fatal("empty group attribute accepted")
	}
}

func TestFiltersANDAndMatchedMeterValidation(t *testing.T) {
	filters := []Filter{
		{Attribute: "region", Operator: Equal, Value: String("eu")},
		{Attribute: "region", Operator: NotEqual, Value: String("us")},
		{Attribute: "plan", Operator: In, Values: []Value{String("pro"), String("enterprise")}},
		{Attribute: "value", Operator: GreaterThan, Value: number(t, "-2")},
		{Attribute: "value", Operator: GreaterThanOrEqual, Value: number(t, "-1")},
		{Attribute: "value", Operator: LessThan, Value: number(t, "0")},
		{Attribute: "value", Operator: LessThanOrEqual, Value: number(t, "-1")},
		{Attribute: "optional", Operator: Exists, Value: Boolean(false)},
		{Attribute: "active", Operator: Exists, Value: Boolean(true)},
	}
	defs := []Definition{
		{Key: "sum", EventType: "compute", Aggregation: Sum, ValueAttribute: "value", GroupBy: []string{"region"}, Filters: filters},
		{Key: "count", EventType: "compute", Aggregation: Count, GroupBy: []string{"plan"}},
	}
	event := Event{Type: "compute", Attributes: map[string]Value{
		"region": String("eu"), "plan": String("pro"), "value": number(t, "-1.0"), "active": Boolean(true),
	}}
	contributions, err := Match(defs, event)
	if err != nil || len(contributions) != 2 || contributions[0].MeterKey != "count" || contributions[1].MeterKey != "sum" {
		t.Fatalf("unexpected match: %+v %v", contributions, err)
	}
	bad := event
	bad.Attributes = cloneValues(event.Attributes)
	delete(bad.Attributes, "plan")
	if contributions, err = Match(defs, bad); !errors.Is(err, ErrEvent) || contributions != nil {
		t.Fatalf("a matched meter was partially applied: %+v %v", contributions, err)
	}
	bad = event
	bad.Attributes = cloneValues(event.Attributes)
	bad.Attributes["value"] = String("-1")
	if contributions, err = Match([]Definition{{Key: "plain-sum", EventType: "compute", Aggregation: Sum, ValueAttribute: "value"}}, bad); !errors.Is(err, ErrEvent) || contributions != nil {
		t.Fatalf("wrong numeric type was accepted: %+v %v", contributions, err)
	}
	bad = event
	bad.Attributes = cloneValues(event.Attributes)
	bad.Attributes["region"] = String("us")
	contributions, err = Match(defs, bad)
	if err != nil || len(contributions) != 1 || contributions[0].MeterKey != "count" {
		t.Fatalf("AND filters did not exclude only the sum meter: %+v %v", contributions, err)
	}
	if _, err = Match(defs, Event{Type: "other"}); !errors.Is(err, ErrNoMatch) {
		t.Fatalf("unmatched event: %v", err)
	}
}

func TestDefinitionValidation(t *testing.T) {
	cases := []Definition{
		{Key: "x", EventType: "x", Aggregation: Sum},
		{Key: "x", EventType: "x", Aggregation: Count, ValueAttribute: "value"},
		{Key: "x", EventType: "x", Aggregation: Count, GroupBy: []string{"a", "a"}},
		{Key: "x", EventType: "x", Aggregation: Count, Filters: []Filter{{Attribute: "a", Operator: In}}},
		{Key: "x", EventType: "x", Aggregation: Count, Filters: []Filter{{Attribute: "a", Operator: GreaterThan, Value: String("1")}}},
		{Key: "x", EventType: "x", Aggregation: Count, Filters: []Filter{{Attribute: "a", Operator: Exists, Value: String("true")}}},
	}
	for _, d := range cases {
		if err := d.Validate(); !errors.Is(err, ErrDefinition) {
			t.Fatalf("invalid definition accepted: %+v %v", d, err)
		}
	}
	d := Definition{Key: "x", EventType: "x", Aggregation: Count}
	if _, err := Match([]Definition{d, d}, Event{Type: "x"}); !errors.Is(err, ErrDefinition) {
		t.Fatalf("duplicate meter key accepted: %v", err)
	}
}

func TestSixAggregationsAndExactArithmetic(t *testing.T) {
	definitions := []Definition{
		{Key: "count", EventType: "use", Aggregation: Count},
		{Key: "sum", EventType: "use", Aggregation: Sum, ValueAttribute: "n"},
		{Key: "min", EventType: "use", Aggregation: Min, ValueAttribute: "n"},
		{Key: "max", EventType: "use", Aggregation: Max, ValueAttribute: "n"},
		{Key: "avg", EventType: "use", Aggregation: Avg, ValueAttribute: "n"},
		{Key: "unique", EventType: "use", Aggregation: UniqueCount, ValueAttribute: "tag"},
	}
	active := map[string]Event{
		"e1": {Type: "use", Attributes: map[string]Value{"n": number(t, "-10.1"), "tag": String("same")}},
		"e2": {Type: "use", Attributes: map[string]Value{"n": number(t, "0.2"), "tag": String("same")}},
		"e3": {Type: "use", Attributes: map[string]Value{"n": number(t, "0.3"), "tag": String("new")}},
	}
	state, err := Build(definitions, active)
	if err != nil {
		t.Fatal(err)
	}
	got := make(map[string]string)
	for meter, groups := range state.Aggregates() {
		got[meter] = groups["[]"].Quantity().String()
	}
	want := map[string]string{"count": "3", "sum": "-9.6", "min": "-10.1", "max": "0.3", "avg": "-3.2", "unique": "2"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("aggregates = %v, want %v", got, want)
	}
	avg := state.Aggregates()["avg"]["[]"]
	if avg.Count != 3 || avg.Sum.String() != "-9.6" {
		t.Fatalf("average lost exact sum/count: %+v", avg)
	}
	if state.Aggregates()["unique"]["[]"].DistinctCount != 2 {
		t.Fatal("unique count was approximate")
	}
	delete(active, "e1")
	if state.Aggregates()["count"]["[]"].Count != 3 {
		t.Fatal("caller mutation changed snapshot")
	}
}

func TestAverageHalfUpBoundaries(t *testing.T) {
	d := []Definition{{Key: "avg", EventType: "use", Aggregation: Avg, ValueAttribute: "n"}}
	for _, tc := range []struct{ input, want string }{
		{"0.0000005", "0.000001"},
		{"-0.0000005", "-0.000001"},
		{"0.00000049", "0"},
		{"-0.00000049", "0"},
		{"1.2345675", "1.234568"},
		{"-1.2345675", "-1.234568"},
	} {
		state, err := Build(d, map[string]Event{"a": {Type: "use", Attributes: map[string]Value{"n": number(t, tc.input)}}})
		if err != nil {
			t.Fatal(err)
		}
		if got := state.Aggregates()["avg"]["[]"].Value.String(); got != tc.want {
			t.Fatalf("average %s = %s, want %s", tc.input, got, tc.want)
		}
	}
	state, err := Build(d, map[string]Event{
		"a": {Type: "use", Attributes: map[string]Value{"n": number(t, "0.000001")}},
		"b": {Type: "use", Attributes: map[string]Value{"n": number(t, "0")}},
	})
	if err != nil || state.Aggregates()["avg"]["[]"].Value.String() != "0.000001" {
		t.Fatalf("quotient boundary failed: %v %v", state.Aggregates(), err)
	}
}

func TestRevisionChangesAndCardinality(t *testing.T) {
	d := []Definition{{Key: "unique", EventType: "use", Aggregation: UniqueCount, ValueAttribute: "tag", GroupBy: []string{"region"}}}
	state, err := Build(d, nil)
	if err != nil {
		t.Fatal(err)
	}
	event := func(region, tag string) Event {
		return Event{Type: "use", Attributes: map[string]Value{"region": String(region), "tag": String(tag)}}
	}
	state, changes, err := state.Apply(Revision{Kind: Insert, EventID: "a", Event: event("eu", "x")})
	if err != nil || len(changes) != 1 || changes[0].Before != nil || changes[0].After.Value.String() != "1" {
		t.Fatalf("insert changes: %+v %v", changes, err)
	}
	state, _, err = state.Apply(Revision{Kind: Insert, EventID: "b", Event: event("eu", "x")})
	if err != nil {
		t.Fatal(err)
	}
	if err := state.CheckLimits(Limits{MaxGroupsPerMeter: 1, MaxDistinctValuesPerGroup: 1}); err != nil {
		t.Fatalf("duplicate value used another distinct slot: %v", err)
	}
	previous := state
	state, changes, err = state.Apply(Revision{Kind: Replace, EventID: "a", Event: event("us", "y")})
	if err != nil || len(changes) != 2 || changes[0].Before == nil || changes[0].After == nil ||
		changes[1].Before != nil || changes[1].After == nil {
		t.Fatalf("group replacement changes: %+v %v", changes, err)
	}
	if err := state.CheckLimits(Limits{MaxGroupsPerMeter: 1}); !errors.Is(err, ErrLimit) {
		t.Fatalf("new group did not consume capacity: %v", err)
	}
	if len(previous.Cardinality()["unique"].Groups) != 1 || len(state.Cardinality()["unique"].Groups) != 2 {
		t.Fatal("previous snapshot changed")
	}
	state, changes, err = state.Apply(Revision{Kind: Cancel, EventID: "b"})
	if err != nil || len(changes) != 1 || changes[0].After != nil {
		t.Fatalf("cancel did not remove empty old group: %+v %v", changes, err)
	}
	if err := state.CheckLimits(Limits{MaxGroupsPerMeter: 1, MaxDistinctValuesPerGroup: 1}); err != nil {
		t.Fatal(err)
	}
	if _, _, err := state.Apply(Revision{Kind: Insert, EventID: "a", Event: event("eu", "x")}); !errors.Is(err, ErrRevision) {
		t.Fatalf("duplicate insert accepted: %v", err)
	}
	if _, _, err := state.Apply(Revision{Kind: Replace, EventID: "missing", Event: event("eu", "x")}); !errors.Is(err, ErrRevision) {
		t.Fatalf("missing replacement accepted: %v", err)
	}
}

func TestDistinctRemovalAndFullEventRejection(t *testing.T) {
	d := []Definition{
		{Key: "a", EventType: "use", Aggregation: UniqueCount, ValueAttribute: "tag"},
		{Key: "b", EventType: "use", Aggregation: Sum, ValueAttribute: "n"},
	}
	state, err := Build(d, map[string]Event{
		"a": {Type: "use", Attributes: map[string]Value{"tag": String("x"), "n": number(t, "1")}},
		"b": {Type: "use", Attributes: map[string]Value{"tag": String("x"), "n": number(t, "2")}},
		"c": {Type: "use", Attributes: map[string]Value{"tag": String("y"), "n": number(t, "3")}},
	})
	if err != nil {
		t.Fatal(err)
	}
	state, _, err = state.Apply(Revision{Kind: Cancel, EventID: "a"})
	if err != nil || state.Aggregates()["a"]["[]"].Value.String() != "2" {
		t.Fatalf("first duplicate removal changed unique count: %v %v", state.Aggregates(), err)
	}
	state, _, err = state.Apply(Revision{Kind: Cancel, EventID: "b"})
	if err != nil || state.Aggregates()["a"]["[]"].Value.String() != "1" {
		t.Fatalf("last duplicate removal missed unique count: %v %v", state.Aggregates(), err)
	}
	_, _, err = state.Apply(Revision{Kind: Insert, EventID: "bad", Event: Event{Type: "use", Attributes: map[string]Value{"tag": String("z")}}})
	if !errors.Is(err, ErrEvent) || state.Aggregates()["b"]["[]"].Value.String() != "3" {
		t.Fatalf("invalid multi-meter event changed state: %v %v", state.Aggregates(), err)
	}
	next, _, err := state.Apply(Revision{Kind: Insert, EventID: "new", Event: Event{Type: "use", Attributes: map[string]Value{"tag": String("z"), "n": number(t, "4")}}})
	if err != nil || !errors.Is(next.CheckLimits(Limits{MaxDistinctValuesPerGroup: 1}), ErrLimit) {
		t.Fatalf("distinct limit did not count pending value: %v", err)
	}
}

func TestExtremaRecomputeAfterReplacementAndCancellation(t *testing.T) {
	definitions := []Definition{
		{Key: "min", EventType: "use", Aggregation: Min, ValueAttribute: "n"},
		{Key: "max", EventType: "use", Aggregation: Max, ValueAttribute: "n"},
		{Key: "sum", EventType: "use", Aggregation: Sum, ValueAttribute: "n"},
	}
	event := func(n string) Event { return Event{Type: "use", Attributes: map[string]Value{"n": number(t, n)}} }
	state, err := Build(definitions, map[string]Event{"a": event("-3"), "b": event("-3.0"), "c": event("5")})
	if err != nil {
		t.Fatal(err)
	}
	state, _, err = state.Apply(Revision{Kind: Replace, EventID: "a", Event: event("8")})
	if err != nil || state.Aggregates()["min"]["[]"].Value.String() != "-3" || state.Aggregates()["max"]["[]"].Value.String() != "8" {
		t.Fatalf("replacement lost remaining minimum or new maximum: %v %v", state.Aggregates(), err)
	}
	state, _, err = state.Apply(Revision{Kind: Cancel, EventID: "b"})
	if err != nil || state.Aggregates()["min"]["[]"].Value.String() != "5" || state.Aggregates()["sum"]["[]"].Sum.String() != "13" {
		t.Fatalf("cancellation did not remove last minimum: %v %v", state.Aggregates(), err)
	}
	state, _, err = state.Apply(Revision{Kind: Cancel, EventID: "a"})
	if err != nil || state.Aggregates()["max"]["[]"].Value.String() != "5" {
		t.Fatalf("cancellation did not remove maximum: %v %v", state.Aggregates(), err)
	}
}

func TestNumericFilterBoundariesAndMissingAttributes(t *testing.T) {
	comparisons := []struct {
		operator FilterOperator
		value    string
		match    bool
	}{
		{GreaterThan, "1", false},
		{GreaterThanOrEqual, "1", true},
		{LessThan, "1", false},
		{LessThanOrEqual, "1", true},
		{GreaterThan, "0.999999", true},
		{LessThan, "1.000001", true},
	}
	for _, tc := range comparisons {
		d := Definition{Key: "x", EventType: "use", Aggregation: Count,
			Filters: []Filter{{Attribute: "n", Operator: tc.operator, Value: number(t, tc.value)}}}
		_, err := Match([]Definition{d}, Event{Type: "use", Attributes: map[string]Value{"n": number(t, "1.0")}})
		if (err == nil) != tc.match {
			t.Fatalf("%s at %s: match=%v err=%v", tc.operator, tc.value, tc.match, err)
		}
		if _, err := Match([]Definition{d}, Event{Type: "use"}); !errors.Is(err, ErrNoMatch) {
			t.Fatalf("missing numeric filter attribute was accepted: %v", err)
		}
	}
}
