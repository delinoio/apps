// SPDX-License-Identifier: FSL-1.1-ALv2

// Package events durably accepts metering events and corrections, enforces
// admission bounds, and materializes group aggregates asynchronously. Callers
// own the PostgreSQL transaction; every function leaves no partial effect when
// it returns an error and the caller rolls back.
package events

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"time"
	"unicode/utf8"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"github.com/jackc/pgx/v5"
)

// Reason classifies a rejected request so the API can return a typed error.
type Reason int

const (
	ReasonInvalidInput Reason = iota + 1
	ReasonInvalidAttribute
	ReasonNoMatchingMeter
	ReasonCardinalityExceeded
	ReasonIdentityConflict
	ReasonStaleRevision
	ReasonNotFound
	ReasonCancelled
	ReasonWindowClosed
	ReasonForbidden
	// ReasonPeriodUnavailable rejects paid usage whose Paddle renewal period
	// is not recorded yet; the caller retries after the period arrives.
	ReasonPeriodUnavailable
)

// Error is a request rejection. Message never contains attribute values,
// credentials, or tenant identifiers.
type Error struct {
	Reason  Reason
	Message string
}

func (e *Error) Error() string { return e.Message }

func reject(reason Reason, message string) error { return &Error{Reason: reason, Message: message} }

// ErrPermanent marks processing failures that retries cannot fix, such as
// inconsistent stored state. Processing hooks may wrap it.
var ErrPermanent = errors.New("permanent metering processing failure")

// Bounds for one flat event. Group keys and stored history derive from these values.
const (
	MaxKeyBytes            = 200
	MaxTypeBytes           = 100
	MaxAttributes          = 100
	MaxAttributeNameBytes  = 100
	MaxAttributeValueBytes = 1024
	MaxNumberBytes         = 64
	// MaxGroupAttributes bounds the group-by fields of one meter definition.
	MaxGroupAttributes = 20
)

// GroupDigest returns the SHA-256 digest of a canonical group key. Admission
// and aggregate rows are keyed by this bounded identity because a group key of
// several maximum-size attributes exceeds the PostgreSQL B-tree entry limit.
func GroupDigest(groupKey string) []byte {
	sum := sha256.Sum256([]byte(groupKey))
	return sum[:]
}

// JobKind is the durable job kind that materializes an event's latest revision.
const JobKind = "metering"

// Policy holds the operator settings that bound acceptance.
type Policy struct {
	Lateness       time.Duration
	ReportingGrace time.Duration
	Limits         metering.Limits
}

// Input is an original event reported by a registered service.
type Input struct {
	TenantID   string
	ServiceID  string
	Key        string
	Type       string
	OccurredAt time.Time
	Attributes map[string]metering.Value
}

// Correction replaces or cancels the latest revision of an event.
type Correction struct {
	TenantID         string
	ServiceID        string
	Key              string
	ExpectedRevision int32
	Kind             metering.RevisionKind
	Type             string
	Attributes       map[string]metering.Value
}

// Accepted identifies the durable revision created or replayed by a request.
type Accepted struct {
	EventID  string
	Revision int32
	Replayed bool
	Meters   int
}

// Snapshot is the latest accepted state of an event.
type Snapshot struct {
	ID              string
	TenantID        string
	ServiceID       string
	Key             string
	Type            string
	OccurredAt      time.Time
	Attributes      map[string]metering.Value
	Revision        int32
	AppliedRevision int32
	Cancelled       bool
}

// ValidateContent checks the flat event shape before meter matching.
func ValidateContent(eventType string, attributes map[string]metering.Value) error {
	if eventType == "" || len(eventType) > MaxTypeBytes || !utf8.ValidString(eventType) {
		return reject(ReasonInvalidInput, fmt.Sprintf("event type must contain 1 to %d bytes", MaxTypeBytes))
	}
	if len(attributes) > MaxAttributes {
		return reject(ReasonInvalidAttribute, fmt.Sprintf("an event has at most %d attributes", MaxAttributes))
	}
	for name, value := range attributes {
		if name == "" || len(name) > MaxAttributeNameBytes || !utf8.ValidString(name) {
			return reject(ReasonInvalidAttribute, fmt.Sprintf("attribute names must contain 1 to %d bytes", MaxAttributeNameBytes))
		}
		switch value.Kind {
		case metering.ValueString:
			if len(value.Text) > MaxAttributeValueBytes || !utf8.ValidString(value.Text) {
				return reject(ReasonInvalidAttribute, fmt.Sprintf("string attributes contain at most %d bytes", MaxAttributeValueBytes))
			}
		case metering.ValueBool:
			if value.Text != "true" && value.Text != "false" {
				return reject(ReasonInvalidAttribute, "invalid boolean attribute")
			}
		case metering.ValueNumber:
			if len(value.Text) > MaxNumberBytes {
				return reject(ReasonInvalidAttribute, fmt.Sprintf("numeric attributes contain at most %d characters", MaxNumberBytes))
			}
			if canonical, err := metering.Number(value.Text); err != nil || canonical.Text != value.Text {
				return reject(ReasonInvalidAttribute, "numeric attributes must be canonical decimals")
			}
		default:
			return reject(ReasonInvalidAttribute, "attributes must be flat strings, booleans, or decimal numbers")
		}
	}
	return nil
}

type storedValue struct {
	Kind  metering.ValueKind `json:"kind"`
	Value string             `json:"value"`
}

// EncodeValues stores a flat map with explicit scalar kinds.
func EncodeValues(values map[string]metering.Value) ([]byte, error) {
	stored := make(map[string]storedValue, len(values))
	for name, value := range values {
		stored[name] = storedValue{Kind: value.Kind, Value: value.Text}
	}
	return json.Marshal(stored)
}

// DecodeValues reverses EncodeValues and rejects unknown scalar kinds.
func DecodeValues(raw []byte) (map[string]metering.Value, error) {
	var stored map[string]storedValue
	if err := json.Unmarshal(raw, &stored); err != nil {
		return nil, fmt.Errorf("%w: stored values: %v", ErrPermanent, err)
	}
	values := make(map[string]metering.Value, len(stored))
	for name, value := range stored {
		switch value.Kind {
		case metering.ValueString, metering.ValueNumber, metering.ValueBool:
		default:
			return nil, fmt.Errorf("%w: stored value kind", ErrPermanent)
		}
		values[name] = metering.Value{Kind: value.Kind, Text: value.Value}
	}
	return values, nil
}

// fingerprint hashes canonical content so equal decimals and attribute order
// replay identically while any content change conflicts.
func fingerprint(parts ...any) string {
	raw, _ := json.Marshal(parts)
	sum := sha256.Sum256(raw)
	return hex.EncodeToString(sum[:])
}

func canonicalAttributes(values map[string]metering.Value) [][3]string {
	out := make([][3]string, 0, len(values))
	for name, value := range values {
		out = append(out, [3]string{name, string(value.Kind), value.Text})
	}
	sort.Slice(out, func(i, j int) bool { return out[i][0] < out[j][0] })
	return out
}

func eventFingerprint(in Input) string {
	return fingerprint("event", in.TenantID, in.ServiceID, in.Key, in.Type, in.OccurredAt.UTC().Format(time.RFC3339Nano), canonicalAttributes(in.Attributes))
}

func correctionFingerprint(eventID string, c Correction) string {
	return fingerprint("correction", eventID, c.ExpectedRevision, string(c.Kind), c.Type, canonicalAttributes(c.Attributes))
}

var aggregations = map[pb.MeterAggregation]metering.Aggregation{
	pb.MeterAggregation_METER_AGGREGATION_COUNT:        metering.Count,
	pb.MeterAggregation_METER_AGGREGATION_SUM:          metering.Sum,
	pb.MeterAggregation_METER_AGGREGATION_MIN:          metering.Min,
	pb.MeterAggregation_METER_AGGREGATION_MAX:          metering.Max,
	pb.MeterAggregation_METER_AGGREGATION_AVG:          metering.Avg,
	pb.MeterAggregation_METER_AGGREGATION_UNIQUE_COUNT: metering.UniqueCount,
}

// CatalogDefinition converts catalog meter fields to a validated metering
// definition. Equality and membership filters compare string scalars, numeric
// comparisons use one decimal, and existence takes one boolean.
func CatalogDefinition(key, eventType string, aggregation pb.MeterAggregation, valueAttribute string, filters []*pb.MeterFilter, groupBy []string) (metering.Definition, error) {
	kind, ok := aggregations[aggregation]
	if !ok {
		return metering.Definition{}, errors.New("unknown meter aggregation")
	}
	definition := metering.Definition{Key: key, EventType: eventType, Aggregation: kind, ValueAttribute: valueAttribute, GroupBy: append([]string(nil), groupBy...)}
	for _, filter := range filters {
		if filter == nil {
			return metering.Definition{}, errors.New("invalid meter filter")
		}
		item := metering.Filter{Attribute: filter.Attribute}
		switch filter.Operator {
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL:
			if len(filter.Values) != 1 {
				return metering.Definition{}, errors.New("equal meter filters require one value")
			}
			item.Operator, item.Value = metering.Equal, metering.String(filter.Values[0])
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_NOT_EQUAL:
			if len(filter.Values) != 1 {
				return metering.Definition{}, errors.New("not-equal meter filters require one value")
			}
			item.Operator, item.Value = metering.NotEqual, metering.String(filter.Values[0])
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN:
			if len(filter.Values) == 0 {
				return metering.Definition{}, errors.New("in meter filters require values")
			}
			item.Operator = metering.In
			item.Values = make([]metering.Value, 0, len(filter.Values))
			for _, value := range filter.Values {
				item.Values = append(item.Values, metering.String(value))
			}
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER,
			pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER_OR_EQUAL,
			pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS,
			pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS_OR_EQUAL:
			if len(filter.Values) != 1 {
				return metering.Definition{}, errors.New("numeric meter filters require one value")
			}
			value, err := metering.Number(filter.Values[0])
			if err != nil {
				return metering.Definition{}, errors.New("numeric meter filter requires a decimal value")
			}
			item.Value = value
			switch filter.Operator {
			case pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER:
				item.Operator = metering.GreaterThan
			case pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER_OR_EQUAL:
				item.Operator = metering.GreaterThanOrEqual
			case pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS:
				item.Operator = metering.LessThan
			default:
				item.Operator = metering.LessThanOrEqual
			}
		case pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS:
			if len(filter.Values) != 1 || (filter.Values[0] != "true" && filter.Values[0] != "false") {
				return metering.Definition{}, errors.New("exists meter filters require one boolean value")
			}
			item.Operator = metering.Exists
			item.Value = metering.Boolean(filter.Values[0] == "true")
		default:
			return metering.Definition{}, errors.New("unknown meter filter operator")
		}
		definition.Filters = append(definition.Filters, item)
	}
	if err := definition.Validate(); err != nil {
		return metering.Definition{}, err
	}
	return definition, nil
}

// storedMeter is the immutable meter definition recorded in catalog_versions.
type storedMeter struct {
	Name           string              `json:"name"`
	EventType      string              `json:"event_type"`
	Aggregation    pb.MeterAggregation `json:"aggregation"`
	ValueAttribute string              `json:"value_attribute"`
	Filters        []*pb.MeterFilter   `json:"filters"`
	GroupBy        []string            `json:"group_by"`
}

// CompatibilityAttribute is the value attribute of CompatibilityDefinition.
const CompatibilityAttribute = "quantity"

// CompatibilityAggregation is the wire aggregation of CompatibilityDefinition,
// which queries report for a meter without a typed definition.
const CompatibilityAggregation = pb.MeterAggregation_METER_AGGREGATION_SUM

// CompatibilityDefinition is the catalog compatibility rule for a meter that
// was registered without a typed definition, such as an operator PutMeter call
// that names only a service and a key: the event type equals the meter key and
// the meter sums the numeric "quantity" attribute without filters or group-by
// fields. It is the effective, immutable definition of every untyped meter, so
// a former quantity report of N units on meter M becomes an event of type M
// with quantity N.
func CompatibilityDefinition(key string) metering.Definition {
	return metering.Definition{Key: key, EventType: key, Aggregation: metering.Sum, ValueAttribute: CompatibilityAttribute}
}

// IsCompatibilityDefinition reports whether a typed definition equals the
// compatibility definition of its key.
func IsCompatibilityDefinition(d metering.Definition) bool {
	c := CompatibilityDefinition(d.Key)
	return d.EventType == c.EventType && d.Aggregation == c.Aggregation && d.ValueAttribute == c.ValueAttribute && len(d.Filters) == 0 && len(d.GroupBy) == 0
}

// UntypedMeterInUse reports whether events were evaluated against an untyped
// meter under its compatibility definition. Such a meter can later be declared
// only with that same definition, which keeps the definition immutable for its
// key.
func UntypedMeterInUse(ctx context.Context, tx pgx.Tx, meterID string) (bool, error) {
	var used bool
	err := tx.QueryRow(ctx, `SELECT EXISTS(SELECT 1 FROM metering_events WHERE $1::uuid=ANY(meter_ids))`, meterID).Scan(&used)
	return used, err
}

// decodeMeter reads a stored catalog meter definition. A meter without a
// typed definition uses CompatibilityDefinition.
func decodeMeter(key string, raw []byte) (metering.Definition, error) {
	var stored storedMeter
	if len(raw) > 0 {
		if err := json.Unmarshal(raw, &stored); err != nil {
			return metering.Definition{}, err
		}
	}
	if stored.EventType == "" {
		definition := CompatibilityDefinition(key)
		return definition, definition.Validate()
	}
	return CatalogDefinition(stored.Name, stored.EventType, stored.Aggregation, stored.ValueAttribute, stored.Filters, stored.GroupBy)
}
