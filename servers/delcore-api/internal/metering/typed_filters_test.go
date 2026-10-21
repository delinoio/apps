// SPDX-License-Identifier: FSL-1.1-ALv2

package metering

import "testing"

// Equality and membership compare the scalar type and canonical value, so a
// number or boolean attribute never equals a string operand with the same text.
func TestTypedEqualityAndMembershipFilters(t *testing.T) {
	five := number(t, "5")
	cases := []struct {
		name   string
		filter Filter
		value  Value
		want   bool
	}{
		{"number equal", Filter{Attribute: "v", Operator: Equal, Value: number(t, "5.0")}, five, true},
		{"number equal other number", Filter{Attribute: "v", Operator: Equal, Value: number(t, "6")}, five, false},
		{"string operand never equals number", Filter{Attribute: "v", Operator: Equal, Value: String("5")}, five, false},
		{"number not equal same number", Filter{Attribute: "v", Operator: NotEqual, Value: five}, five, false},
		{"number not equal string", Filter{Attribute: "v", Operator: NotEqual, Value: five}, String("5"), true},
		{"boolean equal", Filter{Attribute: "v", Operator: Equal, Value: Boolean(false)}, Boolean(false), true},
		{"boolean not equal", Filter{Attribute: "v", Operator: NotEqual, Value: Boolean(false)}, Boolean(true), true},
		{"boolean differs from text", Filter{Attribute: "v", Operator: Equal, Value: Boolean(true)}, String("true"), false},
		{"membership mixed types", Filter{Attribute: "v", Operator: In, Values: []Value{String("x"), number(t, "5.00"), Boolean(true)}}, five, true},
		{"membership boolean", Filter{Attribute: "v", Operator: In, Values: []Value{String("x"), Boolean(true)}}, Boolean(true), true},
		{"membership miss", Filter{Attribute: "v", Operator: In, Values: []Value{String("5"), Boolean(true)}}, five, false},
	}
	for _, c := range cases {
		definition := Definition{Key: "m", EventType: "e", Aggregation: Count, Filters: []Filter{c.filter}}
		_, err := Match([]Definition{definition}, Event{Type: "e", Attributes: map[string]Value{"v": c.value}})
		if (err == nil) != c.want {
			t.Fatalf("%s: matched = %v, want %v (%v)", c.name, err == nil, c.want, err)
		}
	}
}
