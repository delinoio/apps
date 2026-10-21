// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math/rand"
	"net/http"
	"net/http/httptest"
	"sort"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/delcore-api/internal/accounting"
	"github.com/delinoio/apps/servers/delcore-api/internal/config"
	"github.com/delinoio/apps/servers/delcore-api/internal/events"
	"github.com/delinoio/apps/servers/delcore-api/internal/metering"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/internal/rpc"
	"github.com/delinoio/apps/servers/delcore-api/internal/telemetry"
	"github.com/delinoio/apps/servers/delcore-api/internal/worker"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// meterFixture registers one service with meters for all six aggregations.
type meterFixture struct {
	db          *postgres.Store
	client      delcorev1connect.DelcoreServiceClient
	cfg         config.Config
	tenant      string
	service     string
	definitions []metering.Definition
	worker      *worker.Worker
	clockMu     sync.Mutex
	clock       time.Time
}

func (f *meterFixture) now() time.Time {
	f.clockMu.Lock()
	defer f.clockMu.Unlock()
	if f.clock.IsZero() {
		return time.Now().UTC()
	}
	return f.clock
}

func (f *meterFixture) setClock(at time.Time) {
	f.clockMu.Lock()
	defer f.clockMu.Unlock()
	f.clock = at
}

func newMeterFixture(t *testing.T, mutate func(*config.Config)) *meterFixture {
	t.Helper()
	return newObservedMeterFixture(t, mutate, nil, nil)
}

// newObservedMeterFixture lets telemetry tests capture the server and worker
// logs and metrics. A nil logger discards logs; nil metrics record nothing.
func newObservedMeterFixture(t *testing.T, mutate func(*config.Config), logger *slog.Logger, metrics *telemetry.Metrics) *meterFixture {
	t.Helper()
	db, _, cfg := setup(t)
	if mutate != nil {
		mutate(&cfg)
	}
	if logger == nil {
		logger = slog.New(slog.NewTextHandler(io.Discard, nil))
	}
	f := &meterFixture{db: db, cfg: cfg}
	server := &rpc.Server{DB: db, Auth: verifier{}, Config: cfg, Log: logger, Now: f.now, Metrics: metrics}
	path, handler := server.Handler()
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	httpServer := httptest.NewServer(mux)
	t.Cleanup(httpServer.Close)
	f.client = delcorev1connect.NewDelcoreServiceClient(httpServer.Client(), httpServer.URL)
	f.worker = &worker.Worker{DB: db, Config: cfg, Log: logger, Metrics: metrics}
	_, f.tenant = bootstrap(t, f.client)
	ctx := context.Background()
	service, err := f.client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "register metered service", Service: &pb.Service{Key: "metered", Name: "metered", ClientId: "service", Capabilities: []string{"execute"}, Scopes: []string{"authorize", "usage:write", "usage:read"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	f.service = service.Msg.Service.Id
	in := func(values ...string) []*pb.MeterFilter {
		return []*pb.MeterFilter{{Attribute: "outcome", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN, Values: values}}
	}
	for _, m := range []*pb.Meter{
		{Name: "requests", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, Filters: in("cached", "success"), GroupBy: []string{"region"}},
		{Name: "bytes", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_SUM, ValueAttribute: "bytes", GroupBy: []string{"region"}},
		{Name: "latency-min", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_MIN, ValueAttribute: "latency"},
		{Name: "latency-max", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_MAX, ValueAttribute: "latency"},
		{Name: "latency-avg", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_AVG, ValueAttribute: "latency"},
		{Name: "users", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_UNIQUE_COUNT, ValueAttribute: "user", GroupBy: []string{"region"}},
		{Name: "probe-avg", EventType: "probe", Aggregation: pb.MeterAggregation_METER_AGGREGATION_AVG, ValueAttribute: "n", GroupBy: []string{"case"}},
	} {
		m.ServiceId, m.Key = f.service, m.Name
		if _, err = f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare event meter", Meter: m}, "owner")); err != nil {
			t.Fatal(err)
		}
		definition, err := events.CatalogDefinition(m.Name, m.EventType, m.Aggregation, m.ValueAttribute, m.Filters, m.GroupBy)
		if err != nil {
			t.Fatal(err)
		}
		f.definitions = append(f.definitions, definition)
	}
	return f
}

type request struct{ region, outcome, user, bytes, latency string }

func (r request) wire() map[string]*pb.AttributeValue {
	return map[string]*pb.AttributeValue{"region": text(r.region), "outcome": text(r.outcome), "user": text(r.user), "bytes": num(r.bytes), "latency": num(r.latency)}
}

func (r request) event(t *testing.T) metering.Event {
	t.Helper()
	attributes := map[string]metering.Value{"region": metering.String(r.region), "outcome": metering.String(r.outcome), "user": metering.String(r.user)}
	for name, value := range map[string]string{"bytes": r.bytes, "latency": r.latency} {
		n, err := metering.Number(value)
		if err != nil {
			t.Fatal(err)
		}
		attributes[name] = n
	}
	return metering.Event{Type: "request", Attributes: attributes}
}

func text(s string) *pb.AttributeValue {
	return &pb.AttributeValue{Value: &pb.AttributeValue_Text{Text: s}}
}
func num(s string) *pb.AttributeValue {
	return &pb.AttributeValue{Value: &pb.AttributeValue_Number{Number: s}}
}

func (f *meterFixture) ingest(token, eventKey, eventType string, occurred time.Time, attributes map[string]*pb.AttributeValue) (*pb.IngestEventResponse, error) {
	response, err := f.client.IngestEvent(context.Background(), req(&pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: f.tenant, ServiceId: f.service, EventKey: eventKey, EventType: eventType, OccurredAt: timestamppb.New(occurred), Attributes: attributes}}, token))
	if err != nil {
		return nil, err
	}
	return response.Msg, nil
}

func (f *meterFixture) correct(eventKey string, expected int32, kind pb.EventCorrectionKind, eventType string, attributes map[string]*pb.AttributeValue) (*pb.CorrectEventResponse, error) {
	response, err := f.client.CorrectEvent(context.Background(), req(&pb.CorrectEventRequest{RequestKey: key(), TenantId: f.tenant, EventKey: eventKey, ExpectedRevision: expected, Kind: kind, EventType: eventType, Attributes: attributes}, "service"))
	if err != nil {
		return nil, err
	}
	return response.Msg, nil
}

func (f *meterFixture) process(t *testing.T) {
	t.Helper()
	if _, err := f.worker.ProcessMetering(context.Background(), 10000); err != nil {
		t.Fatal(err)
	}
}

func reasonOf(err error) pb.ErrorReason {
	var ce *connect.Error
	if !errors.As(err, &ce) {
		return pb.ErrorReason_ERROR_REASON_UNSPECIFIED
	}
	for _, detail := range ce.Details() {
		if value, e := detail.Value(); e == nil {
			if typed, ok := value.(*pb.ErrorDetail); ok {
				return typed.Reason
			}
		}
	}
	return pb.ErrorReason_ERROR_REASON_UNSPECIFIED
}

func (f *meterFixture) count(t *testing.T, query string, args ...any) int {
	t.Helper()
	var n int
	if err := f.db.Pool.QueryRow(context.Background(), query, args...).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

type persisted struct {
	count, distinct int64
	sum, value      string
}

func (f *meterFixture) aggregates(t *testing.T) map[string]map[string]persisted {
	t.Helper()
	rows, err := f.db.Pool.Query(context.Background(), `SELECT ce.key,a.group_key,a.event_count,a.distinct_count,a.sum::text,a.value::text FROM metering_aggregates a JOIN catalog_entities ce ON ce.id=a.meter_id WHERE a.tenant_id=$1`, f.tenant)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	out := map[string]map[string]persisted{}
	for rows.Next() {
		var meter, group string
		var p persisted
		if err = rows.Scan(&meter, &group, &p.count, &p.distinct, &p.sum, &p.value); err != nil {
			t.Fatal(err)
		}
		if out[meter] == nil {
			out[meter] = map[string]persisted{}
		}
		out[meter][group] = p
	}
	if err = rows.Err(); err != nil {
		t.Fatal(err)
	}
	return out
}

func (f *meterFixture) value(t *testing.T, meter string, groupValues map[string]metering.Value) string {
	t.Helper()
	groupKey, err := metering.GroupKey(groupValues)
	if err != nil {
		t.Fatal(err)
	}
	p, ok := f.aggregates(t)[meter][groupKey]
	if !ok {
		return ""
	}
	parsed, err := accounting.Parse(p.value)
	if err != nil {
		t.Fatal(err)
	}
	return parsed.String()
}

// expect compares materialized aggregates with an in-memory rebuild of the
// latest active revisions, so incremental processing matches exact semantics.
func (f *meterFixture) expect(t *testing.T, active map[string]metering.Event) {
	t.Helper()
	state, err := metering.Build(f.definitions, active)
	if err != nil {
		t.Fatal(err)
	}
	got := f.aggregates(t)
	want := state.Aggregates()
	for meter, groups := range want {
		if len(groups) != len(got[meter]) {
			t.Fatalf("meter %s has %d groups, want %d: %+v", meter, len(got[meter]), len(groups), got[meter])
		}
		for groupKey, aggregate := range groups {
			p, ok := got[meter][groupKey]
			if !ok {
				t.Fatalf("meter %s lacks group %s", meter, groupKey)
			}
			value, _ := accounting.Parse(p.value)
			sum, _ := accounting.Parse(p.sum)
			if p.count != aggregate.Count || value.Cmp(aggregate.Value) != 0 || p.distinct != aggregate.DistinctCount || sum.Cmp(aggregate.Sum) != 0 {
				t.Fatalf("meter %s group %s = %+v, want %+v", meter, groupKey, p, aggregate)
			}
		}
	}
	for meter, groups := range got {
		if len(groups) > 0 && len(want[meter]) == 0 {
			t.Fatalf("meter %s retained groups without active events: %+v", meter, groups)
		}
	}
}

func TestEventIngestionValidationReplayAndIsolation(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	now := time.Now().UTC()
	valid := request{region: "eu", outcome: "success", user: "u1", bytes: "10.50", latency: "3"}
	noEffects := func(stage string) {
		t.Helper()
		if n := f.count(t, "SELECT count(*) FROM metering_events") + f.count(t, "SELECT count(*) FROM jobs WHERE kind='metering'") + f.count(t, "SELECT count(*) FROM metering_admission_groups"); n != 0 {
			t.Fatalf("%s left partial effects: %d", stage, n)
		}
	}
	missing := valid.wire()
	delete(missing, "latency")
	wrongType := valid.wire()
	wrongType["bytes"] = text("10")
	badNumber := valid.wire()
	badNumber["bytes"] = num("1e3")
	unset := valid.wire()
	unset["region"] = &pb.AttributeValue{}
	for _, tc := range []struct {
		name       string
		eventType  string
		attributes map[string]*pb.AttributeValue
		reason     pb.ErrorReason
	}{
		{"unmatched event type", "unknown", valid.wire(), pb.ErrorReason_ERROR_REASON_NO_MATCHING_METER},
		{"one matched meter lacks an attribute", "request", missing, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE},
		{"numeric meter receives text", "request", wrongType, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE},
		{"binary exponent number", "request", badNumber, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE},
		{"empty scalar", "request", unset, pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE},
	} {
		if _, err := f.ingest("service", "rejected", tc.eventType, now, tc.attributes); reasonOf(err) != tc.reason {
			t.Fatalf("%s: got %v", tc.name, err)
		}
		noEffects(tc.name)
	}
	if _, err := f.ingest("service", "future", "request", now.Add(time.Hour), valid.wire()); reasonOf(err) != pb.ErrorReason_ERROR_REASON_INVALID_INPUT {
		t.Fatalf("future occurrence accepted: %v", err)
	}
	if _, err := f.ingest("service", "late", "request", now.Add(-time.Duration(f.cfg.UsageLatenessSeconds+60)*time.Second), valid.wire()); reasonOf(err) != pb.ErrorReason_ERROR_REASON_REPORTING_WINDOW_CLOSED {
		t.Fatalf("late occurrence accepted: %v", err)
	}
	if _, err := f.ingest("owner", "user", "request", now, valid.wire()); connect.CodeOf(err) != connect.CodePermissionDenied {
		t.Fatalf("user token reported usage: %v", err)
	}
	if _, err := f.client.IngestEvent(ctx, req(&pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: f.tenant, ServiceId: key(), EventKey: "x", EventType: "request", OccurredAt: timestamppb.New(now), Attributes: valid.wire()}}, "service")); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("foreign service attribution accepted: %v", err)
	}
	noEffects("rejections")

	accepted, err := f.ingest("service", "event-1", "request", now, valid.wire())
	if err != nil || accepted.Revision != 1 || accepted.EventId == "" {
		t.Fatalf("valid event rejected: %v %v", accepted, err)
	}
	// Equal decimals and a different request key replay without new effects.
	replay := valid
	replay.bytes = "10.5"
	again, err := f.ingest("service", "event-1", "request", now, replay.wire())
	if err != nil || again.EventId != accepted.EventId || again.Revision != 1 {
		t.Fatalf("identical replay failed: %v %v", again, err)
	}
	cached := &pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: f.tenant, ServiceId: f.service, EventKey: "event-1", EventType: "request", OccurredAt: timestamppb.New(now), Attributes: valid.wire()}}
	for range 2 {
		if response, err := f.client.IngestEvent(ctx, req(cached, "service")); err != nil || response.Msg.EventId != accepted.EventId {
			t.Fatalf("request key replay failed: %v", err)
		}
	}
	changed := valid
	changed.bytes = "11"
	if _, err = f.ingest("service", "event-1", "request", now, changed.wire()); reasonOf(err) != pb.ErrorReason_ERROR_REASON_EVENT_IDENTITY_CONFLICT || connect.CodeOf(err) != connect.CodeAlreadyExists {
		t.Fatalf("conflicting content accepted: %v", err)
	}
	if n := f.count(t, "SELECT count(*) FROM metering_events") + f.count(t, "SELECT count(*) FROM jobs WHERE kind='metering'"); n != 2 {
		t.Fatalf("replay repeated effects: %d", n)
	}

	// The acknowledgement promises durable storage only.
	read, err := f.client.GetEvent(ctx, req(&pb.GetEventRequest{TenantId: f.tenant, EventKey: "event-1"}, "service"))
	if err != nil || read.Msg.Revision != 1 || read.Msg.AppliedRevision != 0 || read.Msg.Cancelled || read.Msg.Event.Attributes["bytes"].GetNumber() != "10.5" || !read.Msg.Event.OccurredAt.AsTime().Equal(now.Truncate(time.Microsecond)) {
		t.Fatalf("unexpected pending event read: %v %v", read, err)
	}
	if len(f.aggregates(t)) != 0 {
		t.Fatal("aggregates changed before processing")
	}
	f.process(t)
	read, err = f.client.GetEvent(ctx, req(&pb.GetEventRequest{TenantId: f.tenant, EventKey: "event-1"}, "service"))
	if err != nil || read.Msg.AppliedRevision != 1 {
		t.Fatalf("processed revision not visible: %v %v", read, err)
	}
	f.expect(t, map[string]metering.Event{"event-1": valid.event(t)})

	// Event keys are service scoped; another service cannot read or correct them.
	other, err := f.client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "register second service", Service: &pb.Service{Key: "other", Name: "other", ClientId: "other", Capabilities: []string{"execute"}, Scopes: []string{"authorize", "usage:write", "usage:read"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err = f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare other meter", Meter: &pb.Meter{ServiceId: other.Msg.Service.Id, Name: "requests", Key: "requests", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT}}, "owner")); err != nil {
		t.Fatal(err)
	}
	if _, err = f.client.GetEvent(ctx, req(&pb.GetEventRequest{TenantId: f.tenant, EventKey: "event-1"}, "other")); reasonOf(err) != pb.ErrorReason_ERROR_REASON_EVENT_NOT_FOUND {
		t.Fatalf("cross-service read: %v", err)
	}
	if _, err = f.client.CorrectEvent(ctx, req(&pb.CorrectEventRequest{RequestKey: key(), TenantId: f.tenant, EventKey: "event-1", ExpectedRevision: 1, Kind: pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL}, "other")); reasonOf(err) != pb.ErrorReason_ERROR_REASON_EVENT_NOT_FOUND {
		t.Fatalf("cross-service correction: %v", err)
	}
	otherEvent, err := f.client.IngestEvent(ctx, req(&pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: f.tenant, ServiceId: other.Msg.Service.Id, EventKey: "event-1", EventType: "request", OccurredAt: timestamppb.New(now)}}, "other"))
	if err != nil || otherEvent.Msg.EventId == accepted.EventId {
		t.Fatalf("same key in another service was not independent: %v", err)
	}
	stranger, err := f.client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "stranger"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err = f.client.GetEvent(ctx, req(&pb.GetEventRequest{TenantId: stranger.Msg.Tenants[0].Id, EventKey: "event-1"}, "service")); connect.CodeOf(err) != connect.CodePermissionDenied {
		t.Fatalf("cross-tenant read: %v", err)
	}
	if _, err = f.client.CorrectEvent(ctx, req(&pb.CorrectEventRequest{RequestKey: key(), TenantId: stranger.Msg.Tenants[0].Id, EventKey: "event-1", ExpectedRevision: 1, Kind: pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL}, "service")); connect.CodeOf(err) != connect.CodePermissionDenied {
		t.Fatalf("cross-tenant correction: %v", err)
	}
	history, err := f.client.ListBillingHistory(ctx, req(&pb.ListBillingHistoryRequest{TenantId: f.tenant}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	for _, j := range history.Msg.Jobs {
		if j.Kind == events.JobKind {
			t.Fatal("billing history exposed metering processing work")
		}
	}
}

func TestEventAggregationsCorrectionsAndHistory(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	now := time.Now().UTC().Add(-time.Minute)
	active := map[string]metering.Event{}
	put := func(eventKey string, r request) {
		t.Helper()
		if _, err := f.ingest("service", eventKey, "request", now, r.wire()); err != nil {
			t.Fatal(err)
		}
		active[eventKey] = r.event(t)
	}
	put("a", request{region: "eu", outcome: "success", user: "u1", bytes: "-5", latency: "-10.1"})
	put("b", request{region: "eu", outcome: "cached", user: "u1", bytes: "20", latency: "0.2"})
	put("c", request{region: "us", outcome: "failure", user: "u2", bytes: "0", latency: "0.3"})
	f.process(t)
	f.expect(t, active)
	eu := map[string]metering.Value{"region": metering.String("eu")}
	us := map[string]metering.Value{"region": metering.String("us")}
	for meter, want := range map[string]string{"latency-min": "-10.1", "latency-max": "0.3", "latency-avg": "-3.2"} {
		if got := f.value(t, meter, map[string]metering.Value{}); got != want {
			t.Fatalf("%s = %s, want %s", meter, got, want)
		}
	}
	if f.value(t, "requests", eu) != "2" || f.value(t, "requests", us) != "" || f.value(t, "bytes", eu) != "15" || f.value(t, "users", eu) != "1" {
		t.Fatal("count, filtered group, negative sum, or exact unique count is wrong")
	}

	// Replacement moves the only us event to eu: the us group disappears and
	// the eu unique count gains an exact distinct value.
	moved := request{region: "eu", outcome: "success", user: "u3", bytes: "1", latency: "8"}
	corrected, err := f.correct("c", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", moved.wire())
	if err != nil || corrected.Revision != 2 {
		t.Fatalf("replacement rejected: %v %v", corrected, err)
	}
	active["c"] = moved.event(t)
	f.process(t)
	f.expect(t, active)
	if f.value(t, "bytes", us) != "" || f.value(t, "users", eu) != "2" || f.value(t, "latency-max", map[string]metering.Value{}) != "8" {
		t.Fatal("replacement did not remove or introduce groups exactly")
	}
	// Stale revisions fail; an identical retry of the accepted correction replays.
	if _, err = f.correct("c", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); reasonOf(err) != pb.ErrorReason_ERROR_REASON_STALE_REVISION || connect.CodeOf(err) != connect.CodeAborted {
		t.Fatalf("stale correction accepted: %v", err)
	}
	if retried, err := f.correct("c", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", moved.wire()); err != nil || retried.Revision != 2 {
		t.Fatalf("identical correction retry failed: %v %v", retried, err)
	}
	if _, err = f.correct("c", 3, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); reasonOf(err) != pb.ErrorReason_ERROR_REASON_STALE_REVISION {
		t.Fatalf("future revision accepted: %v", err)
	}
	if _, err = f.correct("c", 2, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "unknown", moved.wire()); reasonOf(err) != pb.ErrorReason_ERROR_REASON_NO_MATCHING_METER {
		t.Fatalf("unmatched replacement accepted: %v", err)
	}
	if _, err = f.correct("missing", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); reasonOf(err) != pb.ErrorReason_ERROR_REASON_EVENT_NOT_FOUND {
		t.Fatalf("unknown event corrected: %v", err)
	}
	if _, err = f.correct("c", 2, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "request", nil); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("cancellation with content accepted: %v", err)
	}

	// Concurrent corrections of one revision: exactly one wins.
	var wg sync.WaitGroup
	results := make(chan error, 6)
	for i := range 6 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			r := moved
			r.user = fmt.Sprintf("concurrent-%d", i)
			_, err := f.correct("c", 2, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", r.wire())
			results <- err
		}()
	}
	wg.Wait()
	close(results)
	wins := 0
	for err := range results {
		if err == nil {
			wins++
		} else if reasonOf(err) != pb.ErrorReason_ERROR_REASON_STALE_REVISION {
			t.Fatalf("concurrent correction failed unexpectedly: %v", err)
		}
	}
	if wins != 1 {
		t.Fatalf("concurrent corrections overwrote each other: %d winners", wins)
	}
	read, err := f.client.GetEvent(ctx, req(&pb.GetEventRequest{TenantId: f.tenant, EventKey: "c"}, "service"))
	if err != nil || read.Msg.Revision != 3 {
		t.Fatalf("re-read after concurrency: %v %v", read, err)
	}
	winner := moved
	winner.user = read.Msg.Event.Attributes["user"].GetText()
	active["c"] = winner.event(t)
	// Cancelling both eu events that used u1 removes the value exactly, and
	// cancelling every event leaves no empty groups.
	for _, eventKey := range []string{"a", "b"} {
		if _, err = f.correct(eventKey, 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
			t.Fatal(err)
		}
		delete(active, eventKey)
	}
	f.process(t)
	f.expect(t, active)
	if f.value(t, "users", eu) != "1" {
		t.Fatal("unique count did not remove the last duplicate value")
	}
	if _, err = f.correct("a", 2, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", moved.wire()); reasonOf(err) != pb.ErrorReason_ERROR_REASON_EVENT_CANCELLED {
		t.Fatalf("cancelled event corrected: %v", err)
	}
	if _, err = f.correct("c", 3, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
		t.Fatal(err)
	}
	delete(active, "c")
	f.process(t)
	f.expect(t, active)
	if len(f.aggregates(t)) != 0 {
		t.Fatal("cancelled events left empty groups")
	}
	if f.count(t, "SELECT count(*) FROM metering_admission_groups") != 0 {
		t.Fatal("cancelled events kept admission capacity")
	}
	// The original and every correction remain append-only history.
	if n := f.count(t, "SELECT count(*) FROM metering_event_revisions r JOIN metering_events e ON e.id=r.event_id WHERE e.event_key='c'"); n != 4 {
		t.Fatalf("correction history = %d revisions", n)
	}
	var original string
	if err = f.db.Pool.QueryRow(ctx, "SELECT r.attributes->'latency'->>'value' FROM metering_event_revisions r JOIN metering_events e ON e.id=r.event_id WHERE e.event_key='c' AND r.revision=1").Scan(&original); err != nil || original != "0.3" {
		t.Fatalf("original revision changed: %s %v", original, err)
	}
	for _, statement := range []string{
		"UPDATE metering_event_revisions SET event_type='changed'",
		"DELETE FROM metering_event_revisions",
		"DELETE FROM metering_contributions",
		"UPDATE metering_events SET occurred_at=occurred_at-interval '1 day'",
		"UPDATE metering_events SET event_key=event_key||'-moved'",
		"UPDATE metering_events SET applied_revision=0",
		"DELETE FROM metering_events",
	} {
		if _, err = f.db.Pool.Exec(ctx, statement); err == nil {
			t.Fatalf("history mutation accepted: %s", statement)
		}
	}
}

func TestEventAverageRoundingAndRandomizedEquivalence(t *testing.T) {
	f := newMeterFixture(t, nil)
	now := time.Now().UTC().Add(-time.Minute)
	active := map[string]metering.Event{}
	for i, tc := range []struct{ input, want string }{
		{"0.0000005", "0.000001"},
		{"-0.0000005", "-0.000001"},
		{"0.00000049", "0"},
		{"-0.00000049", "0"},
		{"1.2345675", "1.234568"},
		{"-1.2345675", "-1.234568"},
	} {
		eventKey := fmt.Sprintf("probe-%d", i)
		group := fmt.Sprintf("case-%d", i)
		if _, err := f.ingest("service", eventKey, "probe", now, map[string]*pb.AttributeValue{"case": text(group), "n": num(tc.input)}); err != nil {
			t.Fatal(err)
		}
		n, _ := metering.Number(tc.input)
		active[eventKey] = metering.Event{Type: "probe", Attributes: map[string]metering.Value{"case": metering.String(group), "n": n}}
		f.process(t)
		if got := f.value(t, "probe-avg", map[string]metering.Value{"case": metering.String(group)}); got != tc.want {
			t.Fatalf("average of %s = %s, want %s", tc.input, got, tc.want)
		}
	}
	f.expect(t, active)

	// Random inserts, replacements, and cancellations with processing at random
	// points must match a full rebuild of the latest revisions.
	random := rand.New(rand.NewSource(81))
	regions := []string{"eu", "us", "ap"}
	outcomes := []string{"success", "cached", "failure"}
	revisions := map[string]int32{}
	randomRequest := func() request {
		return request{region: regions[random.Intn(len(regions))], outcome: outcomes[random.Intn(len(outcomes))], user: fmt.Sprintf("u%d", random.Intn(4)),
			bytes: fmt.Sprintf("%d.%d", random.Intn(21)-10, random.Intn(10)), latency: fmt.Sprintf("%d.%03d", random.Intn(7)-3, random.Intn(1000))}
	}
	for step := 0; step < 120; step++ {
		var keys []string
		for k := range revisions {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		switch choice := random.Intn(10); {
		case choice < 5 || len(keys) == 0:
			eventKey := fmt.Sprintf("random-%d", step)
			r := randomRequest()
			if _, err := f.ingest("service", eventKey, "request", now, r.wire()); err != nil {
				t.Fatal(err)
			}
			revisions[eventKey] = 1
			active[eventKey] = r.event(t)
		case choice < 8:
			eventKey := keys[random.Intn(len(keys))]
			r := randomRequest()
			response, err := f.correct(eventKey, revisions[eventKey], pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", r.wire())
			if err != nil {
				t.Fatal(err)
			}
			revisions[eventKey] = response.Revision
			active[eventKey] = r.event(t)
		default:
			eventKey := keys[random.Intn(len(keys))]
			if _, err := f.correct(eventKey, revisions[eventKey], pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
				t.Fatal(err)
			}
			delete(revisions, eventKey)
			delete(active, eventKey)
		}
		if random.Intn(4) == 0 {
			f.process(t)
			f.expect(t, active)
		}
	}
	f.process(t)
	f.expect(t, active)
	if pending := f.count(t, "SELECT count(*) FROM metering_events WHERE applied_revision<>latest_revision"); pending != 0 {
		t.Fatalf("%d events remained unprocessed", pending)
	}
}

func TestEventCardinalityAdmissionIncludesPendingWork(t *testing.T) {
	f := newMeterFixture(t, func(c *config.Config) { c.MaxMeterGroups = 2; c.MaxMeterGroupValues = 2 })
	now := time.Now().UTC().Add(-time.Minute)
	put := func(eventKey string, r request) error {
		_, err := f.ingest("service", eventKey, "request", now, r.wire())
		return err
	}
	for i, r := range []request{
		{region: "eu", outcome: "success", user: "u1", bytes: "1", latency: "1"},
		{region: "us", outcome: "success", user: "u1", bytes: "2", latency: "2"},
		{region: "eu", outcome: "success", user: "u2", bytes: "3", latency: "2"},
		// Sum and average keep only exact totals, so new amounts need no
		// distinct-value capacity; latency min/max values do, but stay at two.
		{region: "eu", outcome: "success", user: "u1", bytes: "4", latency: "1"},
	} {
		if err := put(fmt.Sprintf("ok-%d", i), r); err != nil {
			t.Fatalf("event %d within bounds rejected: %v", i, err)
		}
	}
	if pending := f.count(t, "SELECT count(*) FROM jobs WHERE kind='metering' AND state='pending'"); pending != 4 {
		t.Fatalf("admission test expected unprocessed work, got %d pending jobs", pending)
	}
	snapshot := func() int {
		return f.count(t, "SELECT count(*) FROM metering_events") + f.count(t, "SELECT COALESCE(sum(events),0)::int FROM metering_admission_groups") + f.count(t, "SELECT count(*) FROM metering_admission_values")
	}
	before := snapshot()
	for name, r := range map[string]request{
		"third group":                  {region: "ap", outcome: "success", user: "u1", bytes: "1", latency: "1"},
		"third distinct user":          {region: "eu", outcome: "success", user: "u3", bytes: "1", latency: "1"},
		"third distinct min/max value": {region: "eu", outcome: "success", user: "u1", bytes: "1", latency: "9"},
	} {
		if err := put("rejected-"+name, r); reasonOf(err) != pb.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED || connect.CodeOf(err) != connect.CodeResourceExhausted {
			t.Fatalf("%s accepted beyond bounds: %v", name, err)
		}
		if snapshot() != before {
			t.Fatalf("%s rejection left partial admission effects", name)
		}
	}
	// Moving the only us event to ap frees its group before claiming the new one.
	if _, err := f.correct("ok-1", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", request{region: "ap", outcome: "success", user: "u1", bytes: "2", latency: "2"}.wire()); err != nil {
		t.Fatalf("group move within bounds rejected: %v", err)
	}
	if err := put("rejected-us", request{region: "us", outcome: "success", user: "u1", bytes: "1", latency: "1"}); reasonOf(err) != pb.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED {
		t.Fatalf("pending replacement did not keep the destination group: %v", err)
	}
	f.process(t)
	if groups := len(f.aggregates(t)["requests"]); groups != 2 {
		t.Fatalf("materialized groups = %d", groups)
	}

	// Concurrent requests cannot claim more groups than remain.
	g := newMeterFixture(t, func(c *config.Config) { c.MaxMeterGroups = 3 })
	var wg sync.WaitGroup
	results := make(chan error, 8)
	for i := range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, err := g.ingest("service", fmt.Sprintf("concurrent-%d", i), "request", now, request{region: fmt.Sprintf("r%d", i), outcome: "success", user: "u", bytes: "1", latency: "1"}.wire())
			results <- err
		}()
	}
	wg.Wait()
	close(results)
	accepted := 0
	for err := range results {
		if err == nil {
			accepted++
		} else if reasonOf(err) != pb.ErrorReason_ERROR_REASON_CARDINALITY_EXCEEDED {
			t.Fatalf("unexpected concurrent rejection: %v", err)
		}
	}
	if accepted != 3 {
		t.Fatalf("concurrent admission accepted %d events into three group slots", accepted)
	}
	g.process(t)
	if groups := len(g.aggregates(t)["bytes"]); groups != 3 {
		t.Fatalf("materialized %d groups after concurrent admission", groups)
	}
}

func TestEventCorrectionLatenessAndDeletionGrace(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	occurred := time.Now().UTC().Add(-time.Hour)
	r := request{region: "eu", outcome: "success", user: "u1", bytes: "1", latency: "1"}
	if _, err := f.ingest("service", "late", "request", occurred, r.wire()); err != nil {
		t.Fatal(err)
	}
	f.setClock(occurred.Add(time.Duration(f.cfg.UsageLatenessSeconds)*time.Second + time.Second))
	if _, err := f.correct("late", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); reasonOf(err) != pb.ErrorReason_ERROR_REASON_REPORTING_WINDOW_CLOSED {
		t.Fatalf("correction outside the lateness bound accepted: %v", err)
	}
	f.setClock(time.Time{})
	if _, err := f.correct("late", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
		t.Fatalf("correction inside the lateness bound rejected: %v", err)
	}
	if _, err := f.client.DeleteTenant(ctx, req(&pb.DeleteTenantRequest{RequestKey: key(), TenantId: f.tenant}, "owner")); err != nil {
		t.Fatal(err)
	}
	if _, err := f.ingest("service", "after-deletion", "request", time.Now().UTC(), r.wire()); reasonOf(err) != pb.ErrorReason_ERROR_REASON_REPORTING_WINDOW_CLOSED {
		t.Fatalf("usage after deletion started was accepted: %v", err)
	}
	if _, err := f.ingest("service", "before-deletion", "request", occurred, r.wire()); err != nil {
		t.Fatalf("prior usage within the reporting grace rejected: %v", err)
	}
}

// Paid usage never falls back to a calendar month: it waits for the recorded
// Paddle period, because an accepted event's period is immutable.
func TestEventPaidUsageWaitsForRecordedPeriod(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	paid, err := f.client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "paid plan", Plan: &pb.Plan{Name: "paid", Rank: 10, Postpaid: true}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now().UTC()
	tx, err := f.db.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	var version string
	if err = tx.QueryRow(ctx, "SELECT active_version_id::text FROM catalog_entities WHERE id=$1", paid.Msg.Plan.Id).Scan(&version); err != nil {
		t.Fatal(err)
	}
	if _, err = tx.Exec(ctx, "UPDATE tenants SET plan_id=$2 WHERE id=$1", f.tenant, paid.Msg.Plan.Id); err != nil {
		t.Fatal(err)
	}
	if err = postgres.ReplaceTenantSelections(ctx, tx, f.tenant, []string{version}, now.Add(-2*time.Hour)); err != nil {
		t.Fatal(err)
	}
	if err = tx.Commit(ctx); err != nil {
		t.Fatal(err)
	}
	r := request{region: "eu", outcome: "success", user: "u1", bytes: "1", latency: "1"}
	occurred := now.Add(-time.Hour)
	rejected := func(label string) {
		t.Helper()
		_, err := f.ingest("service", "paid", "request", occurred, r.wire())
		if connect.CodeOf(err) != connect.CodeFailedPrecondition || reasonOf(err) != pb.ErrorReason_ERROR_REASON_PRECONDITION {
			t.Fatalf("%s: paid usage without a recorded period was not rejected: %v", label, err)
		}
		if n := f.count(t, "SELECT count(*) FROM metering_events WHERE tenant_id=$1", f.tenant); n != 0 {
			t.Fatalf("%s: rejected paid usage stored %d events", label, n)
		}
	}
	rejected("selected paid plan")
	// A tenant without a recorded selection is judged by its current plan row.
	if _, err = f.db.Pool.Exec(ctx, "DELETE FROM tenant_catalog_selections WHERE tenant_id=$1", f.tenant); err != nil {
		t.Fatal(err)
	}
	rejected("legacy paid plan")
	start := now.Add(-24 * time.Hour).Truncate(time.Second)
	if _, err = f.db.Pool.Exec(ctx, "INSERT INTO billing_periods VALUES($1,$2,$3,true)", f.tenant, start, start.AddDate(0, 1, 0)); err != nil {
		t.Fatal(err)
	}
	if _, err = f.ingest("service", "paid", "request", occurred, r.wire()); err != nil {
		t.Fatalf("paid usage inside a recorded period rejected: %v", err)
	}
	var stored time.Time
	if err = f.db.Pool.QueryRow(ctx, "SELECT period_start FROM metering_events WHERE tenant_id=$1", f.tenant).Scan(&stored); err != nil {
		t.Fatal(err)
	}
	if !stored.Equal(start) {
		t.Fatalf("paid usage period starts at %s, want the recorded period %s", stored, start)
	}
}

func TestMeteringWorkerCrashRetryAndOperatorRecovery(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	now := time.Now().UTC().Add(-time.Minute)
	r := request{region: "eu", outcome: "success", user: "u1", bytes: "2", latency: "1"}
	accepted, err := f.ingest("service", "crash", "request", now, r.wire())
	if err != nil {
		t.Fatal(err)
	}
	ready := func() {
		t.Helper()
		if _, err := f.db.Pool.Exec(ctx, "UPDATE jobs SET available_at=now()-interval '1 second' WHERE kind='metering' AND state='pending'"); err != nil {
			t.Fatal(err)
		}
	}
	job := func() (string, string, int) {
		t.Helper()
		var state, category string
		var attempts int
		if err := f.db.Pool.QueryRow(ctx, "SELECT state,error_category,attempts FROM jobs WHERE kind='metering' AND payload->>'event_id'=$1 ORDER BY created_at LIMIT 1", accepted.EventId).Scan(&state, &category, &attempts); err != nil {
			t.Fatal(err)
		}
		return state, category, attempts
	}
	// A crash before commit rolls back the aggregate writes with the job claim.
	f.worker.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error {
		return errors.New("simulated crash before commit")
	}
	f.process(t)
	if state, category, attempts := job(); state != "pending" || category != "metering_transient" || attempts != 1 {
		t.Fatalf("transient failure state = %s %s %d", state, category, attempts)
	}
	if len(f.aggregates(t)) != 0 || f.count(t, "SELECT applied_revision FROM metering_events WHERE id=$1", accepted.EventId) != 0 {
		t.Fatal("rolled back processing left aggregate effects")
	}
	var seen []events.Applied
	f.worker.MeteringApplied = func(_ context.Context, _ pgx.Tx, applied events.Applied) error {
		seen = append(seen, applied)
		return nil
	}
	ready()
	f.process(t)
	if state, _, _ := job(); state != "done" {
		t.Fatalf("retried work state = %s", state)
	}
	if len(seen) != 1 || seen[0].FromRevision != 0 || seen[0].ToRevision != 1 || len(seen[0].Changes) != 6 {
		t.Fatalf("hook did not receive the exact transition: %+v", seen)
	}
	for _, change := range seen[0].Changes {
		if change.Before != nil || change.After == nil || change.MeterID == "" {
			t.Fatalf("insert change = %+v", change)
		}
	}
	f.expect(t, map[string]metering.Event{"crash": r.event(t)})
	// Redelivered work after commit is idempotent.
	if _, err = f.db.Pool.Exec(ctx, `INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES($1,$2,'metering',$3,jsonb_build_object('event_id',$4::text),now()-interval '1 second',now(),now())`, key(), f.tenant, "redelivery:"+accepted.EventId, accepted.EventId); err != nil {
		t.Fatal(err)
	}
	f.process(t)
	if len(seen) != 2 || len(seen[1].Changes) != 0 {
		t.Fatalf("redelivered work changed aggregates: %+v", seen)
	}
	f.expect(t, map[string]metering.Event{"crash": r.event(t)})

	// Permanent failures stop retries and wait for an audited operator retry.
	replacement := r
	replacement.region = "us"
	if _, err = f.correct("crash", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "request", replacement.wire()); err != nil {
		t.Fatal(err)
	}
	f.worker.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error {
		return fmt.Errorf("simulated invalid state: %w", events.ErrPermanent)
	}
	f.process(t)
	var failedID string
	if err = f.db.Pool.QueryRow(ctx, "SELECT id::text FROM jobs WHERE kind='metering' AND state='failed' AND error_category='metering_invalid'").Scan(&failedID); err != nil {
		t.Fatalf("permanent failure was not classified: %v", err)
	}
	jobs, err := f.client.ListJobs(ctx, req(&pb.ListJobsRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, j := range jobs.Msg.Jobs {
		found = found || j.Id == failedID && j.State == pb.JobState_JOB_STATE_FAILED && j.Kind == events.JobKind
	}
	if !found {
		t.Fatal("failed metering work is not inspectable")
	}
	if _, err = f.client.RetryJob(ctx, req(&pb.RetryJobRequest{RequestKey: key(), JobId: failedID, Reason: "fixed processing state"}, "member")); connect.CodeOf(err) != connect.CodePermissionDenied {
		t.Fatalf("non-operator retried work: %v", err)
	}
	if _, err = f.client.RetryJob(ctx, req(&pb.RetryJobRequest{RequestKey: key(), JobId: failedID, Reason: "fixed processing state"}, "owner")); err != nil {
		t.Fatal(err)
	}
	if f.count(t, "SELECT count(*) FROM audit WHERE action='job.retry'") != 1 {
		t.Fatal("operator recovery was not audited")
	}
	f.worker.MeteringApplied = nil
	f.process(t)
	var state string
	if err = f.db.Pool.QueryRow(ctx, "SELECT state FROM jobs WHERE id=$1", failedID).Scan(&state); err != nil || state != "done" {
		t.Fatalf("operator retry was not processed: %s %v", state, err)
	}
	f.expect(t, map[string]metering.Event{"crash": replacement.event(t)})

	// Transient failures are bounded by the retry limit.
	if _, err = f.correct("crash", 2, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
		t.Fatal(err)
	}
	f.worker.MeteringApplied = func(context.Context, pgx.Tx, events.Applied) error { return errors.New("dependency unavailable") }
	for range f.cfg.RetryLimit {
		ready()
		f.process(t)
	}
	if n := f.count(t, "SELECT count(*) FROM jobs WHERE kind='metering' AND state='failed' AND error_category='retry_exhausted' AND attempts=$1", f.cfg.RetryLimit); n != 1 {
		t.Fatalf("retry exhaustion was not recorded: %d", n)
	}
	f.expect(t, map[string]metering.Event{"crash": replacement.event(t)})
}

func TestMeteringRetentionKeepsPendingWorkAndReplayIdentity(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	org, err := f.client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "Metered organization"}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	f.tenant = org.Msg.Tenant.Id
	occurred := time.Now().UTC().Add(-time.Minute)
	r := request{region: "eu", outcome: "success", user: "u1", bytes: "1", latency: "1"}
	accepted, err := f.ingest("service", "retained", "request", occurred, r.wire())
	if err != nil {
		t.Fatal(err)
	}
	if _, err = f.client.DeleteTenant(ctx, req(&pb.DeleteTenantRequest{RequestKey: key(), TenantId: f.tenant}, "owner")); err != nil {
		t.Fatal(err)
	}
	cfg := f.cfg
	cfg.DeletionGraceSeconds = 1
	cfg.RawUsageRetentionSeconds = 3600
	cfg.AccountingRetentionSeconds = 3600
	cfg.AuditRetentionSeconds = 3600
	w := &worker.Worker{DB: f.db, Config: cfg, Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
	future := time.Now().UTC().Add(48 * time.Hour)
	deleted := func() bool {
		t.Helper()
		return f.count(t, "SELECT count(*) FROM tenants WHERE id=$1 AND deleted_at IS NOT NULL", f.tenant) == 1
	}
	if err = w.Maintain(ctx, future); err != nil {
		t.Fatal(err)
	}
	if deleted() {
		t.Fatal("pending metering work did not hold final deletion")
	}
	f.process(t)
	if err = w.Maintain(ctx, future); err != nil {
		t.Fatal(err)
	}
	if !deleted() || f.count(t, "SELECT count(*) FROM metering_events WHERE id=$1", accepted.EventId) != 1 {
		t.Fatal("deletion did not finish or removed event history before retention")
	}
	if err = w.Maintain(ctx, future.Add(2*time.Hour)); err != nil {
		t.Fatal(err)
	}
	for _, table := range []string{"metering_events", "metering_event_revisions", "metering_contributions", "metering_aggregates", "metering_admission_groups"} {
		if n := f.count(t, "SELECT count(*) FROM "+table+" WHERE tenant_id=$1", f.tenant); n != 0 {
			t.Fatalf("%s retained %d rows after retention", table, n)
		}
	}
	if f.count(t, "SELECT count(*) FROM metering_replay WHERE event_id=$1", accepted.EventId) != 1 {
		t.Fatal("replay identity was not retained")
	}
	again, err := f.ingest("service", "retained", "request", occurred, r.wire())
	if err != nil || again.EventId != accepted.EventId {
		t.Fatalf("replay after retention repeated effects: %v %v", again, err)
	}
	changed := r
	changed.bytes = "2"
	if _, err = f.ingest("service", "retained", "request", occurred, changed.wire()); reasonOf(err) != pb.ErrorReason_ERROR_REASON_EVENT_IDENTITY_CONFLICT {
		t.Fatalf("conflict after retention accepted: %v", err)
	}
}

// Group keys of several maximum-size attributes exceed the PostgreSQL B-tree
// entry limit, so admission and aggregate rows must index a bounded digest.
func TestEventOversizedGroupKeysUseBoundedIdentity(t *testing.T) {
	f := newMeterFixture(t, nil)
	ctx := context.Background()
	for _, m := range []*pb.Meter{
		{Name: "wide-max", EventType: "wide", Aggregation: pb.MeterAggregation_METER_AGGREGATION_MAX, ValueAttribute: "n", GroupBy: []string{"a", "b", "c"}},
		{Name: "wide-users", EventType: "wide", Aggregation: pb.MeterAggregation_METER_AGGREGATION_UNIQUE_COUNT, ValueAttribute: "u", GroupBy: []string{"a", "b", "c"}},
	} {
		m.ServiceId, m.Key = f.service, m.Name
		if _, err := f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare wide meter", Meter: m}, "owner")); err != nil {
			t.Fatal(err)
		}
		definition, err := events.CatalogDefinition(m.Name, m.EventType, m.Aggregation, m.ValueAttribute, m.Filters, m.GroupBy)
		if err != nil {
			t.Fatal(err)
		}
		f.definitions = append(f.definitions, definition)
	}
	// Random characters that JSON escapes keep the encoded key large and
	// incompressible, well above the 2,704-byte B-tree entry limit.
	random := rand.New(rand.NewSource(81))
	const alphabet = `abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789<>&"\`
	long := func() string {
		b := make([]byte, events.MaxAttributeValueBytes)
		for i := range b {
			b[i] = alphabet[random.Intn(len(alphabet))]
		}
		return string(b)
	}
	group := func() map[string]string { return map[string]string{"a": long(), "b": long(), "c": long()} }
	first, second := group(), group()
	if groupKey, err := metering.GroupKey(map[string]metering.Value{"a": metering.String(first["a"]), "b": metering.String(first["b"]), "c": metering.String(first["c"])}); err != nil || len(groupKey) <= 2704 {
		t.Fatalf("test group key has %d bytes: %v", len(groupKey), err)
	}
	now := time.Now().UTC().Add(-time.Minute)
	active := map[string]metering.Event{}
	wide := func(g map[string]string, n, user string) (map[string]*pb.AttributeValue, metering.Event) {
		wire := map[string]*pb.AttributeValue{"a": text(g["a"]), "b": text(g["b"]), "c": text(g["c"]), "n": num(n), "u": text(user)}
		attributes := map[string]metering.Value{"a": metering.String(g["a"]), "b": metering.String(g["b"]), "c": metering.String(g["c"]), "u": metering.String(user)}
		number, err := metering.Number(n)
		if err != nil {
			t.Fatal(err)
		}
		attributes["n"] = number
		return wire, metering.Event{Type: "wide", Attributes: attributes}
	}
	for i, value := range []struct{ n, user string }{{"5", long()}, {"7", long()}} {
		wire, event := wide(first, value.n, value.user)
		eventKey := fmt.Sprintf("wide-%d", i)
		if _, err := f.ingest("service", eventKey, "wide", now, wire); err != nil {
			t.Fatalf("event with an oversized group key rejected: %v", err)
		}
		active[eventKey] = event
	}
	f.process(t)
	f.expect(t, active)
	firstValues := map[string]metering.Value{"a": metering.String(first["a"]), "b": metering.String(first["b"]), "c": metering.String(first["c"])}
	if f.value(t, "wide-max", firstValues) != "7" || f.value(t, "wide-users", firstValues) != "2" {
		t.Fatal("oversized group aggregates are wrong")
	}
	// Moving and cancelling events releases admission and aggregate rows by the same identity.
	wire, event := wide(second, "3", long())
	if _, err := f.correct("wide-0", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "wide", wire); err != nil {
		t.Fatalf("move between oversized groups rejected: %v", err)
	}
	active["wide-0"] = event
	f.process(t)
	f.expect(t, active)
	if _, err := f.correct("wide-1", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil); err != nil {
		t.Fatal(err)
	}
	delete(active, "wide-1")
	f.process(t)
	f.expect(t, active)
	if f.value(t, "wide-max", firstValues) != "" {
		t.Fatal("cancelled oversized group retained an aggregate")
	}
	if groups := f.count(t, "SELECT count(*) FROM metering_admission_groups a JOIN catalog_entities ce ON ce.id=a.meter_id WHERE ce.key IN ('wide-max','wide-users')"); groups != 2 {
		t.Fatalf("admission groups = %d after moving and cancelling oversized groups", groups)
	}
}
