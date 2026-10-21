// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"io"
	"log/slog"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// A YAML exists filter without values and typed number and boolean operands
// apply through catalog synchronization, stay stable across restarts and
// equivalent PutMeter encodings, and match typed event attributes end to end.
func TestTypedAndExistenceFiltersFromYAMLAndPutMeter(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	root := t.TempDir()
	setCatalogText(t, root, "service.yaml", "kind: service\nkey: jobs\nname: Jobs\nclient_id: service\ncapabilities: [execute]\nscopes: [authorize, usage:write, usage:read]\n")
	setCatalogText(t, root, "meter.yaml", `kind: meter
service: jobs
key: urgent-jobs
event_type: job
aggregation: count
filters:
  - attribute: priority
    operator: equal
    values: [5]
  - attribute: urgent
    operator: in
    values: [true, "yes"]
  - attribute: retried
    operator: not_equal
    values: [true]
  - attribute: trace
    operator: exists
`)
	for range 2 {
		if err := syncCatalog(ctx, db, root, logger); err != nil {
			t.Fatal("typed or existence filters rejected", err)
		}
	}
	serviceID, _ := catalogIdentity(t, db, "service", "", "jobs")
	meterID, version := catalogIdentity(t, db, "meter", serviceID, "urgent-jobs")
	catalogResponse, err := client.GetCatalog(ctx, req(&pb.GetCatalogRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	var declared *pb.Meter
	for _, m := range catalogResponse.Msg.Meters {
		if m.Id == meterID {
			declared = m
		}
	}
	if declared == nil || len(declared.Filters) != 4 {
		t.Fatalf("catalog meter = %+v", declared)
	}
	for _, filter := range declared.Filters {
		switch filter.Attribute {
		case "trace":
			if len(filter.Values) != 1 || filter.Values[0] != "true" || len(filter.TypedValues) != 0 {
				t.Fatalf("exists filter = %+v", filter)
			}
		case "priority", "urgent", "retried":
			if len(filter.Values) != 0 || len(filter.TypedValues) == 0 {
				t.Fatalf("typed filter = %+v", filter)
			}
		}
	}
	// An equivalent PutMeter encoding reuses the version: untyped exists
	// "true", a non-canonical typed number, and reordered membership.
	equivalent := &pb.Meter{ServiceId: serviceID, Name: "urgent-jobs", Key: "urgent-jobs", EventType: "job", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, Filters: []*pb.MeterFilter{
		{Attribute: "trace", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS, Values: []string{"true"}},
		{Attribute: "urgent", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN, TypedValues: []*pb.MeterFilterValue{{Kind: pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_STRING, Value: "yes"}, {Kind: pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_BOOLEAN, Value: "true"}}},
		{Attribute: "priority", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL, TypedValues: []*pb.MeterFilterValue{{Kind: pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_NUMBER, Value: "5.00"}}},
		{Attribute: "retried", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_NOT_EQUAL, TypedValues: []*pb.MeterFilterValue{{Kind: pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_BOOLEAN, Value: "true"}}},
	}}
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "equivalent meter encoding", Meter: equivalent}, "owner")); err != nil {
		t.Fatal("equivalent PutMeter encoding rejected", err)
	}
	if _, now := catalogIdentity(t, db, "meter", serviceID, "urgent-jobs"); now != version {
		t.Fatal("equivalent encoding created a new meter version")
	}
	equivalent.Filters[2].TypedValues[0].Kind = pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_STRING
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "changed operand type", Meter: equivalent}, "owner")); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("operand type change accepted for an immutable key: %v", err)
	}
	both := &pb.Meter{ServiceId: serviceID, Name: "both-forms", Key: "both-forms", EventType: "job", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, Filters: []*pb.MeterFilter{
		{Attribute: "a", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL, Values: []string{"x"}, TypedValues: []*pb.MeterFilterValue{{Kind: pb.MeterFilterValueKind_METER_FILTER_VALUE_KIND_STRING, Value: "x"}}},
	}}
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "ambiguous operands", Meter: both}, "owner")); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("filter with both operand forms accepted: %v", err)
	}

	_, tenant := bootstrap(t, client)
	ingest := func(eventKey string, attributes map[string]*pb.AttributeValue) error {
		_, err := client.IngestEvent(ctx, req(&pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: tenant, ServiceId: serviceID, EventKey: eventKey, EventType: "job", OccurredAt: timestamppb.New(time.Now().UTC().Add(-time.Minute)), Attributes: attributes}}, "service"))
		return err
	}
	flag := func(v bool) *pb.AttributeValue { return &pb.AttributeValue{Value: &pb.AttributeValue_Boolean{Boolean: v}} }
	matching := func() map[string]*pb.AttributeValue {
		return map[string]*pb.AttributeValue{"priority": num("5"), "urgent": flag(true), "retried": flag(false), "trace": text("t")}
	}
	if err = ingest("typed-match", matching()); err != nil {
		t.Fatal("typed attributes did not match typed operands", err)
	}
	for name, mutate := range map[string]func(map[string]*pb.AttributeValue){
		"string priority":   func(a map[string]*pb.AttributeValue) { a["priority"] = text("5") },
		"other priority":    func(a map[string]*pb.AttributeValue) { a["priority"] = num("6") },
		"string urgent":     func(a map[string]*pb.AttributeValue) { a["urgent"] = text("true") },
		"retried":           func(a map[string]*pb.AttributeValue) { a["retried"] = flag(true) },
		"missing existence": func(a map[string]*pb.AttributeValue) { delete(a, "trace") },
	} {
		attributes := matching()
		mutate(attributes)
		if err = ingest("typed-"+name, attributes); reasonOf(err) != pb.ErrorReason_ERROR_REASON_NO_MATCHING_METER {
			t.Fatalf("%s matched: %v", name, err)
		}
	}
	alternative := matching()
	alternative["urgent"] = text("yes")
	alternative["retried"] = text("true")
	if err = ingest("typed-membership-text", alternative); err != nil {
		t.Fatal("string membership operand or typed inequality failed", err)
	}
}
