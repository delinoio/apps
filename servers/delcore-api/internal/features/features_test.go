// SPDX-License-Identifier: FSL-1.1-ALv2

package features

import (
	"errors"
	"strings"
	"testing"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"google.golang.org/protobuf/proto"
)

func decimal(t *testing.T, s string) accounting.Decimal {
	t.Helper()
	d, err := accounting.Parse(s)
	if err != nil {
		t.Fatal(err)
	}
	return d
}

func TestDecodeGrantAccessAndMetered(t *testing.T) {
	g, err := DecodeGrant(pb.FeatureKind_FEATURE_KIND_ACCESS, 0, []byte(`{"enabled":true}`))
	if err != nil || !g.Enabled {
		t.Fatal("enabled access grant", g, err)
	}
	g, err = DecodeGrant(pb.FeatureKind_FEATURE_KIND_ACCESS, 0, []byte(`{"enabled":false}`))
	if err != nil || g.Enabled {
		t.Fatal("disabled access grant", g, err)
	}
	g, err = DecodeGrant(pb.FeatureKind_FEATURE_KIND_METERED, 0, []byte(`{"limit":"10.50"}`))
	if err != nil || g.Limit.String() != "10.5" {
		t.Fatal("metered grant", g, err)
	}
	for _, tc := range []struct {
		kind pb.FeatureKind
		raw  string
	}{
		{pb.FeatureKind_FEATURE_KIND_ACCESS, `{"enabled":"yes"}`},
		{pb.FeatureKind_FEATURE_KIND_ACCESS, `{"limit":"1"}`},
		{pb.FeatureKind_FEATURE_KIND_ACCESS, `{"enabled":true,"limit":"1"}`},
		{pb.FeatureKind_FEATURE_KIND_METERED, `{"limit":"-1"}`},
		{pb.FeatureKind_FEATURE_KIND_METERED, `{"limit":1}`},
		{pb.FeatureKind_FEATURE_KIND_METERED, `{"enabled":true}`},
		{pb.FeatureKind_FEATURE_KIND_UNSPECIFIED, `{"enabled":true}`},
		{pb.FeatureKind_FEATURE_KIND_ACCESS, `not json`},
	} {
		if _, err = DecodeGrant(tc.kind, 0, []byte(tc.raw)); !errors.Is(err, ErrGrant) {
			t.Fatalf("%s %s accepted: %v", tc.kind, tc.raw, err)
		}
	}
}

func TestDecodeGrantConfigurationTypes(t *testing.T) {
	for _, tc := range []struct {
		valueType pb.ConfigurationValueType
		raw       string
		want      *pb.ConfigurationValue
	}{
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_BOOLEAN, `{"boolean":true}`, &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Boolean{Boolean: true}}},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_INTEGER, `{"integer":-30}`, &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Integer{Integer: -30}}},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_DECIMAL, `{"decimal":"2.50"}`, &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Decimal{Decimal: "2.5"}}},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING, `{"text":"eu-west"}`, &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Text{Text: "eu-west"}}},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING_LIST, `{"string_list":["a","b"]}`, &pb.ConfigurationValue{Value: &pb.ConfigurationValue_StringList{StringList: &pb.StringList{Values: []string{"a", "b"}}}}},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING_LIST, `{"string_list":null}`, &pb.ConfigurationValue{Value: &pb.ConfigurationValue_StringList{StringList: &pb.StringList{Values: []string{}}}}},
	} {
		g, err := DecodeGrant(pb.FeatureKind_FEATURE_KIND_CONFIGURATION, tc.valueType, []byte(tc.raw))
		if err != nil || !proto.Equal(g.Configuration, tc.want) {
			t.Fatalf("%s: got %v %v", tc.valueType, g.Configuration, err)
		}
	}
	for _, tc := range []struct {
		valueType pb.ConfigurationValueType
		raw       string
	}{
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_BOOLEAN, `{"integer":1}`},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_INTEGER, `{"integer":1.5}`},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_DECIMAL, `{"decimal":"1e3"}`},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING, `{"text":1}`},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING_LIST, `{"string_list":"a"}`},
		{pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_UNSPECIFIED, `{"text":"a"}`},
	} {
		if _, err := DecodeGrant(pb.FeatureKind_FEATURE_KIND_CONFIGURATION, tc.valueType, []byte(tc.raw)); !errors.Is(err, ErrGrant) {
			t.Fatalf("%s %s accepted: %v", tc.valueType, tc.raw, err)
		}
	}
}

func TestDecodeMeter(t *testing.T) {
	d, err := DecodeMeter(nil)
	if err != nil || d.EventType != "" || len(d.GroupBy) != 0 {
		t.Fatal("legacy meter", d, err)
	}
	d, err = DecodeMeter([]byte(`{"name":"m","event_type":"request","aggregation":2,"value_attribute":"bytes","filters":[],"group_by":["model","region"]}`))
	if err != nil || d.Aggregation != pb.MeterAggregation_METER_AGGREGATION_SUM || strings.Join(d.GroupBy, ",") != "model,region" {
		t.Fatal("typed meter", d, err)
	}
	if _, err = DecodeMeter([]byte(`[`)); err == nil {
		t.Fatal("malformed meter accepted")
	}
}

func TestGroupKeyMatchesIngestionGrouping(t *testing.T) {
	region := metering.String("eu")
	five, err := metering.Number("5")
	if err != nil {
		t.Fatal(err)
	}
	values := map[string]metering.Value{"region": region, "tier": five}
	key, err := GroupKey([]string{"tier", "region"}, values)
	if err != nil {
		t.Fatal(err)
	}
	stored, err := metering.GroupKey(values)
	if err != nil || key != stored {
		t.Fatalf("group key %q differs from ingestion key %q: %v", key, stored, err)
	}
	empty, err := GroupKey(nil, nil)
	if err != nil || empty != "[]" {
		t.Fatalf("ungrouped meter key %q: %v", empty, err)
	}
	// Scalar types are part of a group: a numeric 5 and the text "5" differ.
	textual, err := GroupKey([]string{"tier", "region"}, map[string]metering.Value{"region": region, "tier": metering.String("5")})
	if err != nil || textual == key {
		t.Fatal("scalar type did not select a different group", err)
	}
	for name, invalid := range map[string]map[string]metering.Value{
		"missing":        {"region": region},
		"extra":          {"region": region, "tier": five, "model": metering.String("x")},
		"wrong name":     {"region": region, "level": five},
		"long string":    {"region": metering.String(strings.Repeat("x", 1025)), "tier": five},
		"bad boolean":    {"region": {Kind: metering.ValueBool, Text: "yes"}, "tier": five},
		"bad number":     {"region": region, "tier": {Kind: metering.ValueNumber, Text: "5.0"}},
		"unknown scalar": {"region": {Kind: "object", Text: "{}"}, "tier": five},
	} {
		if _, err = GroupKey([]string{"region", "tier"}, invalid); !errors.Is(err, ErrGroup) {
			t.Fatalf("%s group values accepted: %v", name, err)
		}
	}
}

func TestEvaluate(t *testing.T) {
	zero := accounting.Zero()
	limit := Grant{Kind: pb.FeatureKind_FEATURE_KIND_METERED, Limit: decimal(t, "10")}
	for _, tc := range []struct {
		name     string
		grants   []Grant
		applied  accounting.Decimal
		entitled bool
		reason   pb.BlockReason
	}{
		{"no grant", nil, zero, false, pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED},
		{"conflicting grants fail closed", []Grant{{Kind: pb.FeatureKind_FEATURE_KIND_ACCESS, Enabled: true}, {Kind: pb.FeatureKind_FEATURE_KIND_ACCESS, Enabled: true}}, zero, false, pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED},
		{"enabled access", []Grant{{Kind: pb.FeatureKind_FEATURE_KIND_ACCESS, Enabled: true}}, zero, true, pb.BlockReason_BLOCK_REASON_UNSPECIFIED},
		{"disabled access", []Grant{{Kind: pb.FeatureKind_FEATURE_KIND_ACCESS}}, zero, false, pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED},
		{"configuration", []Grant{{Kind: pb.FeatureKind_FEATURE_KIND_CONFIGURATION}}, zero, true, pb.BlockReason_BLOCK_REASON_UNSPECIFIED},
		{"below limit", []Grant{limit}, decimal(t, "9.999999"), true, pb.BlockReason_BLOCK_REASON_UNSPECIFIED},
		{"negative usage", []Grant{limit}, decimal(t, "-20"), true, pb.BlockReason_BLOCK_REASON_UNSPECIFIED},
		{"at limit", []Grant{limit}, decimal(t, "10.000"), true, pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED},
		{"over limit", []Grant{limit}, decimal(t, "11"), true, pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED},
		{"zero limit", []Grant{{Kind: pb.FeatureKind_FEATURE_KIND_METERED, Limit: zero}}, zero, true, pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED},
		{"unknown kind", []Grant{{}}, zero, false, pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED},
	} {
		got := Evaluate(tc.grants, tc.applied)
		if got.Entitled != tc.entitled || got.Reason != tc.reason {
			t.Fatalf("%s: got %+v", tc.name, got)
		}
	}
}
