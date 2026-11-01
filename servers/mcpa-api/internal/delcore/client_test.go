// SPDX-License-Identifier: FSL-1.1-ALv2

package delcore

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"connectrpc.com/connect"
	"golang.org/x/oauth2"
	"google.golang.org/protobuf/types/known/timestamppb"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
)

// fakeDelcore is a scriptable Delcore server.
type fakeDelcore struct {
	delcorev1connect.UnimplementedDelcoreServiceHandler
	mu          sync.Mutex
	allowed     bool
	down        bool
	state       delcorev1.TenantState
	calls       map[string]int
	authHeaders []string
}

func (f *fakeDelcore) record(name string, h http.Header) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.calls == nil {
		f.calls = map[string]int{}
	}
	f.calls[name]++
	f.authHeaders = append(f.authHeaders, h.Get("Authorization"))
	if f.down {
		return connect.NewError(connect.CodeUnavailable, errors.New("down"))
	}
	return nil
}

func (f *fakeDelcore) count(name string) int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.calls[name]
}

func (f *fakeDelcore) set(fn func(*fakeDelcore)) {
	f.mu.Lock()
	defer f.mu.Unlock()
	fn(f)
}

func (f *fakeDelcore) Authorize(_ context.Context, req *connect.Request[delcorev1.AuthorizeRequest]) (*connect.Response[delcorev1.AuthorizeResponse], error) {
	if err := f.record("Authorize", req.Header()); err != nil {
		return nil, err
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	resp := &delcorev1.AuthorizeResponse{Allowed: f.allowed}
	if !f.allowed {
		resp.Reasons = []delcorev1.BlockReason{delcorev1.BlockReason_BLOCK_REASON_PERMISSION}
	}
	return connect.NewResponse(resp), nil
}

func (f *fakeDelcore) AuthorizeApiKey(_ context.Context, req *connect.Request[delcorev1.AuthorizeApiKeyRequest]) (*connect.Response[delcorev1.AuthorizeApiKeyResponse], error) {
	if err := f.record("AuthorizeApiKey", req.Header()); err != nil {
		return nil, err
	}
	if req.Msg.GetSecret() != "dlk_good" {
		return nil, connect.NewError(connect.CodeUnauthenticated, errors.New("invalid key"))
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	return connect.NewResponse(&delcorev1.AuthorizeApiKeyResponse{
		Allowed: f.allowed, KeyId: "k", TenantId: "t", UserId: "u", OwnerKind: delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL,
	}), nil
}

func (f *fakeDelcore) GetTenantStates(_ context.Context, req *connect.Request[delcorev1.GetTenantStatesRequest]) (*connect.Response[delcorev1.GetTenantStatesResponse], error) {
	if err := f.record("GetTenantStates", req.Header()); err != nil {
		return nil, err
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	var states []*delcorev1.TenantStateEntry
	for _, id := range req.Msg.GetTenantIds() {
		states = append(states, &delcorev1.TenantStateEntry{TenantId: id, State: f.state})
	}
	return connect.NewResponse(&delcorev1.GetTenantStatesResponse{States: states}), nil
}

func (f *fakeDelcore) GetCurrentUser(_ context.Context, req *connect.Request[delcorev1.GetCurrentUserRequest]) (*connect.Response[delcorev1.GetCurrentUserResponse], error) {
	if err := f.record("GetCurrentUser", req.Header()); err != nil {
		return nil, err
	}
	if req.Msg.GetPage().GetAfterId() == "" {
		return connect.NewResponse(&delcorev1.GetCurrentUserResponse{UserId: "user-1", Tenants: []*delcorev1.Tenant{{Id: "t1"}}, NextAfterId: "t1"}), nil
	}
	return connect.NewResponse(&delcorev1.GetCurrentUserResponse{UserId: "user-1", Tenants: []*delcorev1.Tenant{{Id: "t2"}}}), nil
}

func (f *fakeDelcore) ListGroups(_ context.Context, req *connect.Request[delcorev1.ListGroupsRequest]) (*connect.Response[delcorev1.ListGroupsResponse], error) {
	if err := f.record("ListGroups", req.Header()); err != nil {
		return nil, err
	}
	if req.Msg.GetPage().GetAfterId() == "" {
		return connect.NewResponse(&delcorev1.ListGroupsResponse{Groups: []*delcorev1.Group{{Id: "g1", UserIds: []string{"u"}}}, NextAfterId: "g1"}), nil
	}
	return connect.NewResponse(&delcorev1.ListGroupsResponse{Groups: []*delcorev1.Group{{Id: "g2"}}}), nil
}

func (f *fakeDelcore) ListMeterAggregates(_ context.Context, req *connect.Request[delcorev1.ListMeterAggregatesRequest]) (*connect.Response[delcorev1.ListMeterAggregatesResponse], error) {
	if err := f.record("ListMeterAggregates", req.Header()); err != nil {
		return nil, err
	}
	if req.Msg.GetServiceId() != "svc" || req.Msg.GetMeterKey() != "mcpa-traffic-bytes" {
		return nil, connect.NewError(connect.CodeInvalidArgument, errors.New("bad meter"))
	}
	return connect.NewResponse(&delcorev1.ListMeterAggregatesResponse{
		PeriodStart: timestamppb.New(time.Date(2026, 11, 1, 0, 0, 0, 0, time.UTC)),
		PeriodEnd:   timestamppb.New(time.Date(2026, 12, 1, 0, 0, 0, 0, time.UTC)),
	}), nil
}

type clock struct{ t atomic.Int64 }

func (c *clock) now() time.Time          { return time.Unix(0, c.t.Load()) }
func (c *clock) advance(d time.Duration) { c.t.Add(int64(d)) }

func newTestClient(t *testing.T, f *fakeDelcore) (*Client, *clock) {
	t.Helper()
	_, handler := delcorev1connect.NewDelcoreServiceHandler(f)
	srv := httptest.NewServer(handler)
	t.Cleanup(srv.Close)
	clk := &clock{}
	clk.t.Store(time.Date(2026, 11, 1, 12, 0, 0, 0, time.UTC).UnixNano())
	c, err := New(Config{
		BaseURL:     srv.URL,
		ServiceID:   "svc",
		Timeout:     2 * time.Second,
		CacheTTL:    30 * time.Second,
		HTTPClient:  srv.Client(),
		Now:         clk.now,
		TokenSource: oauth2.StaticTokenSource(&oauth2.Token{AccessToken: "m2m-token"}),
	})
	if err != nil {
		t.Fatal(err)
	}
	return c, clk
}

func TestAuthorizeCachesOnlyAllowed(t *testing.T) {
	f := &fakeDelcore{allowed: false}
	c, clk := newTestClient(t, f)
	ctx := context.Background()

	for range 2 {
		d, err := c.Authorize(ctx, "t", "u", CapabilityAccessResources, false)
		if err != nil || d.Allowed {
			t.Fatalf("denial: %+v %v", d, err)
		}
	}
	if n := f.count("Authorize"); n != 2 {
		t.Fatalf("denials must not be cached: %d calls", n)
	}

	f.set(func(f *fakeDelcore) { f.allowed = true })
	for range 3 {
		if d, err := c.Authorize(ctx, "t", "u", CapabilityAccessResources, false); err != nil || !d.Allowed {
			t.Fatalf("allowed: %+v %v", d, err)
		}
	}
	if n := f.count("Authorize"); n != 3 {
		t.Fatalf("allowed result must be cached: %d calls", n)
	}
	// Another capability or paid flag is a different cache entry.
	if _, err := c.Authorize(ctx, "t", "u", CapabilityAccessResources, true); err != nil {
		t.Fatal(err)
	}
	if n := f.count("Authorize"); n != 4 {
		t.Fatalf("paid must not share the cache entry: %d calls", n)
	}

	// After expiry with Delcore down, the call fails closed.
	f.set(func(f *fakeDelcore) { f.down = true })
	if d, err := c.Authorize(ctx, "t", "u", CapabilityAccessResources, false); err != nil || !d.Allowed {
		t.Fatalf("within TTL the cached result applies: %+v %v", d, err)
	}
	clk.advance(31 * time.Second)
	d, err := c.Authorize(ctx, "t", "u", CapabilityAccessResources, false)
	if err == nil || d.Allowed {
		t.Fatalf("expired entry with Delcore down must fail closed: %+v %v", d, err)
	}
	var de *Error
	if !errors.As(err, &de) || de.Kind != KindUnavailable {
		t.Fatalf("error kind: %v", err)
	}
	// Errors are not cached either.
	f.set(func(f *fakeDelcore) { f.down = false })
	if d, err := c.Authorize(ctx, "t", "u", CapabilityAccessResources, false); err != nil || !d.Allowed {
		t.Fatalf("recovery: %+v %v", d, err)
	}
}

func TestM2MTokenOnServiceCallsOnly(t *testing.T) {
	f := &fakeDelcore{allowed: true}
	c, _ := newTestClient(t, f)
	ctx := context.Background()
	if _, err := c.Authorize(ctx, "t", "u", CapabilityCreateResources, false); err != nil {
		t.Fatal(err)
	}
	user, err := c.GetCurrentUser(ctx, "user-token")
	if err != nil {
		t.Fatal(err)
	}
	if user.UserID != "user-1" || len(user.Tenants) != 2 {
		t.Fatalf("all pages: %+v", user)
	}
	if _, err := c.GetCurrentUser(ctx, ""); err == nil {
		t.Fatal("empty user token must not fall back to the service identity")
	}
	groups, err := c.ListGroups(ctx, "user-token", "t")
	if err != nil || len(groups) != 2 {
		t.Fatalf("groups: %v %v", groups, err)
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	want := []string{"Bearer m2m-token", "Bearer user-token", "Bearer user-token", "Bearer user-token", "Bearer user-token"}
	if len(f.authHeaders) != len(want) {
		t.Fatalf("headers: %v", f.authHeaders)
	}
	for i := range want {
		if f.authHeaders[i] != want[i] {
			t.Fatalf("header %d = %q, want %q", i, f.authHeaders[i], want[i])
		}
	}
}

func TestTenantStateCachesOnlyActive(t *testing.T) {
	f := &fakeDelcore{state: delcorev1.TenantState_TENANT_STATE_BLOCKED}
	c, clk := newTestClient(t, f)
	ctx := context.Background()
	for range 2 {
		s, err := c.TenantState(ctx, "t")
		if err != nil || s != delcorev1.TenantState_TENANT_STATE_BLOCKED {
			t.Fatalf("state %v %v", s, err)
		}
	}
	if n := f.count("GetTenantStates"); n != 2 {
		t.Fatalf("non-active states must not be cached: %d", n)
	}
	f.set(func(f *fakeDelcore) { f.state = delcorev1.TenantState_TENANT_STATE_ACTIVE })
	for range 3 {
		if s, err := c.TenantState(ctx, "t"); err != nil || s != delcorev1.TenantState_TENANT_STATE_ACTIVE {
			t.Fatalf("state %v %v", s, err)
		}
	}
	if n := f.count("GetTenantStates"); n != 3 {
		t.Fatalf("active state must be cached: %d", n)
	}
	f.set(func(f *fakeDelcore) { f.down = true })
	clk.advance(30 * time.Second)
	if _, err := c.TenantState(ctx, "t"); err == nil {
		t.Fatal("expired state with Delcore down must fail closed")
	}
}

func TestAuthorizeAPIKey(t *testing.T) {
	f := &fakeDelcore{allowed: true}
	c, _ := newTestClient(t, f)
	ctx := context.Background()
	if _, err := c.AuthorizeAPIKey(ctx, "dlk_bad", CapabilityAccessResources, false); !IsUnauthenticated(err) {
		t.Fatalf("invalid key: %v", err)
	}
	d, err := c.AuthorizeAPIKey(ctx, "dlk_good", CapabilityAccessResources, false)
	if err != nil || !d.Allowed || d.UserID != "u" || d.OwnerKind != delcorev1.ApiKeyOwnerKind_API_KEY_OWNER_KIND_PERSONAL {
		t.Fatalf("decision %+v %v", d, err)
	}
	if _, err := c.AuthorizeAPIKey(ctx, "dlk_good", CapabilityAccessResources, false); err != nil {
		t.Fatal(err)
	}
	if n := f.count("AuthorizeApiKey"); n != 2 {
		t.Fatalf("allowed key result must be cached: %d", n)
	}
}

func TestBillingPeriod(t *testing.T) {
	f := &fakeDelcore{}
	c, _ := newTestClient(t, f)
	p, err := c.BillingPeriod(context.Background(), "t")
	if err != nil {
		t.Fatal(err)
	}
	if p.Start.Month() != time.November || p.End.Month() != time.December {
		t.Fatalf("period %+v", p)
	}
	if _, err := c.BillingPeriod(context.Background(), "t"); err != nil {
		t.Fatal(err)
	}
	if n := f.count("ListMeterAggregates"); n != 1 {
		t.Fatalf("period must be cached: %d", n)
	}
}
