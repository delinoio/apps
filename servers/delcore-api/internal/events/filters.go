// SPDX-License-Identifier: FSL-1.1-ALv2

package events

import (
	"encoding/json"
	"errors"
	"sort"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

var filterOperators = map[pb.MeterFilterOperator]metering.FilterOperator{
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL:            metering.Equal,
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_NOT_EQUAL:        metering.NotEqual,
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN:               metering.In,
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER:          metering.GreaterThan,
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER_OR_EQUAL: metering.GreaterThanOrEqual,
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS:             metering.LessThan,
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS_OR_EQUAL:    metering.LessThanOrEqual,
	pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS:           metering.Exists,
}

// filterOperands returns the operands of a filter with their scalar types.
// Untyped values are strings for equality and membership, one decimal for
// numeric comparisons, and one boolean for existence.
func filterOperands(filter *pb.MeterFilter, operator metering.FilterOperator) ([]metering.Value, error) {
	if len(filter.Values) > 0 && len(filter.TypedValues) > 0 {
		return nil, errors.New("a meter filter uses values or typed_values, not both")
	}
	out := make([]metering.Value, 0, len(filter.Values)+len(filter.TypedValues))
	for _, typed := range filter.TypedValues {
		if typed == nil {
			return nil, errors.New("invalid typed meter filter value")
		}
		switch typed.Kind {
		case pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_STRING:
			out = append(out, metering.String(typed.Value))
		case pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_NUMBER:
			value, err := metering.Number(typed.Value)
			if err != nil {
				return nil, errors.New("number filter values must be decimal strings")
			}
			out = append(out, value)
		case pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_BOOLEAN:
			if typed.Value != "true" && typed.Value != "false" {
				return nil, errors.New("boolean filter values must be true or false")
			}
			out = append(out, metering.Boolean(typed.Value == "true"))
		default:
			return nil, errors.New("unknown meter filter value kind")
		}
	}
	for _, text := range filter.Values {
		switch operator {
		case metering.Equal, metering.NotEqual, metering.In:
			out = append(out, metering.String(text))
		case metering.Exists:
			if text != "true" && text != "false" {
				return nil, errors.New("exists meter filters take no value or one boolean value")
			}
			out = append(out, metering.Boolean(text == "true"))
		default:
			value, err := metering.Number(text)
			if err != nil {
				return nil, errors.New("numeric meter filter requires a decimal value")
			}
			out = append(out, value)
		}
	}
	return out, nil
}

// domainFilter converts one protocol filter to a metering filter.
func domainFilter(filter *pb.MeterFilter) (metering.Filter, error) {
	if filter == nil {
		return metering.Filter{}, errors.New("invalid meter filter")
	}
	operator, ok := filterOperators[filter.Operator]
	if !ok {
		return metering.Filter{}, errors.New("unknown meter filter operator")
	}
	operands, err := filterOperands(filter, operator)
	if err != nil {
		return metering.Filter{}, err
	}
	item := metering.Filter{Attribute: filter.Attribute, Operator: operator}
	switch operator {
	case metering.Equal, metering.NotEqual:
		if len(operands) != 1 {
			return metering.Filter{}, errors.New("equality meter filters require one value")
		}
		item.Value = operands[0]
	case metering.In:
		if len(operands) == 0 {
			return metering.Filter{}, errors.New("in meter filters require values")
		}
		item.Values = operands
	case metering.Exists:
		switch {
		case len(operands) == 0:
			// No operand means the attribute must be present.
			item.Value = metering.Boolean(true)
		case len(operands) == 1 && operands[0].Kind == metering.ValueBool:
			item.Value = operands[0]
		default:
			return metering.Filter{}, errors.New("exists meter filters take no value or one boolean value")
		}
	default:
		if len(operands) != 1 || operands[0].Kind != metering.ValueNumber {
			return metering.Filter{}, errors.New("numeric meter filters require one number value")
		}
		item.Value = operands[0]
	}
	return item, nil
}

// CanonicalFilters validates protocol filters and returns the form stored in
// a meter's catalog version, sorted independently of declaration order.
//
// Every definition that was expressible before typed operands keeps its
// earlier stored form, so semantic hashes and version reuse stay stable:
// string-only equality and membership operands, numeric comparison values,
// and exists operands stay in values (membership sorted), and untyped values
// keep their submitted text. An exists filter without an operand is stored as
// the equivalent untyped "true". Typed operands that are all strings fold into
// values. Only equality and membership filters with number or boolean
// operands use typed_values, with canonical decimals and membership sorted
// by kind and value.
func CanonicalFilters(filters []*pb.MeterFilter) ([]*pb.MeterFilter, error) {
	out := make([]*pb.MeterFilter, 0, len(filters))
	for _, filter := range filters {
		item, err := domainFilter(filter)
		if err != nil {
			return nil, err
		}
		stored := &pb.MeterFilter{Attribute: filter.Attribute, Operator: filter.Operator}
		switch item.Operator {
		case metering.Equal, metering.NotEqual, metering.In:
			operands := item.Values
			if item.Operator != metering.In {
				operands = []metering.Value{item.Value}
			}
			stringsOnly := true
			for _, operand := range operands {
				stringsOnly = stringsOnly && operand.Kind == metering.ValueString
			}
			if stringsOnly {
				for _, operand := range operands {
					stored.Values = append(stored.Values, operand.Text)
				}
				if item.Operator == metering.In {
					sort.Strings(stored.Values)
				}
				break
			}
			for _, operand := range operands {
				stored.TypedValues = append(stored.TypedValues, &pb.MeterFilterValue{Kind: filterValueKinds[operand.Kind], Value: operand.Text})
			}
			if item.Operator == metering.In {
				sort.Slice(stored.TypedValues, func(i, j int) bool {
					a, b := stored.TypedValues[i], stored.TypedValues[j]
					return a.Kind < b.Kind || a.Kind == b.Kind && a.Value < b.Value
				})
			}
		case metering.Exists:
			stored.Values = []string{item.Value.Text}
		default:
			// Numeric comparisons store the canonical decimal, so equivalent
			// spellings such as 1.0 and 1.00 are one immutable definition.
			stored.Values = []string{item.Value.Text}
		}
		out = append(out, stored)
	}
	sort.Slice(out, func(i, j int) bool {
		a, _ := json.Marshal(out[i])
		b, _ := json.Marshal(out[j])
		return string(a) < string(b)
	})
	return out, nil
}

var filterValueKinds = map[metering.ValueKind]pb.MeterFilterValueKind{
	metering.ValueString: pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_STRING,
	metering.ValueNumber: pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_NUMBER,
	metering.ValueBool:   pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_BOOLEAN,
}
