// SPDX-License-Identifier: Apache-2.0

package main

import (
	"bytes"
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
)

// fakeDelcore models the asynchronous contract: ingestion is acknowledged
// before aggregates change, the first ingestion attempt is lost in transit,
// and the first correction meets a concurrent correction.
type fakeDelcore struct {
	delcorev1connect.UnimplementedDelcoreServiceHandler
	mu            sync.Mutex
	calls         []string
	ingestKeys    map[string]int
	eventKeys     map[string]bool
	revision      int32
	staleReturned bool
	allowed       bool
}

func (f *fakeDelcore) record(name string, h http.Header) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, name)
	if !strings.HasPrefix(h.Get("Authorization"), "Bearer ") {
		return connect.NewError(connect.CodeUnauthenticated, errors.New("missing token"))
	}
	return nil
}

func stale() error {
	err := connect.NewError(connect.CodeAborted, errors.New("expected revision is not the latest revision"))
	detail, _ := connect.NewErrorDetail(&pb.ErrorDetail{Reason: pb.ErrorReason_ERROR_REASON_STALE_REVISION})
	err.AddDetail(detail)
	return err
}

func (f *fakeDelcore) GetCurrentUser(_ context.Context, r *connect.Request[pb.GetCurrentUserRequest]) (*connect.Response[pb.GetCurrentUserResponse], error) {
	if e := f.record("GetCurrentUser", r.Header()); e != nil {
		return nil, e
	}
	return connect.NewResponse(&pb.GetCurrentUserResponse{UserId: "user-1", Tenants: []*pb.Tenant{{Id: "tenant-1"}}, Metadata: &pb.Metadata{CorrelationId: "c-1"}}), nil
}

func (f *fakeDelcore) Authorize(_ context.Context, r *connect.Request[pb.AuthorizeRequest]) (*connect.Response[pb.AuthorizeResponse], error) {
	if e := f.record("Authorize", r.Header()); e != nil {
		return nil, e
	}
	if r.Msg.FeatureKey != "monthly-requests" || r.Msg.GroupValues["region"].GetText() != "us-east" || r.Msg.TenantId != "tenant-1" || r.Msg.UserId != "user-1" {
		return nil, connect.NewError(connect.CodeInvalidArgument, errors.New("unexpected authorization request"))
	}
	response := &pb.AuthorizeResponse{Allowed: f.allowed, Feature: &pb.FeatureAuthorization{Entitled: true, Usage: "999", Limit: "1000"}}
	if !f.allowed {
		response.Reasons = []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED}
	}
	return connect.NewResponse(response), nil
}

func (f *fakeDelcore) IngestEvent(_ context.Context, r *connect.Request[pb.IngestEventRequest]) (*connect.Response[pb.IngestEventResponse], error) {
	if e := f.record("IngestEvent", r.Header()); e != nil {
		return nil, e
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	f.ingestKeys[r.Msg.RequestKey]++
	f.eventKeys[r.Msg.Event.EventKey] = true
	if r.Msg.Event.ServiceId != "service-1" || r.Msg.Event.Attributes["status"].GetNumber() != "200" {
		return nil, connect.NewError(connect.CodeInvalidArgument, errors.New("unexpected event"))
	}
	if f.ingestKeys[r.Msg.RequestKey] == 1 {
		return nil, connect.NewError(connect.CodeUnavailable, errors.New("lost in transit"))
	}
	f.revision = 1
	return connect.NewResponse(&pb.IngestEventResponse{EventId: "event-1", Revision: 1}), nil
}

func (f *fakeDelcore) CorrectEvent(_ context.Context, r *connect.Request[pb.CorrectEventRequest]) (*connect.Response[pb.CorrectEventResponse], error) {
	if e := f.record("CorrectEvent", r.Header()); e != nil {
		return nil, e
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if !f.staleReturned {
		// Another correction of the same event won first.
		f.staleReturned = true
		f.revision = 2
		return nil, stale()
	}
	if r.Msg.ExpectedRevision != f.revision || r.Msg.Attributes["outcome"].GetText() != "cached" {
		return nil, stale()
	}
	f.revision++
	return connect.NewResponse(&pb.CorrectEventResponse{EventId: "event-1", Revision: f.revision}), nil
}

func (f *fakeDelcore) GetEvent(_ context.Context, r *connect.Request[pb.GetEventRequest]) (*connect.Response[pb.GetEventResponse], error) {
	if e := f.record("GetEvent", r.Header()); e != nil {
		return nil, e
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	return connect.NewResponse(&pb.GetEventResponse{Revision: f.revision}), nil
}

func (f *fakeDelcore) ListMeterAggregates(_ context.Context, r *connect.Request[pb.ListMeterAggregatesRequest]) (*connect.Response[pb.ListMeterAggregatesResponse], error) {
	if e := f.record("ListMeterAggregates", r.Header()); e != nil {
		return nil, e
	}
	// Processing has not caught up: the accepted event is not visible yet.
	return connect.NewResponse(&pb.ListMeterAggregatesResponse{}), nil
}

func (f *fakeDelcore) GetMeteringStatus(_ context.Context, r *connect.Request[pb.GetMeteringStatusRequest]) (*connect.Response[pb.GetMeteringStatusResponse], error) {
	if e := f.record("GetMeteringStatus", r.Header()); e != nil {
		return nil, e
	}
	return connect.NewResponse(&pb.GetMeteringStatusResponse{UnappliedEvents: 1, PendingJobs: 3, LagSeconds: 2}), nil
}

func start(t *testing.T, fake *fakeDelcore) delcorev1connect.DelcoreServiceClient {
	t.Helper()
	path, handler := delcorev1connect.NewDelcoreServiceHandler(fake)
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	server := httptest.NewServer(mux)
	t.Cleanup(server.Close)
	return delcorev1connect.NewDelcoreServiceClient(server.Client(), server.URL)
}

func TestMeteringFlowRetriesWithStableKeysAndRereadsStaleRevisions(t *testing.T) {
	fake := &fakeDelcore{ingestKeys: map[string]int{}, eventKeys: map[string]bool{}, allowed: true}
	var out bytes.Buffer
	o := options{accessToken: "user-secret", serviceToken: "service-secret", serviceID: "service-1", featureKey: "monthly-requests", meterKey: "requests", region: "us-east", attempts: 3, retryDelay: time.Millisecond}
	if err := run(context.Background(), start(t, fake), o, &out); err != nil {
		t.Fatal(err, out.String())
	}
	want := []string{"GetCurrentUser", "Authorize", "IngestEvent", "IngestEvent", "CorrectEvent", "GetEvent", "CorrectEvent", "ListMeterAggregates", "GetMeteringStatus"}
	if strings.Join(fake.calls, ",") != strings.Join(want, ",") {
		t.Fatalf("calls = %v, want %v", fake.calls, want)
	}
	if len(fake.ingestKeys) != 1 || len(fake.eventKeys) != 1 {
		t.Fatalf("retry changed request or event keys: %v %v", fake.ingestKeys, fake.eventKeys)
	}
	text := out.String()
	for _, expected := range []string{"aggregates update asynchronously", "Stale revision 1; latest is 2", "CorrectEvent accepted revision 3", "returned 0 groups", "not applied yet"} {
		if !strings.Contains(text, expected) {
			t.Fatalf("output lacks %q:\n%s", expected, text)
		}
	}
	if strings.Contains(text, "secret") {
		t.Fatal("output exposes a token")
	}
}

func TestBlockedAuthorizationReportsNoEvent(t *testing.T) {
	fake := &fakeDelcore{ingestKeys: map[string]int{}, eventKeys: map[string]bool{}}
	var out bytes.Buffer
	o := options{serviceToken: "service-secret", serviceID: "service-1", tenantID: "tenant-1", userID: "user-1", featureKey: "monthly-requests", meterKey: "requests", region: "us-east", attempts: 1}
	if err := run(context.Background(), start(t, fake), o, &out); err != nil {
		t.Fatal(err)
	}
	if strings.Join(fake.calls, ",") != "Authorize" || !strings.Contains(out.String(), "Execution blocked") {
		t.Fatalf("blocked execution reported usage: %v %s", fake.calls, out.String())
	}
}
