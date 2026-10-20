// SPDX-License-Identifier: FSL-1.1-ALv2

// Package metering matches flat events to immutable meter definitions and computes exact aggregates.
package metering

import (
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strconv"

	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
)

var (
	ErrDefinition = errors.New("invalid meter definition")
	ErrEvent      = errors.New("invalid meter event")
	ErrNoMatch    = errors.New("no meter matches event")
	ErrRevision   = errors.New("invalid meter revision")
	ErrLimit      = errors.New("meter cardinality limit exceeded")
)

type ValueKind string

const (
	ValueString ValueKind = "string"
	ValueNumber ValueKind = "number"
	ValueBool   ValueKind = "boolean"
)

// Value stores a scalar in canonical text form. Use the constructors or ParseValue.
type Value struct {
	Kind ValueKind
	Text string
}

func String(s string) Value { return Value{Kind: ValueString, Text: s} }
func Boolean(v bool) Value  { return Value{Kind: ValueBool, Text: strconv.FormatBool(v)} }
func Number(s string) (Value, error) {
	d, err := accounting.Parse(s)
	if err != nil {
		return Value{}, fmt.Errorf("%w: number: %v", ErrEvent, err)
	}
	return Value{Kind: ValueNumber, Text: d.String()}, nil
}

// ParseValue rejects nested values and binary floating point numbers, which can lose decimal precision.
func ParseValue(v any) (Value, error) {
	switch x := v.(type) {
	case string:
		return String(x), nil
	case bool:
		return Boolean(x), nil
	case json.Number:
		return Number(string(x))
	case int:
		return Number(strconv.Itoa(x))
	case int64:
		return Number(strconv.FormatInt(x, 10))
	default:
		return Value{}, fmt.Errorf("%w: attribute must be a string, boolean, or exact number", ErrEvent)
	}
}

func (v Value) canonical() (Value, error) {
	switch v.Kind {
	case ValueString:
		return v, nil
	case ValueNumber:
		return Number(v.Text)
	case ValueBool:
		if v.Text != "true" && v.Text != "false" {
			return Value{}, fmt.Errorf("%w: invalid boolean", ErrEvent)
		}
		return v, nil
	default:
		return Value{}, fmt.Errorf("%w: invalid attribute kind", ErrEvent)
	}
}

func (v Value) decimal() (accounting.Decimal, error) {
	if v.Kind != ValueNumber {
		return accounting.Decimal{}, fmt.Errorf("%w: numeric attribute required", ErrEvent)
	}
	return accounting.Parse(v.Text)
}

type Aggregation string

const (
	Count       Aggregation = "count"
	Sum         Aggregation = "sum"
	Min         Aggregation = "min"
	Max         Aggregation = "max"
	Avg         Aggregation = "avg"
	UniqueCount Aggregation = "unique_count"
)

type FilterOperator string

const (
	Equal              FilterOperator = "equal"
	NotEqual           FilterOperator = "not_equal"
	In                 FilterOperator = "in"
	GreaterThan        FilterOperator = "greater_than"
	GreaterThanOrEqual FilterOperator = "greater_than_or_equal"
	LessThan           FilterOperator = "less_than"
	LessThanOrEqual    FilterOperator = "less_than_or_equal"
	Exists             FilterOperator = "exists"
)

type Filter struct {
	Attribute string
	Operator  FilterOperator
	Value     Value
	Values    []Value
}

type Definition struct {
	Key            string
	EventType      string
	Aggregation    Aggregation
	ValueAttribute string
	GroupBy        []string
	Filters        []Filter
}

func (d Definition) Validate() error {
	if d.Key == "" || d.EventType == "" {
		return fmt.Errorf("%w: key and event type are required", ErrDefinition)
	}
	switch d.Aggregation {
	case Count:
		if d.ValueAttribute != "" {
			return fmt.Errorf("%w: count cannot have a value attribute", ErrDefinition)
		}
	case Sum, Min, Max, Avg, UniqueCount:
		if d.ValueAttribute == "" {
			return fmt.Errorf("%w: value attribute is required", ErrDefinition)
		}
	default:
		return fmt.Errorf("%w: unknown aggregation", ErrDefinition)
	}
	seen := make(map[string]bool, len(d.GroupBy))
	for _, name := range d.GroupBy {
		if name == "" || seen[name] {
			return fmt.Errorf("%w: invalid group attribute", ErrDefinition)
		}
		seen[name] = true
	}
	for _, f := range d.Filters {
		if f.Attribute == "" {
			return fmt.Errorf("%w: filter attribute is required", ErrDefinition)
		}
		switch f.Operator {
		case Equal, NotEqual:
			if len(f.Values) != 0 {
				return fmt.Errorf("%w: scalar filter has a value list", ErrDefinition)
			}
			if _, err := f.Value.canonical(); err != nil {
				return fmt.Errorf("%w: %v", ErrDefinition, err)
			}
		case In:
			if len(f.Values) == 0 || f.Value.Kind != "" {
				return fmt.Errorf("%w: membership filter needs a value list", ErrDefinition)
			}
			for _, v := range f.Values {
				if _, err := v.canonical(); err != nil {
					return fmt.Errorf("%w: %v", ErrDefinition, err)
				}
			}
		case GreaterThan, GreaterThanOrEqual, LessThan, LessThanOrEqual:
			if len(f.Values) != 0 || f.Value.Kind != ValueNumber {
				return fmt.Errorf("%w: numeric filter needs one number", ErrDefinition)
			}
			if _, err := f.Value.canonical(); err != nil {
				return fmt.Errorf("%w: %v", ErrDefinition, err)
			}
		case Exists:
			if len(f.Values) != 0 || f.Value.Kind != ValueBool || (f.Value.Text != "true" && f.Value.Text != "false") {
				return fmt.Errorf("%w: existence filter needs one boolean", ErrDefinition)
			}
		default:
			return fmt.Errorf("%w: unknown filter operator", ErrDefinition)
		}
	}
	return nil
}

type Event struct {
	Type       string
	Attributes map[string]Value
}

func (e Event) canonical() (Event, error) {
	if e.Type == "" {
		return Event{}, fmt.Errorf("%w: event type is required", ErrEvent)
	}
	attrs := make(map[string]Value, len(e.Attributes))
	for name, value := range e.Attributes {
		if name == "" {
			return Event{}, fmt.Errorf("%w: attribute name is required", ErrEvent)
		}
		v, err := value.canonical()
		if err != nil {
			return Event{}, fmt.Errorf("%w: attribute %q: %v", ErrEvent, name, err)
		}
		attrs[name] = v
	}
	return Event{Type: e.Type, Attributes: attrs}, nil
}

type Contribution struct {
	MeterKey    string
	GroupKey    string
	GroupValues map[string]Value
	Value       Value
}

// Match validates every matched meter before returning any contribution.
func Match(definitions []Definition, event Event) ([]Contribution, error) {
	validated, err := validateDefinitions(definitions)
	if err != nil {
		return nil, err
	}
	return matchValidated(validated, event)
}

func validateDefinitions(definitions []Definition) ([]Definition, error) {
	seen := make(map[string]bool, len(definitions))
	out := append([]Definition(nil), definitions...)
	for _, d := range out {
		if err := d.Validate(); err != nil {
			return nil, err
		}
		if seen[d.Key] {
			return nil, fmt.Errorf("%w: duplicate meter key %q", ErrDefinition, d.Key)
		}
		seen[d.Key] = true
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Key < out[j].Key })
	return out, nil
}

func matchValidated(definitions []Definition, event Event) ([]Contribution, error) {
	e, err := event.canonical()
	if err != nil {
		return nil, err
	}
	var matches []Contribution
	for _, d := range definitions {
		if d.EventType != e.Type || !matchesFilters(d.Filters, e.Attributes) {
			continue
		}
		values := make(map[string]Value, len(d.GroupBy))
		for _, name := range d.GroupBy {
			value, ok := e.Attributes[name]
			if !ok {
				return nil, fmt.Errorf("%w: meter %q missing group attribute %q", ErrEvent, d.Key, name)
			}
			values[name] = value
		}
		key, err := GroupKey(values)
		if err != nil {
			return nil, err
		}
		var value Value
		if d.Aggregation != Count {
			var ok bool
			value, ok = e.Attributes[d.ValueAttribute]
			if !ok {
				return nil, fmt.Errorf("%w: meter %q missing value attribute %q", ErrEvent, d.Key, d.ValueAttribute)
			}
			if d.Aggregation != UniqueCount && value.Kind != ValueNumber {
				return nil, fmt.Errorf("%w: meter %q needs a numeric value", ErrEvent, d.Key)
			}
		}
		matches = append(matches, Contribution{MeterKey: d.Key, GroupKey: key, GroupValues: values, Value: value})
	}
	if len(matches) == 0 {
		return nil, ErrNoMatch
	}
	return matches, nil
}

func matchesFilters(filters []Filter, attrs map[string]Value) bool {
	for _, f := range filters {
		value, present := attrs[f.Attribute]
		if f.Operator == Exists {
			if present != (f.Value.Text == "true") {
				return false
			}
			continue
		}
		if !present {
			return false
		}
		switch f.Operator {
		case Equal, NotEqual:
			wanted, _ := f.Value.canonical()
			if (value == wanted) != (f.Operator == Equal) {
				return false
			}
		case In:
			found := false
			for _, member := range f.Values {
				member, _ = member.canonical()
				if value == member {
					found = true
					break
				}
			}
			if !found {
				return false
			}
		case GreaterThan, GreaterThanOrEqual, LessThan, LessThanOrEqual:
			if value.Kind != ValueNumber {
				return false
			}
			left, _ := value.decimal()
			right, _ := f.Value.decimal()
			comparison := left.Cmp(right)
			if (f.Operator == GreaterThan && comparison <= 0) || (f.Operator == GreaterThanOrEqual && comparison < 0) ||
				(f.Operator == LessThan && comparison >= 0) || (f.Operator == LessThanOrEqual && comparison > 0) {
				return false
			}
		}
	}
	return true
}

// GroupKey encodes sorted attribute names, scalar types, and canonical values without delimiter collisions.
func GroupKey(values map[string]Value) (string, error) {
	type part struct {
		Name  string    `json:"name"`
		Kind  ValueKind `json:"kind"`
		Value string    `json:"value"`
	}
	parts := make([]part, 0, len(values))
	for name, value := range values {
		if name == "" {
			return "", fmt.Errorf("%w: group attribute name is required", ErrEvent)
		}
		v, err := value.canonical()
		if err != nil {
			return "", err
		}
		parts = append(parts, part{Name: name, Kind: v.Kind, Value: v.Text})
	}
	sort.Slice(parts, func(i, j int) bool { return parts[i].Name < parts[j].Name })
	encoded, err := json.Marshal(parts)
	if err != nil {
		return "", err
	}
	return string(encoded), nil
}
