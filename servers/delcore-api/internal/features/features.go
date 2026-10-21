// SPDX-License-Identifier: FSL-1.1-ALv2

// Package features decodes pinned catalog feature grants and evaluates feature
// authorization decisions. It holds no database state: callers read the grant
// selected for a tenant and the last applied group aggregate, and this package
// turns them into a typed decision.
package features

import (
	"encoding/json"
	"errors"
	"fmt"
	"unicode/utf8"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

var (
	// ErrGrant marks a stored grant that does not match its feature definition.
	ErrGrant = errors.New("invalid stored feature grant")
	// ErrGroup marks group values that do not select a group of the meter.
	ErrGroup = errors.New("invalid feature group values")
)

// Grant is one decoded feature value of a pinned plan or add-on version.
type Grant struct {
	Kind pb.FeatureKind
	// Enabled is the access switch of an access feature.
	Enabled bool
	// Limit is the per-group limit of a metered feature.
	Limit accounting.Decimal
	// Configuration is the typed value of a configuration feature.
	Configuration *pb.ConfigurationValue
}

// DecodeGrant reads a catalog_version_features value with the shape written by
// catalog mutations and validates it against the immutable feature definition.
func DecodeGrant(kind pb.FeatureKind, valueType pb.ConfigurationValueType, raw []byte) (Grant, error) {
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(raw, &fields); err != nil || len(fields) != 1 {
		return Grant{}, fmt.Errorf("%w: grant must contain exactly one value", ErrGrant)
	}
	g := Grant{Kind: kind}
	switch kind {
	case pb.FeatureKind_FEATURE_KIND_ACCESS:
		value, ok := fields["enabled"]
		if !ok || json.Unmarshal(value, &g.Enabled) != nil {
			return Grant{}, fmt.Errorf("%w: access grant needs a boolean", ErrGrant)
		}
	case pb.FeatureKind_FEATURE_KIND_METERED:
		var text string
		value, ok := fields["limit"]
		if !ok || json.Unmarshal(value, &text) != nil {
			return Grant{}, fmt.Errorf("%w: metered grant needs a decimal limit", ErrGrant)
		}
		limit, err := accounting.Parse(text)
		if err != nil || limit.Cmp(accounting.Zero()) < 0 {
			return Grant{}, fmt.Errorf("%w: metered grant needs a nonnegative decimal limit", ErrGrant)
		}
		g.Limit = limit
	case pb.FeatureKind_FEATURE_KIND_CONFIGURATION:
		configuration, err := decodeConfiguration(valueType, fields)
		if err != nil {
			return Grant{}, err
		}
		g.Configuration = configuration
	default:
		return Grant{}, fmt.Errorf("%w: unknown feature kind", ErrGrant)
	}
	return g, nil
}

func decodeConfiguration(valueType pb.ConfigurationValueType, fields map[string]json.RawMessage) (*pb.ConfigurationValue, error) {
	mismatch := fmt.Errorf("%w: configuration value does not match its declared type", ErrGrant)
	switch valueType {
	case pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_BOOLEAN:
		var v bool
		if raw, ok := fields["boolean"]; !ok || json.Unmarshal(raw, &v) != nil {
			return nil, mismatch
		}
		return &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Boolean{Boolean: v}}, nil
	case pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_INTEGER:
		var v int64
		if raw, ok := fields["integer"]; !ok || json.Unmarshal(raw, &v) != nil {
			return nil, mismatch
		}
		return &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Integer{Integer: v}}, nil
	case pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_DECIMAL:
		var v string
		if raw, ok := fields["decimal"]; !ok || json.Unmarshal(raw, &v) != nil {
			return nil, mismatch
		}
		d, err := accounting.Parse(v)
		if err != nil {
			return nil, mismatch
		}
		return &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Decimal{Decimal: d.String()}}, nil
	case pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING:
		var v string
		if raw, ok := fields["text"]; !ok || json.Unmarshal(raw, &v) != nil {
			return nil, mismatch
		}
		return &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Text{Text: v}}, nil
	case pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING_LIST:
		var v []string
		if raw, ok := fields["string_list"]; !ok || json.Unmarshal(raw, &v) != nil {
			return nil, mismatch
		}
		if v == nil {
			v = []string{}
		}
		return &pb.ConfigurationValue{Value: &pb.ConfigurationValue_StringList{StringList: &pb.StringList{Values: v}}}, nil
	default:
		return nil, fmt.Errorf("%w: unknown configuration type", ErrGrant)
	}
}

// MeterDefinition is the part of an immutable stored meter version that
// feature authorization and aggregate queries read.
type MeterDefinition struct {
	EventType   string              `json:"event_type"`
	Aggregation pb.MeterAggregation `json:"aggregation"`
	GroupBy     []string            `json:"group_by"`
}

// DecodeMeter reads a meter catalog version. A legacy meter has no version;
// nil input returns an empty definition without group-by attributes, and such
// a meter never receives events.
func DecodeMeter(raw []byte) (MeterDefinition, error) {
	var d MeterDefinition
	if len(raw) == 0 {
		return d, nil
	}
	if err := json.Unmarshal(raw, &d); err != nil {
		return MeterDefinition{}, fmt.Errorf("stored meter definition: %w", err)
	}
	return d, nil
}

// GroupKey validates authorization group values against the meter's group-by
// attributes and returns the same key that ingestion stores. The values must
// name exactly the group-by attributes; scalar types are part of the key, so a
// value must use the type the service reports at ingestion.
func GroupKey(groupBy []string, values map[string]metering.Value) (string, error) {
	if len(values) != len(groupBy) {
		return "", fmt.Errorf("%w: supply exactly the meter group-by attributes", ErrGroup)
	}
	for _, name := range groupBy {
		value, ok := values[name]
		if !ok {
			return "", fmt.Errorf("%w: supply exactly the meter group-by attributes", ErrGroup)
		}
		switch value.Kind {
		case metering.ValueString:
			if len(value.Text) > events.MaxAttributeValueBytes || !utf8.ValidString(value.Text) {
				return "", fmt.Errorf("%w: string group values contain at most %d bytes", ErrGroup, events.MaxAttributeValueBytes)
			}
		case metering.ValueBool:
			if value.Text != "true" && value.Text != "false" {
				return "", fmt.Errorf("%w: invalid boolean group value", ErrGroup)
			}
		case metering.ValueNumber:
			canonical, err := metering.Number(value.Text)
			if err != nil || canonical.Text != value.Text || len(value.Text) > events.MaxNumberBytes {
				return "", fmt.Errorf("%w: numeric group values must be canonical decimals", ErrGroup)
			}
		default:
			return "", fmt.Errorf("%w: group values must be flat scalars", ErrGroup)
		}
	}
	key, err := metering.GroupKey(values)
	if err != nil {
		return "", fmt.Errorf("%w: %v", ErrGroup, err)
	}
	return key, nil
}

// Decision is the feature part of one authorization response.
type Decision struct {
	Entitled bool
	// Reason is BLOCK_REASON_UNSPECIFIED when the feature permits execution.
	Reason pb.BlockReason
}

// Evaluate decides access from the grants pinned for the tenant at the
// request time. grants holds every pinned grant of the feature; more than one
// grant is a conflicting selection and fails closed. applied is the last
// applied aggregate of the selected group in the current billing period and is
// used only for metered features. Execution is blocked once the applied value
// reaches the limit; accepted but unapplied usage is never estimated, so
// concurrent or lagging execution can exceed the limit.
func Evaluate(grants []Grant, applied accounting.Decimal) Decision {
	if len(grants) != 1 {
		return Decision{Reason: pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED}
	}
	g := grants[0]
	switch g.Kind {
	case pb.FeatureKind_FEATURE_KIND_ACCESS:
		if !g.Enabled {
			return Decision{Reason: pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED}
		}
		return Decision{Entitled: true}
	case pb.FeatureKind_FEATURE_KIND_METERED:
		if applied.Cmp(g.Limit) >= 0 {
			return Decision{Entitled: true, Reason: pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED}
		}
		return Decision{Entitled: true}
	case pb.FeatureKind_FEATURE_KIND_CONFIGURATION:
		return Decision{Entitled: true}
	default:
		return Decision{Reason: pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED}
	}
}
