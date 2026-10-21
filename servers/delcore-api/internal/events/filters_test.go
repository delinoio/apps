// SPDX-License-Identifier: FSL-1.1-ALv2

package events

import (
	"encoding/json"
	"testing"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

const (
	opEqual    = pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL
	opNotEqual = pb.MeterFilterOperator_METER_FILTER_OPERATOR_NOT_EQUAL
	opIn       = pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN
	opGreater  = pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER
	opExists   = pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS
	kindString = pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_STRING
	kindNumber = pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_NUMBER
	kindBool   = pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_BOOLEAN
)

func typed(kind pb.MeterFilterValueKind, value string) *pb.MeterFilterValue {
	return &pb.MeterFilterValue{Kind: kind, Value: value}
}

func storedJSON(t *testing.T, filters ...*pb.MeterFilter) string {
	t.Helper()
	canonical, err := CanonicalFilters(filters)
	if err != nil {
		t.Fatal(err)
	}
	raw, err := json.Marshal(canonical)
	if err != nil {
		t.Fatal(err)
	}
	return string(raw)
}

// Definitions that were expressible before typed operands keep their stored
// JSON, so their semantic hashes and version reuse are unchanged.
func TestCanonicalFiltersKeepEarlierStoredForms(t *testing.T) {
	for name, c := range map[string]struct {
		filter *pb.MeterFilter
		want   string
	}{
		"membership":        {&pb.MeterFilter{Attribute: "o", Operator: opIn, Values: []string{"success", "cached"}}, `[{"attribute":"o","operator":3,"values":["cached","success"]}]`},
		"equality":          {&pb.MeterFilter{Attribute: "o", Operator: opEqual, Values: []string{"x"}}, `[{"attribute":"o","operator":1,"values":["x"]}]`},
		"numeric canonical": {&pb.MeterFilter{Attribute: "n", Operator: opGreater, Values: []string{"2.50"}}, `[{"attribute":"n","operator":4,"values":["2.5"]}]`},
		"exists true":       {&pb.MeterFilter{Attribute: "d", Operator: opExists, Values: []string{"true"}}, `[{"attribute":"d","operator":8,"values":["true"]}]`},
		"exists false":      {&pb.MeterFilter{Attribute: "d", Operator: opExists, Values: []string{"false"}}, `[{"attribute":"d","operator":8,"values":["false"]}]`},
	} {
		if got := storedJSON(t, c.filter); got != c.want {
			t.Fatalf("%s stored as %s, want %s", name, got, c.want)
		}
	}
}

func TestCanonicalFiltersNormalizeEquivalentForms(t *testing.T) {
	presence := storedJSON(t, &pb.MeterFilter{Attribute: "d", Operator: opExists, Values: []string{"true"}})
	for _, equivalent := range []*pb.MeterFilter{
		{Attribute: "d", Operator: opExists},
		{Attribute: "d", Operator: opExists, TypedValues: []*pb.MeterFilterValue{typed(kindBool, "true")}},
	} {
		if got := storedJSON(t, equivalent); got != presence {
			t.Fatalf("exists form %+v stored as %s, want %s", equivalent, got, presence)
		}
	}
	text := storedJSON(t, &pb.MeterFilter{Attribute: "o", Operator: opIn, Values: []string{"b", "a"}})
	if got := storedJSON(t, &pb.MeterFilter{Attribute: "o", Operator: opIn, TypedValues: []*pb.MeterFilterValue{typed(kindString, "a"), typed(kindString, "b")}}); got != text {
		t.Fatalf("typed strings stored as %s, want %s", got, text)
	}
	numeric := storedJSON(t, &pb.MeterFilter{Attribute: "n", Operator: opGreater, Values: []string{"2.5"}})
	if got := storedJSON(t, &pb.MeterFilter{Attribute: "n", Operator: opGreater, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "2.50")}}); got != numeric {
		t.Fatalf("typed comparison stored as %s, want %s", got, numeric)
	}
	first := storedJSON(t, &pb.MeterFilter{Attribute: "v", Operator: opIn, TypedValues: []*pb.MeterFilterValue{typed(kindBool, "true"), typed(kindNumber, "5.0"), typed(kindString, "5")}})
	second := storedJSON(t, &pb.MeterFilter{Attribute: "v", Operator: opIn, TypedValues: []*pb.MeterFilterValue{typed(kindString, "5"), typed(kindNumber, "5"), typed(kindBool, "true")}})
	if first != second || first != `[{"attribute":"v","operator":3,"typed_values":[{"kind":1,"value":"5"},{"kind":2,"value":"5"},{"kind":3,"value":"true"}]}]` {
		t.Fatalf("typed membership is not canonical: %s %s", first, second)
	}
}

func TestCanonicalFiltersRejectInvalidOperands(t *testing.T) {
	for name, filter := range map[string]*pb.MeterFilter{
		"both forms":        {Attribute: "a", Operator: opEqual, Values: []string{"x"}, TypedValues: []*pb.MeterFilterValue{typed(kindString, "x")}},
		"unknown kind":      {Attribute: "a", Operator: opEqual, TypedValues: []*pb.MeterFilterValue{{Value: "x"}}},
		"bad number":        {Attribute: "a", Operator: opEqual, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "1e3")}},
		"bad boolean":       {Attribute: "a", Operator: opEqual, TypedValues: []*pb.MeterFilterValue{typed(kindBool, "yes")}},
		"string comparison": {Attribute: "a", Operator: opGreater, TypedValues: []*pb.MeterFilterValue{typed(kindString, "high")}},
		"exists string":     {Attribute: "a", Operator: opExists, Values: []string{"present"}},
		"exists number":     {Attribute: "a", Operator: opExists, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "1")}},
		"two equal values":  {Attribute: "a", Operator: opEqual, Values: []string{"x", "y"}},
		"empty membership":  {Attribute: "a", Operator: opIn},
	} {
		if _, err := CanonicalFilters([]*pb.MeterFilter{filter}); err == nil {
			t.Fatalf("%s accepted", name)
		}
	}
}

func TestTypedFiltersMatchNumberAndBooleanAttributes(t *testing.T) {
	define := func(filter *pb.MeterFilter) metering.Definition {
		t.Helper()
		canonical, err := CanonicalFilters([]*pb.MeterFilter{filter})
		if err != nil {
			t.Fatal(err)
		}
		definition, err := CatalogDefinition("m", "e", pb.MeterAggregation_METER_AGGREGATION_COUNT, "", canonical, nil)
		if err != nil {
			t.Fatal(err)
		}
		return definition
	}
	matches := func(definition metering.Definition, value metering.Value) bool {
		_, err := metering.Match([]metering.Definition{definition}, metering.Event{Type: "e", Attributes: map[string]metering.Value{"v": value}})
		return err == nil
	}
	five, text, yes, no := number(t, "5"), metering.String("5"), metering.Boolean(true), metering.Boolean(false)
	for _, c := range []struct {
		name   string
		filter *pb.MeterFilter
		value  metering.Value
		want   bool
	}{
		{"number equals number", &pb.MeterFilter{Attribute: "v", Operator: opEqual, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "5.00")}}, five, true},
		{"number differs from text", &pb.MeterFilter{Attribute: "v", Operator: opEqual, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "5")}}, text, false},
		{"untyped text differs from number", &pb.MeterFilter{Attribute: "v", Operator: opEqual, Values: []string{"5"}}, five, false},
		{"number not equal excludes number", &pb.MeterFilter{Attribute: "v", Operator: opNotEqual, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "5")}}, five, false},
		{"number not equal keeps text", &pb.MeterFilter{Attribute: "v", Operator: opNotEqual, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "5")}}, text, true},
		{"boolean equals boolean", &pb.MeterFilter{Attribute: "v", Operator: opEqual, TypedValues: []*pb.MeterFilterValue{typed(kindBool, "true")}}, yes, true},
		{"boolean not equal excludes boolean", &pb.MeterFilter{Attribute: "v", Operator: opNotEqual, TypedValues: []*pb.MeterFilterValue{typed(kindBool, "true")}}, yes, false},
		{"boolean not equal keeps other boolean", &pb.MeterFilter{Attribute: "v", Operator: opNotEqual, TypedValues: []*pb.MeterFilterValue{typed(kindBool, "true")}}, no, true},
		{"membership number", &pb.MeterFilter{Attribute: "v", Operator: opIn, TypedValues: []*pb.MeterFilterValue{typed(kindNumber, "4"), typed(kindNumber, "5")}}, five, true},
		{"membership boolean", &pb.MeterFilter{Attribute: "v", Operator: opIn, TypedValues: []*pb.MeterFilterValue{typed(kindString, "x"), typed(kindBool, "false")}}, no, true},
		{"membership type mismatch", &pb.MeterFilter{Attribute: "v", Operator: opIn, TypedValues: []*pb.MeterFilterValue{typed(kindString, "true"), typed(kindNumber, "1")}}, yes, false},
		{"exists without operand", &pb.MeterFilter{Attribute: "v", Operator: opExists}, no, true},
	} {
		if got := matches(define(c.filter), c.value); got != c.want {
			t.Fatalf("%s: match = %v, want %v", c.name, got, c.want)
		}
	}
	absent := define(&pb.MeterFilter{Attribute: "v", Operator: opExists})
	if _, err := metering.Match([]metering.Definition{absent}, metering.Event{Type: "e", Attributes: map[string]metering.Value{"other": text}}); err == nil {
		t.Fatal("exists without operand matched a missing attribute")
	}
}
