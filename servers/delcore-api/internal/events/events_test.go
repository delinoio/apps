// SPDX-License-Identifier: FSL-1.1-ALv2

package events

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"
	"time"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
)

func number(t *testing.T, text string) metering.Value {
	t.Helper()
	v, err := metering.Number(text)
	if err != nil {
		t.Fatal(err)
	}
	return v
}

func reasonOf(err error) Reason {
	var rejection *Error
	if errors.As(err, &rejection) {
		return rejection.Reason
	}
	return 0
}

func TestValidateContentBoundsFlatScalars(t *testing.T) {
	valid := map[string]metering.Value{"region": metering.String("eu"), "n": number(t, "-1.5"), "ok": metering.Boolean(true)}
	if err := ValidateContent("use", valid); err != nil {
		t.Fatal(err)
	}
	if reasonOf(ValidateContent("", valid)) != ReasonInvalidInput || reasonOf(ValidateContent(strings.Repeat("t", MaxTypeBytes+1), valid)) != ReasonInvalidInput {
		t.Fatal("invalid event type accepted")
	}
	tooMany := map[string]metering.Value{}
	for i := 0; i <= MaxAttributes; i++ {
		tooMany[strings.Repeat("a", i+1)] = metering.String("x")
	}
	for name, attributes := range map[string]map[string]metering.Value{
		"too many":      tooMany,
		"empty name":    {"": metering.String("x")},
		"long name":     {strings.Repeat("n", MaxAttributeNameBytes+1): metering.String("x")},
		"long string":   {"s": metering.String(strings.Repeat("v", MaxAttributeValueBytes+1))},
		"bad boolean":   {"b": {Kind: metering.ValueBool, Text: "yes"}},
		"nested kind":   {"o": {Kind: "object", Text: "{}"}},
		"noncanonical":  {"n": {Kind: metering.ValueNumber, Text: "1.0"}},
		"invalid utf-8": {"s": metering.String("\xff")},
	} {
		if reasonOf(ValidateContent("use", attributes)) != ReasonInvalidAttribute {
			t.Fatalf("%s attributes accepted", name)
		}
	}
}

func TestStoredValuesRoundTrip(t *testing.T) {
	values := map[string]metering.Value{"region": metering.String("eu"), "n": number(t, "2.5"), "ok": metering.Boolean(false)}
	raw, err := EncodeValues(values)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := DecodeValues(raw)
	if err != nil || len(decoded) != 3 || decoded["n"] != values["n"] || decoded["ok"] != values["ok"] || decoded["region"] != values["region"] {
		t.Fatalf("round trip changed values: %+v %v", decoded, err)
	}
	if _, err = DecodeValues([]byte(`{"x":{"kind":"object","value":"{}"}}`)); !errors.Is(err, ErrPermanent) {
		t.Fatal("unknown stored kind accepted")
	}
	if empty, err := EncodeValues(nil); err != nil || string(empty) != "{}" {
		t.Fatalf("empty attributes must encode as an empty object: %s %v", empty, err)
	}
}

func TestFingerprintsUseCanonicalContent(t *testing.T) {
	at := time.Date(2026, 1, 2, 3, 4, 5, 6000, time.UTC)
	base := Input{TenantID: "t", ServiceID: "s", Key: "k", Type: "use", OccurredAt: at, Attributes: map[string]metering.Value{"a": number(t, "1.50"), "b": metering.String("x")}}
	same := base
	same.OccurredAt = at.In(time.FixedZone("x", 3600))
	same.Attributes = map[string]metering.Value{"b": metering.String("x"), "a": number(t, "1.5")}
	if eventFingerprint(base) != eventFingerprint(same) {
		t.Fatal("equivalent content changed the fingerprint")
	}
	for _, mutate := range []func(*Input){
		func(in *Input) { in.Type = "other" },
		func(in *Input) { in.TenantID = "u" },
		func(in *Input) { in.OccurredAt = at.Add(time.Microsecond) },
		func(in *Input) {
			in.Attributes = map[string]metering.Value{"a": metering.String("1.5"), "b": metering.String("x")}
		},
	} {
		changed := base
		mutate(&changed)
		if eventFingerprint(changed) == eventFingerprint(base) {
			t.Fatal("changed content kept the fingerprint")
		}
	}
	replace := Correction{ExpectedRevision: 1, Kind: metering.Replace, Type: "use", Attributes: base.Attributes}
	cancel := Correction{ExpectedRevision: 1, Kind: metering.Cancel}
	if correctionFingerprint("e", replace) == correctionFingerprint("e", cancel) || correctionFingerprint("e", replace) == correctionFingerprint("f", replace) {
		t.Fatal("distinct corrections shared a fingerprint")
	}
}

func TestStoredCatalogMeterDefinitions(t *testing.T) {
	stored := storedMeter{Name: "requests", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_UNIQUE_COUNT, ValueAttribute: "user",
		Filters: []*pb.MeterFilter{
			{Attribute: "outcome", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN, Values: []string{"cached", "success"}},
			{Attribute: "bytes", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_GREATER_OR_EQUAL, Values: []string{"10"}},
			{Attribute: "debug", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS, Values: []string{"false"}},
		}, GroupBy: []string{"region"}}
	raw, err := json.Marshal(stored)
	if err != nil {
		t.Fatal(err)
	}
	definition, err := decodeMeter(raw)
	if err != nil {
		t.Fatal(err)
	}
	if definition.Key != "requests" || definition.Aggregation != metering.UniqueCount || len(definition.Filters) != 3 || definition.Filters[1].Value != number(t, "10") {
		t.Fatalf("stored definition changed: %+v", definition)
	}
	event := metering.Event{Type: "request", Attributes: map[string]metering.Value{"outcome": metering.String("cached"), "bytes": number(t, "10"), "region": metering.String("eu"), "user": metering.String("u1")}}
	contributions, err := match([]meter{{ID: "m", Definition: definition}}, event)
	if err != nil || len(contributions) != 1 || contributions[0].MeterID != "m" || contributions[0].Value != metering.String("u1") {
		t.Fatalf("stored definition did not match: %+v %v", contributions, err)
	}
	event.Attributes["debug"] = metering.Boolean(true)
	if _, err = match([]meter{{ID: "m", Definition: definition}}, event); reasonOf(err) != ReasonNoMatchingMeter {
		t.Fatalf("existence filter ignored: %v", err)
	}
	delete(event.Attributes, "debug")
	delete(event.Attributes, "region")
	if _, err = match([]meter{{ID: "m", Definition: definition}}, event); reasonOf(err) != ReasonInvalidAttribute {
		t.Fatalf("missing group attribute accepted: %v", err)
	}
	if _, err = match(nil, event); reasonOf(err) != ReasonNoMatchingMeter {
		t.Fatal("event without service meters accepted")
	}
	for _, bad := range []storedMeter{
		{Name: "x", EventType: "e", Aggregation: pb.MeterAggregation_METER_AGGREGATION_UNSPECIFIED},
		{Name: "x", EventType: "e", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, Filters: []*pb.MeterFilter{{Attribute: "a", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_LESS, Values: []string{"x"}}}},
		{Name: "x", EventType: "e", Aggregation: pb.MeterAggregation_METER_AGGREGATION_SUM},
	} {
		raw, _ := json.Marshal(bad)
		if _, err := decodeMeter(raw); err == nil {
			t.Fatalf("invalid stored meter accepted: %+v", bad)
		}
	}
}
