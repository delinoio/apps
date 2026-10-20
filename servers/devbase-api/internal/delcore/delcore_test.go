// SPDX-License-Identifier: FSL-1.1-ALv2

package delcore

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
)

// stub is a mocked Delcore server that records the credentials it receives.
type stub struct {
	delcorev1connect.UnimplementedDelcoreServiceHandler
	members func(authorization string) error
	user    func(authorization string) (string, error)
	states  func(authorization string, ids []string) ([]*pb.TenantStateEntry, error)
}

func (s *stub) GetCurrentUser(ctx context.Context, r *connect.Request[pb.GetCurrentUserRequest]) (*connect.Response[pb.GetCurrentUserResponse], error) {
	id, e := s.user(r.Header().Get("Authorization"))
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(&pb.GetCurrentUserResponse{UserId: id}), nil
}

func (s *stub) ListMembers(ctx context.Context, r *connect.Request[pb.ListMembersRequest]) (*connect.Response[pb.ListMembersResponse], error) {
	if r.Msg.Page.GetSize() != 1 {
		return nil, connect.NewError(connect.CodeInvalidArgument, errors.New("page size 1 expected"))
	}
	if e := s.members(r.Header().Get("Authorization")); e != nil {
		return nil, e
	}
	return connect.NewResponse(&pb.ListMembersResponse{}), nil
}

func (s *stub) GetTenantStates(ctx context.Context, r *connect.Request[pb.GetTenantStatesRequest]) (*connect.Response[pb.GetTenantStatesResponse], error) {
	entries, e := s.states(r.Header().Get("Authorization"), r.Msg.TenantIds)
	if e != nil {
		return nil, e
	}
	return connect.NewResponse(&pb.GetTenantStatesResponse{States: entries}), nil
}

func client(t *testing.T, s *stub, timeout time.Duration) *Client {
	t.Helper()
	mux := http.NewServeMux()
	mux.Handle(delcorev1connect.NewDelcoreServiceHandler(s))
	ts := httptest.NewServer(mux)
	t.Cleanup(ts.Close)
	return &Client{
		RPC:     delcorev1connect.NewDelcoreServiceClient(ts.Client(), ts.URL),
		Token:   func(context.Context) (string, error) { return "service-token", nil },
		Timeout: timeout,
	}
}

func TestLoginChecksForwardTheCallerToken(t *testing.T) {
	var seen []string
	s := &stub{
		user:    func(a string) (string, error) { seen = append(seen, a); return "user-1", nil },
		members: func(a string) error { seen = append(seen, a); return nil },
	}
	c := client(t, s, time.Second)
	id, e := c.Identify(context.Background(), "Bearer login")
	if e != nil || id != "user-1" {
		t.Fatal(e)
	}
	if e = c.RequireManager(context.Background(), "Bearer login", "tenant"); e != nil {
		t.Fatal(e)
	}
	if len(seen) != 2 || seen[0] != "Bearer login" || seen[1] != "Bearer login" {
		t.Fatal("login token was not forwarded unchanged", seen)
	}
}

func TestFailureClassification(t *testing.T) {
	s := &stub{
		user: func(string) (string, error) {
			return "", connect.NewError(connect.CodeUnauthenticated, errors.New("x"))
		},
		members: func(string) error { return connect.NewError(connect.CodePermissionDenied, errors.New("x")) },
	}
	c := client(t, s, time.Second)
	if _, e := c.Identify(context.Background(), "Bearer x"); !errors.Is(e, ErrUnauthenticated) {
		t.Fatal(e)
	}
	if e := c.RequireManager(context.Background(), "Bearer x", "t"); !errors.Is(e, ErrForbidden) {
		t.Fatal(e)
	}
	s.members = func(string) error { return connect.NewError(connect.CodeFailedPrecondition, errors.New("x")) }
	if e := c.RequireManager(context.Background(), "Bearer x", "t"); !errors.Is(e, ErrUnavailable) {
		t.Fatal("unexpected Delcore error did not deny as unavailable:", e)
	}
	s.user = func(string) (string, error) { return "", nil }
	if _, e := c.Identify(context.Background(), "Bearer x"); !errors.Is(e, ErrUnavailable) {
		t.Fatal("empty user ID accepted")
	}
}

func TestTenantStatesUseTheServiceToken(t *testing.T) {
	var authorization string
	s := &stub{states: func(a string, ids []string) ([]*pb.TenantStateEntry, error) {
		authorization = a
		return []*pb.TenantStateEntry{
			{TenantId: ids[0], State: pb.TenantState_TENANT_STATE_BLOCKED},
			{TenantId: ids[1], State: pb.TenantState_TENANT_STATE_DELETED},
			{TenantId: ids[2], State: pb.TenantState_TENANT_STATE_UNKNOWN},
		}, nil
	}}
	c := client(t, s, time.Second)
	got, e := c.TenantStates(context.Background(), []string{"a", "b", "c"})
	if e != nil || authorization != "Bearer service-token" || got["a"] != StateBlocked || got["b"] != StateDeleted || got["c"] != StateUnknown {
		t.Fatal(got, authorization, e)
	}
	// A malformed answer must fail closed, never grant access.
	entry := func(id string, s pb.TenantState) *pb.TenantStateEntry {
		return &pb.TenantStateEntry{TenantId: id, State: s}
	}
	active, deleted := pb.TenantState_TENANT_STATE_ACTIVE, pb.TenantState_TENANT_STATE_DELETED
	for name, answer := range map[string][]*pb.TenantStateEntry{
		"unspecified state": {entry("a", pb.TenantState_TENANT_STATE_UNSPECIFIED), entry("b", active)},
		"duplicate entry":   {entry("a", deleted), entry("a", active)},
		"missing entry":     {entry("a", active)},
		"extra entry":       {entry("a", active), entry("b", active), entry("c", active)},
		"reordered":         {entry("b", active), entry("a", deleted)},
		"unrequested id":    {entry("a", active), entry("z", active)},
		"empty":             nil,
	} {
		s.states = func(string, []string) ([]*pb.TenantStateEntry, error) { return answer, nil }
		if got, e = c.TenantStates(context.Background(), []string{"a", "b"}); !errors.Is(e, ErrUnavailable) || got != nil {
			t.Errorf("%s: %v %v", name, got, e)
		}
	}
	s.states = func(string, []string) ([]*pb.TenantStateEntry, error) {
		return nil, connect.NewError(connect.CodeInternal, errors.New("x"))
	}
	if _, e = c.TenantStates(context.Background(), []string{"a"}); !errors.Is(e, ErrUnavailable) {
		t.Fatal(e)
	}
	c.Token = func(context.Context) (string, error) { return "", errors.New("down") }
	if _, e = c.TenantStates(context.Background(), []string{"a"}); !errors.Is(e, ErrUnavailable) {
		t.Fatal("token failure did not deny")
	}
}

func TestDependencyTimeout(t *testing.T) {
	s := &stub{members: func(string) error { time.Sleep(300 * time.Millisecond); return nil }}
	c := client(t, s, 50*time.Millisecond)
	start := time.Now()
	if e := c.RequireManager(context.Background(), "Bearer x", "t"); !errors.Is(e, ErrUnavailable) || time.Since(start) > 250*time.Millisecond {
		t.Fatal("slow dependency was not cut off", e)
	}
}

func TestClientCredentialsCacheAndFailures(t *testing.T) {
	var calls atomic.Int32
	status := http.StatusOK
	body := `{"access_token":"tok","expires_in":3600}`
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		id, secret, ok := r.BasicAuth()
		if !ok || id != "client" || secret != "secret" || r.FormValue("grant_type") != "client_credentials" || r.FormValue("resource") != "https://delcore" || r.FormValue("scope") != "authorize" {
			t.Error("unexpected token request")
		}
		w.WriteHeader(status)
		w.Write([]byte(body))
	}))
	defer ts.Close()
	now := time.Unix(1000, 0)
	source := &ClientCredentials{HTTP: ts.Client(), URL: ts.URL, Resource: "https://delcore", ID: "client", Secret: "secret", Scope: "authorize", Now: func() time.Time { return now }}
	for i := 0; i < 3; i++ {
		if tok, e := source.Token(context.Background()); e != nil || tok != "tok" {
			t.Fatal(e)
		}
	}
	if calls.Load() != 1 {
		t.Fatal("token was not cached", calls.Load())
	}
	now = now.Add(2 * time.Hour)
	if _, e := source.Token(context.Background()); e != nil || calls.Load() != 2 {
		t.Fatal("expired token reused")
	}
	// Short-lived tokens are never cached.
	body = `{"access_token":"short","expires_in":30}`
	now = now.Add(2 * time.Hour)
	source.Token(context.Background())
	source.Token(context.Background())
	if calls.Load() != 4 {
		t.Fatal("short-lived token was cached", calls.Load())
	}
	status = http.StatusUnauthorized
	now = now.Add(2 * time.Hour)
	if _, e := source.Token(context.Background()); e == nil {
		t.Fatal("rejected token request succeeded")
	}
	status, body = http.StatusOK, `{"nope":true}`
	if _, e := source.Token(context.Background()); e == nil {
		t.Fatal("malformed token response accepted")
	}
}
