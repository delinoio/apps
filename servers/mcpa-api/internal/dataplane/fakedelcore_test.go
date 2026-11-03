// SPDX-License-Identifier: FSL-1.1-ALv2

package dataplane

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	"golang.org/x/oauth2"
	"google.golang.org/protobuf/types/known/timestamppb"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
)

// fakeDelcore is a scriptable Delcore Connect server.
type fakeDelcore struct {
	delcorev1connect.UnimplementedDelcoreServiceHandler
	srv *httptest.Server

	mu sync.Mutex
	// down makes every RPC fail with UNAVAILABLE.
	down bool
	// periodDown makes only ListMeterAggregates (the billing period read)
	// fail with UNAVAILABLE.
	periodDown bool
	// states maps tenant IDs to states; missing tenants are ACTIVE.
	states map[string]delcorev1.TenantState
	// denied maps "tenant/user/capability" to block reasons.
	denied map[string][]delcorev1.BlockReason
	// keys maps personal key secrets to owner user and tenant.
	keys map[string][2]string
	// ingestErr, when set, answers IngestEvent.
	ingestErr func(*delcorev1.Event) error
	events    []*delcorev1.Event
	requests  []string
	// attempts counts IngestEvent calls per event key, including failures
	// and replays.
	attempts  map[string]int
	period    delcore.Period
	finalized map[string]bool
	calls     map[string]int
}

func newFakeDelcore(t testing.TB) *fakeDelcore {
	t.Helper()
	f := &fakeDelcore{
		states: map[string]delcorev1.TenantState{}, denied: map[string][]delcorev1.BlockReason{},
		keys: map[string][2]string{}, finalized: map[string]bool{}, calls: map[string]int{}, attempts: map[string]int{},
		period: delcore.Period{Start: time.Now().UTC().Add(-24 * time.Hour), End: time.Now().UTC().Add(29 * 24 * time.Hour)},
	}
	path, handler := delcorev1connect.NewDelcoreServiceHandler(f)
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	f.srv = httptest.NewServer(mux)
	t.Cleanup(f.srv.Close)
	return f
}

// client returns a Delcore client for the fake with the given cache TTL.
func (f *fakeDelcore) client(t testing.TB, ttl time.Duration, now func() time.Time) *delcore.Client {
	t.Helper()
	c, err := delcore.New(delcore.Config{
		BaseURL: f.srv.URL, ServiceID: "019a0000-0000-7000-8000-000000000001", Timeout: 2 * time.Second, CacheTTL: ttl, Now: now,
		TokenSource: oauth2.StaticTokenSource(&oauth2.Token{AccessToken: "m2m"}),
	})
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func (f *fakeDelcore) set(fn func(f *fakeDelcore)) {
	f.mu.Lock()
	defer f.mu.Unlock()
	fn(f)
}

func (f *fakeDelcore) enter(rpc string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls[rpc]++
	if f.down {
		return connect.NewError(connect.CodeUnavailable, errors.New("delcore is down"))
	}
	return nil
}

func (f *fakeDelcore) count(rpc string) int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.calls[rpc]
}

func (f *fakeDelcore) ingested() []*delcorev1.Event {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]*delcorev1.Event(nil), f.events...)
}

func reasonError(code connect.Code, reason delcorev1.ErrorReason) error {
	err := connect.NewError(code, errors.New(strings.ToLower(reason.String())))
	if detail, derr := connect.NewErrorDetail(&delcorev1.ErrorDetail{Reason: reason}); derr == nil {
		err.AddDetail(detail)
	}
	return err
}

func (f *fakeDelcore) Authorize(_ context.Context, req *connect.Request[delcorev1.AuthorizeRequest]) (*connect.Response[delcorev1.AuthorizeResponse], error) {
	if err := f.enter("Authorize"); err != nil {
		return nil, err
	}
	m := req.Msg
	f.mu.Lock()
	reasons := f.denied[m.GetTenantId()+"/"+m.GetUserId()+"/"+m.GetCapability()]
	f.mu.Unlock()
	return connect.NewResponse(&delcorev1.AuthorizeResponse{Allowed: len(reasons) == 0, Reasons: reasons}), nil
}

func (f *fakeDelcore) AuthorizeApiKey(_ context.Context, req *connect.Request[delcorev1.AuthorizeApiKeyRequest]) (*connect.Response[delcorev1.AuthorizeApiKeyResponse], error) {
	if err := f.enter("AuthorizeApiKey"); err != nil {
		return nil, err
	}
	f.mu.Lock()
	owner, ok := f.keys[req.Msg.GetSecret()]
	var reasons []delcorev1.BlockReason
	if ok {
		reasons = f.denied[owner[1]+"/"+owner[0]+"/"+req.Msg.GetCapability()]
	}
	f.mu.Unlock()
	if !ok {
		return nil, connect.NewError(connect.CodeUnauthenticated, errors.New("invalid key"))
	}
	return connect.NewResponse(&delcorev1.AuthorizeApiKeyResponse{
		Allowed: len(reasons) == 0, Reasons: reasons, KeyId: "key-" + owner[0], TenantId: owner[1],
		OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL, UserId: owner[0],
	}), nil
}

func (f *fakeDelcore) GetTenantStates(_ context.Context, req *connect.Request[delcorev1.GetTenantStatesRequest]) (*connect.Response[delcorev1.GetTenantStatesResponse], error) {
	if err := f.enter("GetTenantStates"); err != nil {
		return nil, err
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	out := &delcorev1.GetTenantStatesResponse{}
	for _, id := range req.Msg.GetTenantIds() {
		state, ok := f.states[id]
		if !ok {
			state = delcorev1.TenantState_TENANT_STATE_ACTIVE
		}
		out.States = append(out.States, &delcorev1.TenantStateEntry{TenantId: id, State: state})
	}
	return connect.NewResponse(out), nil
}

func (f *fakeDelcore) CheckTenantLifecycle(_ context.Context, req *connect.Request[delcorev1.CheckTenantLifecycleRequest]) (*connect.Response[delcorev1.CheckTenantLifecycleResponse], error) {
	if err := f.enter("CheckTenantLifecycle"); err != nil {
		return nil, err
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	out := &delcorev1.CheckTenantLifecycleResponse{}
	for _, id := range req.Msg.GetTenantIds() {
		if f.finalized[id] {
			out.FinalizedTenantIds = append(out.FinalizedTenantIds, id)
		}
	}
	return connect.NewResponse(out), nil
}

func (f *fakeDelcore) ListMeterAggregates(_ context.Context, req *connect.Request[delcorev1.ListMeterAggregatesRequest]) (*connect.Response[delcorev1.ListMeterAggregatesResponse], error) {
	if err := f.enter("ListMeterAggregates"); err != nil {
		return nil, err
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.periodDown {
		return nil, connect.NewError(connect.CodeUnavailable, errors.New("billing period is unavailable"))
	}
	return connect.NewResponse(&delcorev1.ListMeterAggregatesResponse{
		PeriodStart: timestamppb.New(f.period.Start), PeriodEnd: timestamppb.New(f.period.End),
	}), nil
}

func (f *fakeDelcore) IngestEvent(_ context.Context, req *connect.Request[delcorev1.IngestEventRequest]) (*connect.Response[delcorev1.IngestEventResponse], error) {
	f.mu.Lock()
	f.attempts[req.Msg.GetEvent().GetEventKey()]++
	f.mu.Unlock()
	if err := f.enter("IngestEvent"); err != nil {
		return nil, err
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.ingestErr != nil {
		if err := f.ingestErr(req.Msg.GetEvent()); err != nil {
			return nil, err
		}
	}
	for _, e := range f.events {
		if e.GetEventKey() == req.Msg.GetEvent().GetEventKey() {
			// Identical replay: the original identity, no new event.
			return connect.NewResponse(&delcorev1.IngestEventResponse{EventId: "evt", Revision: 1}), nil
		}
	}
	f.events = append(f.events, req.Msg.GetEvent())
	f.requests = append(f.requests, req.Msg.GetRequestKey())
	return connect.NewResponse(&delcorev1.IngestEventResponse{EventId: "evt", Revision: 1}), nil
}

func (f *fakeDelcore) GetCurrentUser(context.Context, *connect.Request[delcorev1.GetCurrentUserRequest]) (*connect.Response[delcorev1.GetCurrentUserResponse], error) {
	return nil, connect.NewError(connect.CodeUnauthenticated, errors.New("user tokens are not used in these tests"))
}
