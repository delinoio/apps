// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package authz

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store/storetest"
)

// countingDelcore counts ListGroups calls. delay stretches each call so that
// concurrent callers overlap; err, when set, fails each call.
type countingDelcore struct {
	fakeDelcore
	calls atomic.Int64
	mu    sync.Mutex
	delay time.Duration
	err   error
}

func (c *countingDelcore) ListGroups(ctx context.Context, _, tenantID string) ([]*delcorev1.Group, error) {
	c.calls.Add(1)
	c.mu.Lock()
	delay, err := c.delay, c.err
	c.mu.Unlock()
	select {
	case <-time.After(delay):
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	if err != nil {
		return nil, err
	}
	return []*delcorev1.Group{{Id: "g1", Name: "Group " + tenantID, UserIds: []string{"u1"}}}, nil
}

func (c *countingDelcore) set(delay time.Duration, err error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.delay, c.err = delay, err
}

type groupEnv struct {
	db    *store.Store
	dc    *countingDelcore
	clock *testClock
	user  *auth.Principal
}

type testClock struct {
	mu sync.Mutex
	t  time.Time
}

func (c *testClock) now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.t
}

func (c *testClock) advance(d time.Duration) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.t = c.t.Add(d)
}

func newGroupEnv(t *testing.T) *groupEnv {
	t.Helper()
	return &groupEnv{
		db:    storetest.Open(t),
		dc:    &countingDelcore{},
		clock: &testClock{t: time.Now().UTC().Truncate(time.Microsecond)},
		user: auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, "u1", "s1", func(context.Context) (string, error) {
			return "user-token", nil
		}),
	}
}

// replica builds an Authorizer like one API process; all replicas of an
// environment share the database, the Delcore fake and the clock.
func (e *groupEnv) replica(ttl, timeout time.Duration) *Authorizer {
	a := New(e.db, e.dc, ttl, slog.New(slog.NewTextHandler(io.Discard, nil))).WithRefreshTimeout(timeout)
	a.now = e.clock.now
	return a
}

func (e *groupEnv) refreshTime(t *testing.T, tenant string) time.Time {
	t.Helper()
	at, err := store.SnapshotRefreshTime(context.Background(), e.db.Pool, tenant)
	if err != nil {
		t.Fatal(err)
	}
	return at
}

func TestSnapshotGroupsReusesFreshSnapshot(t *testing.T) {
	ctx := context.Background()
	e := newGroupEnv(t)
	a := e.replica(time.Minute, 2*time.Second)

	for i := 0; i < 3; i++ {
		groups, err := a.SnapshotGroups(ctx, e.user, "t1")
		if err != nil || len(groups) != 1 || groups[0].ID != "g1" {
			t.Fatalf("call %d: %v, %v", i, groups, err)
		}
	}
	if got := e.dc.calls.Load(); got != 1 {
		t.Fatalf("ListGroups called %d times within the TTL, want 1", got)
	}

	// The same snapshot serves another replica.
	if _, err := e.replica(time.Minute, 2*time.Second).SnapshotGroups(ctx, e.user, "t1"); err != nil {
		t.Fatal(err)
	}
	if got := e.dc.calls.Load(); got != 1 {
		t.Fatalf("a second replica called ListGroups: %d calls", got)
	}

	// Another tenant has its own TTL window.
	if _, err := a.SnapshotGroups(ctx, e.user, "t2"); err != nil {
		t.Fatal(err)
	}
	if got := e.dc.calls.Load(); got != 2 {
		t.Fatalf("second tenant: %d calls, want 2", got)
	}

	// An expired snapshot refreshes once.
	e.clock.advance(time.Minute + time.Second)
	for i := 0; i < 2; i++ {
		if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err != nil {
			t.Fatal(err)
		}
	}
	if got := e.dc.calls.Load(); got != 3 {
		t.Fatalf("after expiry: %d calls, want 3", got)
	}
}

func TestSnapshotGroupsConcurrentCallersAcrossReplicas(t *testing.T) {
	ctx := context.Background()
	e := newGroupEnv(t)
	e.dc.set(150*time.Millisecond, nil)
	replicas := []*Authorizer{e.replica(time.Minute, 5*time.Second), e.replica(time.Minute, 5*time.Second)}

	const callers = 24
	var wg sync.WaitGroup
	errs := make(chan error, callers)
	for i := 0; i < callers; i++ {
		wg.Add(1)
		go func(a *Authorizer) {
			defer wg.Done()
			groups, err := a.SnapshotGroups(ctx, e.user, "t1")
			if err == nil && (len(groups) != 1 || groups[0].ID != "g1") {
				err = errors.New("caller did not see the refreshed snapshot")
			}
			errs <- err
		}(replicas[i%len(replicas)])
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	if got := e.dc.calls.Load(); got != 1 {
		t.Fatalf("%d concurrent callers on two replicas made %d ListGroups calls, want 1", callers, got)
	}
}

func TestSnapshotGroupsFailedRefreshKeepsSnapshot(t *testing.T) {
	ctx := context.Background()
	e := newGroupEnv(t)
	a := e.replica(time.Minute, 2*time.Second)

	// A first refresh that fails leaves no snapshot and reports the error.
	e.dc.set(0, errors.New("delcore unavailable"))
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err == nil {
		t.Fatal("first refresh failure must be reported")
	}
	if at := e.refreshTime(t, "t1"); !at.IsZero() {
		t.Fatalf("failed first refresh stored a snapshot at %v", at)
	}

	e.dc.set(0, nil)
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err != nil {
		t.Fatal(err)
	}
	stored := e.refreshTime(t, "t1")

	// A later dependency failure serves the previous snapshot unchanged.
	e.clock.advance(2 * time.Minute)
	e.dc.set(0, errors.New("delcore unavailable"))
	before := e.dc.calls.Load()
	groups, err := a.SnapshotGroups(ctx, e.user, "t1")
	if err != nil || len(groups) != 1 || groups[0].ID != "g1" {
		t.Fatalf("stale snapshot: %v, %v", groups, err)
	}
	if e.dc.calls.Load() != before+1 {
		t.Fatal("the stale snapshot must trigger one refresh attempt")
	}
	if at := e.refreshTime(t, "t1"); !at.Equal(stored) {
		t.Fatalf("failed refresh moved refresh_time from %v to %v", stored, at)
	}

	// A rejected user credential is reported even with a snapshot.
	e.dc.set(0, connectUnauthenticated())
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); !delcore.IsUnauthenticated(err) {
		t.Fatalf("expired credentials: %v", err)
	}
	if at := e.refreshTime(t, "t1"); !at.Equal(stored) {
		t.Fatalf("rejected refresh moved refresh_time to %v", at)
	}

	// Recovery refreshes and moves refresh_time.
	e.dc.set(0, nil)
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err != nil {
		t.Fatal(err)
	}
	if at := e.refreshTime(t, "t1"); !at.After(stored) {
		t.Fatalf("recovered refresh left refresh_time at %v", at)
	}
}

func TestSnapshotGroupsPersonalKeyReadsSnapshotOnly(t *testing.T) {
	ctx := context.Background()
	e := newGroupEnv(t)
	a := e.replica(time.Minute, 2*time.Second)
	key := auth.NewPersonalKeyPrincipal("u1", "t1", "dlk_x")

	groups, err := a.SnapshotGroups(ctx, key, "t1")
	if err != nil || len(groups) != 0 {
		t.Fatalf("empty snapshot: %v, %v", groups, err)
	}
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err != nil {
		t.Fatal(err)
	}
	e.clock.advance(time.Hour)
	groups, err = a.SnapshotGroups(ctx, key, "t1")
	if err != nil || len(groups) != 1 {
		t.Fatalf("stored snapshot: %v, %v", groups, err)
	}
	if got := e.dc.calls.Load(); got != 1 {
		t.Fatalf("a personal key triggered ListGroups: %d calls", got)
	}
}

// TestRequireGroupsReportsLostMFAEvidence covers the console request paths:
// an MFA-required error from the user token function is returned, a stale
// snapshot is not served for it, the best-effort RefreshGroups keeps
// swallowing it, other refresh failures stay logged, and a personal key
// (no user token) is exempt.
func TestRequireGroupsReportsLostMFAEvidence(t *testing.T) {
	ctx := context.Background()
	e := newGroupEnv(t)
	a := e.replica(time.Minute, 2*time.Second)
	lost := auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, "u1", "s1", func(context.Context) (string, error) {
		return "", apperr.MFARequired("https://login.example.test/mfa")
	})

	err := a.RequireGroups(ctx, lost, "t1")
	if got := apperr.As(err); err == nil || got.Reason != mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED || got.ReauthURL != "https://login.example.test/mfa" {
		t.Fatalf("RequireGroups with lost MFA evidence returned %v, want MFA_REQUIRED with the reauthentication URL", err)
	}
	if got := e.dc.calls.Load(); got != 0 {
		t.Fatalf("lost MFA evidence still called Delcore ListGroups %d times", got)
	}

	// With a stored snapshot, SnapshotGroups must not serve it for a principal
	// that lost MFA evidence, unlike other refresh failures.
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err != nil {
		t.Fatal(err)
	}
	e.clock.advance(time.Hour)
	if _, err := a.SnapshotGroups(ctx, lost, "t1"); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_MFA_REQUIRED {
		t.Fatalf("SnapshotGroups with lost MFA evidence returned %v, want MFA_REQUIRED", err)
	}

	// The best-effort form used for MCP grants keeps the old behavior.
	a.RefreshGroups(ctx, lost, "t1")

	// Other refresh failures are logged and do not fail the request.
	e.dc.set(0, errors.New("delcore down"))
	if err := a.RequireGroups(ctx, e.user, "t1"); err != nil {
		t.Fatalf("RequireGroups with a Delcore outage returned %v, want nil", err)
	}

	// Personal keys carry no user token and stay MFA-exempt.
	key := auth.NewPersonalKeyPrincipal("u1", "t1", "dlk_x")
	if err := a.RequireGroups(ctx, key, "t1"); err != nil {
		t.Fatalf("RequireGroups for a personal key returned %v, want nil", err)
	}
}

func TestSnapshotGroupsRefreshIsBoundedByTimeout(t *testing.T) {
	ctx := context.Background()
	e := newGroupEnv(t)
	a := e.replica(time.Minute, 300*time.Millisecond)

	// Another replica holds the tenant's refresh lock and never finishes.
	holder, err := e.db.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer holder.Rollback(ctx)
	if _, err := holder.Exec(ctx, `SELECT pg_advisory_xact_lock(hashtextextended($1, 0))`, groupRefreshLockKey+"t1"); err != nil {
		t.Fatal(err)
	}

	start := time.Now()
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err == nil {
		t.Fatal("a refresh blocked by another replica must fail at the timeout")
	}
	if elapsed := time.Since(start); elapsed > 3*time.Second {
		t.Fatalf("refresh waited %v for a lock held by another replica", elapsed)
	}
	if got := e.dc.calls.Load(); got != 0 {
		t.Fatalf("ListGroups ran without the refresh lock: %d calls", got)
	}

	// The lock is transaction scoped: once released, refreshes proceed.
	if err := holder.Rollback(ctx); err != nil && !errors.Is(err, pgx.ErrTxClosed) {
		t.Fatal(err)
	}
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err != nil {
		t.Fatal(err)
	}
}

func TestSnapshotGroupsTimeoutAbortsSlowDelcore(t *testing.T) {
	ctx := context.Background()
	e := newGroupEnv(t)
	a := e.replica(time.Minute, 200*time.Millisecond)
	e.dc.set(10*time.Second, nil)

	start := time.Now()
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err == nil {
		t.Fatal("a slow Delcore call must fail at the timeout")
	}
	if elapsed := time.Since(start); elapsed > 3*time.Second {
		t.Fatalf("refresh took %v", elapsed)
	}
	if at := e.refreshTime(t, "t1"); !at.IsZero() {
		t.Fatalf("aborted refresh stored a snapshot at %v", at)
	}
	// The aborted transaction released its lock.
	e.dc.set(0, nil)
	if _, err := a.SnapshotGroups(ctx, e.user, "t1"); err != nil {
		t.Fatal(err)
	}
}

// connectUnauthenticated is the error Delcore returns for a rejected user
// token.
func connectUnauthenticated() error {
	return &delcore.Error{Kind: delcore.KindUnauthenticated, RPC: "ListGroups", Cause: errors.New("token rejected")}
}
