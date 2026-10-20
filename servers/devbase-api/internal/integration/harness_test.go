// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"bytes"
	"context"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/devbase/v1"
	"github.com/delinoio/apps/protos/devbase/v1/devbasev1connect"
	"github.com/delinoio/apps/servers/devbase-api/internal/api"
	"github.com/delinoio/apps/servers/devbase-api/internal/config"
	"github.com/delinoio/apps/servers/devbase-api/internal/delcore"
	"github.com/delinoio/apps/servers/devbase-api/internal/maintenance"
	"github.com/delinoio/apps/servers/devbase-api/internal/metrics"
	"github.com/delinoio/apps/servers/devbase-api/internal/store"
	"github.com/google/uuid"
	"github.com/prometheus/client_golang/prometheus"
)

// fakeDelcore is the mocked Delcore. Login tokens are the strings "token:<user>".
// Roles are per user and tenant; states are per tenant. Failures can be injected.
type fakeDelcore struct {
	mu       sync.Mutex
	roles    map[string]string // user|tenant -> role name
	states   map[string]delcore.State
	queued   map[string][]delcore.State // answered once each, in order, before states
	fail     error                      // returned by every call when set
	stateErr error
	delay    time.Duration
	calls    int
	// beforeStates runs once at the start of the next tenant-state read. Authorization reads
	// the state after it resolved the project and before the operation runs, so a hook can
	// delete the project in exactly that window.
	beforeStates func()
}

// wait simulates a slow dependency and honors cancellation like a real client.
func (f *fakeDelcore) wait(ctx context.Context) error {
	f.mu.Lock()
	d := f.delay
	f.mu.Unlock()
	if d == 0 {
		return nil
	}
	select {
	case <-time.After(d):
		return nil
	case <-ctx.Done():
		return delcore.ErrUnavailable
	}
}

func (f *fakeDelcore) setDelay(d time.Duration) { f.mu.Lock(); f.delay = d; f.mu.Unlock() }

func newFake() *fakeDelcore {
	return &fakeDelcore{roles: map[string]string{}, states: map[string]delcore.State{}, queued: map[string][]delcore.State{}}
}

func (f *fakeDelcore) grant(user, tenant, role string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.roles[user+"|"+tenant] = role
}

func (f *fakeDelcore) setState(tenant string, s delcore.State) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.states[tenant] = s
}

// queueState makes the next state read of the tenant answer s once, before the stored state.
func (f *fakeDelcore) queueState(tenant string, s delcore.State) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.queued[tenant] = append(f.queued[tenant], s)
}

func (f *fakeDelcore) setFail(e error) { f.mu.Lock(); f.fail = e; f.mu.Unlock() }

func (f *fakeDelcore) user(authorization string) (string, error) {
	const prefix = "Bearer token:"
	if len(authorization) <= len(prefix) || authorization[:len(prefix)] != prefix {
		return "", delcore.ErrUnauthenticated
	}
	return authorization[len(prefix):], nil
}

func (f *fakeDelcore) Identify(ctx context.Context, authorization string) (string, error) {
	if e := f.wait(ctx); e != nil {
		return "", e
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls++
	if f.fail != nil {
		return "", f.fail
	}
	return f.user(authorization)
}

func (f *fakeDelcore) RequireManager(ctx context.Context, authorization, tenant string) error {
	if e := f.wait(ctx); e != nil {
		return e
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls++
	if f.fail != nil {
		return f.fail
	}
	user, e := f.user(authorization)
	if e != nil {
		return e
	}
	if role := f.roles[user+"|"+tenant]; role != "owner" && role != "admin" {
		return delcore.ErrForbidden
	}
	return nil
}

func (f *fakeDelcore) TenantStates(ctx context.Context, tenants []string) (map[string]delcore.State, error) {
	if e := f.wait(ctx); e != nil {
		return nil, e
	}
	f.mu.Lock()
	hook := f.beforeStates
	f.beforeStates = nil
	f.mu.Unlock()
	if hook != nil {
		hook()
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls++
	if f.fail != nil || f.stateErr != nil {
		if f.stateErr != nil {
			return nil, f.stateErr
		}
		return nil, f.fail
	}
	out := map[string]delcore.State{}
	for _, t := range tenants {
		if q := f.queued[t]; len(q) > 0 {
			out[t], f.queued[t] = q[0], q[1:]
		} else if s, ok := f.states[t]; ok {
			out[t] = s
		}
	}
	return out, nil
}

type fixture struct {
	t       *testing.T
	db      *store.Store
	fake    *fakeDelcore
	server  *api.Server
	client  devbasev1connect.DevBaseServiceClient
	limits  config.Limits
	clock   *clock
	metrics *metrics.Metrics
	gather  prometheus.Gatherer
	logs    *syncBuffer
}

// syncBuffer collects server logs so tests can assert that nothing sensitive is logged.
type syncBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuffer) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.Write(p)
}
func (s *syncBuffer) String() string { s.mu.Lock(); defer s.mu.Unlock(); return s.b.String() }

type clock struct {
	mu sync.Mutex
	t  time.Time
}

func (c *clock) now() time.Time { c.mu.Lock(); defer c.mu.Unlock(); return c.t }
func (c *clock) advance(d time.Duration) {
	c.mu.Lock()
	c.t = c.t.Add(d)
	c.mu.Unlock()
}

func newID() string { return uuid.Must(uuid.NewV7()).String() }

// setup returns a fixture with a clean migrated schema.
func setup(t *testing.T, mutate ...func(*config.Limits)) *fixture {
	t.Helper()
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		t.Skip("TEST_DATABASE_URL is required for PostgreSQL integration tests")
	}
	ctx := context.Background()
	db, e := store.Open(ctx, url)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(db.Close)
	if _, e = db.Pool.Exec(ctx, "DROP SCHEMA public CASCADE; CREATE SCHEMA public"); e != nil {
		t.Fatal(e)
	}
	if e = db.Migrate(ctx); e != nil {
		t.Fatal(e)
	}
	limits := config.DefaultLimits()
	for _, m := range mutate {
		m(&limits)
	}
	f := &fixture{t: t, db: db, fake: newFake(), limits: limits, clock: &clock{t: time.Now().UTC()}}
	registry := prometheus.NewRegistry()
	f.metrics, f.gather = metrics.New(registry), registry
	f.logs = &syncBuffer{}
	f.server = api.New(db.Pool, f.fake, limits, slog.New(slog.NewTextHandler(io.MultiWriter(logSink(), f.logs), nil)), f.metrics, f.clock.now)
	path, handler := f.server.Handler()
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	ts := httptest.NewServer(mux)
	t.Cleanup(ts.Close)
	f.client = devbasev1connect.NewDevBaseServiceClient(ts.Client(), ts.URL)
	return f
}

type discard struct{}

func (discard) Write(p []byte) (int, error) { return len(p), nil }

// as sends a request authenticated with the given bearer token.
func as[Q any](token string, msg *Q) *connect.Request[Q] {
	r := connect.NewRequest(msg)
	r.Header().Set("Authorization", "Bearer "+token)
	return r
}

func login(user string) string { return "token:" + user }

func code(e error) connect.Code {
	if e == nil {
		return 0
	}
	var ce *connect.Error
	if errors.As(e, &ce) {
		return ce.Code()
	}
	return connect.CodeUnknown
}

func idem() string { return "k-" + uuid.NewString() }

// tenant creates a tenant with an owner and an admin.
func (f *fixture) tenant() (tenant, owner, admin string) {
	tenant, owner, admin = newID(), "owner-"+newID(), "admin-"+newID()
	f.fake.grant(owner, tenant, "owner")
	f.fake.grant(admin, tenant, "admin")
	f.fake.setState(tenant, delcore.StateActive)
	return
}

func (f *fixture) project(token, tenant, name string) *pb.Project {
	f.t.Helper()
	r, e := f.client.CreateProject(context.Background(), as(token, &pb.CreateProjectRequest{IdempotencyKey: idem(), TenantId: tenant, Name: name}))
	if e != nil {
		f.t.Fatal("create project:", e)
	}
	return r.Msg.Project
}

func (f *fixture) key(token, project string, permission pb.ApiKeyPermission) (string, *pb.ApiKey) {
	f.t.Helper()
	r, e := f.client.IssueApiKey(context.Background(), as(token, &pb.IssueApiKeyRequest{IdempotencyKey: idem(), ProjectId: project, Permission: permission}))
	if e != nil {
		f.t.Fatal("issue key:", e)
	}
	return r.Msg.Secret, r.Msg.ApiKey
}

func (f *fixture) count(query string, args ...any) int {
	f.t.Helper()
	var n int
	if e := f.db.Pool.QueryRow(context.Background(), query, args...).Scan(&n); e != nil {
		f.t.Fatal(e)
	}
	return n
}

// logSink hides server logs unless DEVBASE_TEST_LOG is set.
func logSink() io.Writer {
	if os.Getenv("DEVBASE_TEST_LOG") != "" {
		return os.Stderr
	}
	return discard{}
}

func (f *fixture) worker() *maintenance.Worker {
	return &maintenance.Worker{Pool: f.db.Pool, Authority: f.fake, Log: slog.New(slog.NewTextHandler(io.MultiWriter(logSink(), f.logs), nil)), Metrics: f.metrics, Now: f.clock.now, Interval: time.Minute}
}

// user creates an end user through the login Owner token.
func (f *fixture) user(token, project, external string) *pb.EndUser {
	f.t.Helper()
	r, e := f.client.CreateEndUser(context.Background(), as(token, &pb.CreateEndUserRequest{IdempotencyKey: idem(), ProjectId: project, ExternalId: external, Name: "Name " + external, Email: external + "@example.com"}))
	if e != nil {
		f.t.Fatal("create end user:", e)
	}
	return r.Msg.EndUser
}
