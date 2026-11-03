// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package service_test

import (
	"context"
	"io"
	"log/slog"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"google.golang.org/protobuf/types/known/timestamppb"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/authz"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/service"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store/storetest"
)

const testTenant = "tenant-it"

// fakeDelcore allows every capability. onAuthorize, when set, runs once on
// the next Authorize call; authz.Resource calls Authorize after it has read
// the caller's role, so the hook can change the ACL in that window.
type fakeDelcore struct {
	mu          sync.Mutex
	onAuthorize func()
	// denied lists users that hold no Delcore capability.
	denied map[string]bool
	// listGroupsCalls counts ListGroups calls; listGroupsDelay stretches each
	// call so that concurrent callers overlap.
	listGroupsCalls atomic.Int64
	listGroupsDelay time.Duration
	// keyDecision, when set, answers AuthorizeAPIKey.
	keyDecision *delcore.KeyDecision
}

func (f *fakeDelcore) Authorize(_ context.Context, _ string, userID string, _ delcore.Capability, _ bool) (delcore.Decision, error) {
	f.mu.Lock()
	hook := f.onAuthorize
	f.onAuthorize = nil
	denied := f.denied[userID]
	f.mu.Unlock()
	if hook != nil {
		hook()
	}
	return delcore.Decision{Allowed: !denied}, nil
}

func (f *fakeDelcore) AuthorizeAPIKey(context.Context, string, delcore.Capability, bool) (delcore.KeyDecision, error) {
	if f.keyDecision != nil {
		return *f.keyDecision, nil
	}
	return delcore.KeyDecision{}, nil
}

func (f *fakeDelcore) ListGroups(context.Context, string, string) ([]*delcorev1.Group, error) {
	f.listGroupsCalls.Add(1)
	time.Sleep(f.listGroupsDelay)
	return []*delcorev1.Group{{Id: "group-1", Name: "Engineering", UserIds: []string{"member"}}}, nil
}

func (f *fakeDelcore) setHook(h func()) {
	f.mu.Lock()
	f.onAuthorize = h
	f.mu.Unlock()
}

type env struct {
	t   *testing.T
	ctx context.Context
	db  *store.Store
	dc  *fakeDelcore
	svc *service.Service
}

func newEnv(t *testing.T) *env {
	t.Helper()
	db := storetest.Open(t)
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	dc := &fakeDelcore{}
	svc := service.New(service.Config{
		Store: db, Authorizer: authz.New(db, dc, time.Minute, log), PublicURL: "https://mcpa.example.test", Log: log,
	})
	return &env{t: t, ctx: context.Background(), db: db, dc: dc, svc: svc}
}

// as returns a context authenticated as a console user without a Delcore
// user token, so no group snapshot refresh runs.
func (e *env) as(userID string) context.Context {
	return auth.WithPrincipal(e.ctx, auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, userID, "sub-"+userID, nil))
}

func newUser() string { return uuid.NewString() }

func (e *env) createResource(userID, requestKey string) *mcpav1.Resource {
	e.t.Helper()
	resp, err := e.svc.CreateResource(e.as(userID), connect.NewRequest(&mcpav1.CreateResourceRequest{
		RequestKey: requestKey, TenantId: testTenant, Name: "res-" + requestKey,
	}))
	if err != nil {
		e.t.Fatalf("create resource: %v", err)
	}
	return resp.Msg.GetResource()
}

func (e *env) putACL(actor, resourceID, userID string, role mcpav1.ResourceRole) error {
	_, err := e.svc.PutAclEntry(e.as(actor), connect.NewRequest(&mcpav1.PutAclEntryRequest{
		ResourceId: resourceID, PrincipalKind: mcpav1.PrincipalKind_PRINCIPAL_KIND_USER, PrincipalId: userID, Role: role,
	}))
	return err
}

// removeUserACL removes a user's entry the way RemoveAclEntry does: in its
// own transaction under the resource lock.
func (e *env) removeUserACL(resourceID, userID string) {
	e.t.Helper()
	id := uuid.MustParse(resourceID)
	err := e.db.InTx(e.ctx, func(tx pgx.Tx) error {
		if _, err := store.LockResource(e.ctx, tx, id); err != nil {
			return err
		}
		_, err := store.RemoveACL(e.ctx, tx, id, mcpav1.PrincipalKind_PRINCIPAL_KIND_USER, userID)
		return err
	})
	if err != nil {
		e.t.Fatalf("remove ACL entry: %v", err)
	}
}

func (e *env) userRole(resourceID, userID string) mcpav1.ResourceRole {
	e.t.Helper()
	entries, err := store.ListACL(e.ctx, e.db.Pool, uuid.MustParse(resourceID))
	if err != nil {
		e.t.Fatal(err)
	}
	return authz.Effective(entries, userID, nil)
}

func reason(err error) mcpav1.ErrorReason { return apperr.As(err).Reason }

func TestMutationRechecksRoleUnderResourceLock(t *testing.T) {
	e := newEnv(t)
	alice, bob := newUser(), newUser()
	res := e.createResource(alice, store.NewID().String())
	if err := e.putACL(alice, res.GetId(), bob, mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE); err != nil {
		t.Fatal(err)
	}

	// Bob removes Alice after her PutAclEntry passed the first role check but
	// before it locks the resource. The pending request must not restore her
	// MANAGE entry.
	e.dc.setHook(func() { e.removeUserACL(res.GetId(), alice) })
	err := e.putACL(alice, res.GetId(), alice, mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE)
	if reason(err) != mcpav1.ErrorReason_ERROR_REASON_NOT_FOUND {
		t.Fatalf("PutAclEntry after removal returned %v, want NOT_FOUND", err)
	}
	if got := e.userRole(res.GetId(), alice); got != mcpav1.ResourceRole_RESOURCE_ROLE_UNSPECIFIED {
		t.Fatalf("removed manager holds %v after the pending mutation", got)
	}

	// A manager downgraded to INVOKE in the same window cannot finish a
	// MANAGE action.
	carol := newUser()
	if err := e.putACL(bob, res.GetId(), carol, mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE); err != nil {
		t.Fatal(err)
	}
	e.dc.setHook(func() {
		if err := e.putACL(bob, res.GetId(), carol, mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE); err != nil {
			t.Errorf("downgrade: %v", err)
		}
	})
	_, err = e.svc.UpdateResource(e.as(carol), connect.NewRequest(&mcpav1.UpdateResourceRequest{
		ResourceId: res.GetId(), Name: "renamed",
	}))
	if reason(err) != mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED {
		t.Fatalf("UpdateResource after downgrade returned %v, want PERMISSION_DENIED", err)
	}
	got, err := e.svc.GetResource(e.as(bob), connect.NewRequest(&mcpav1.GetResourceRequest{ResourceId: res.GetId()}))
	if err != nil {
		t.Fatal(err)
	}
	if got.Msg.GetResource().GetName() == "renamed" {
		t.Fatal("the downgraded manager renamed the resource")
	}

	// Without a concurrent change the mutation still succeeds.
	if err := e.putACL(bob, res.GetId(), alice, mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE); err != nil {
		t.Fatalf("PutAclEntry by a current manager: %v", err)
	}
}

func TestCreateResourceReplayRequiresCurrentRole(t *testing.T) {
	e := newEnv(t)
	alice, bob := newUser(), newUser()
	key := store.NewID().String()
	res := e.createResource(alice, key)

	// The creator's replay returns the original resource with the creator's role.
	again := e.createResource(alice, key)
	if again.GetId() != res.GetId() || again.GetCallerRole() != mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE {
		t.Fatalf("replay returned %s with %v, want %s with MANAGE", again.GetId(), again.GetCallerRole(), res.GetId())
	}

	if err := e.putACL(alice, res.GetId(), bob, mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE); err != nil {
		t.Fatal(err)
	}
	if _, err := e.svc.UpdateRouting(e.as(bob), connect.NewRequest(&mcpav1.UpdateRoutingRequest{
		ResourceId: res.GetId(), Routing: &mcpav1.RoutingSettings{ModelAllowlist: []string{"secret-model"}},
	})); err != nil {
		t.Fatal(err)
	}
	e.removeUserACL(res.GetId(), alice)

	// After losing every role, the replay must not disclose the resource.
	_, err := e.svc.CreateResource(e.as(alice), connect.NewRequest(&mcpav1.CreateResourceRequest{
		RequestKey: key, TenantId: testTenant, Name: "res-" + key,
	}))
	if reason(err) != mcpav1.ErrorReason_ERROR_REASON_NOT_FOUND {
		t.Fatalf("replay without a role returned %v, want NOT_FOUND", err)
	}
	_, err = e.svc.GetResource(e.as(alice), connect.NewRequest(&mcpav1.GetResourceRequest{ResourceId: res.GetId()}))
	if reason(err) != mcpav1.ErrorReason_ERROR_REASON_NOT_FOUND {
		t.Fatalf("GetResource without a role returned %v, want NOT_FOUND", err)
	}

	// An INVOKE role restores the replay with that role.
	if err := e.putACL(bob, res.GetId(), alice, mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE); err != nil {
		t.Fatal(err)
	}
	if got := e.createResource(alice, key); got.GetCallerRole() != mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE {
		t.Fatalf("replay with INVOKE returned role %v", got.GetCallerRole())
	}
}

func TestListTenantAuditEventsReturnsTenantLevelEvents(t *testing.T) {
	e := newEnv(t)
	owner, outsider := newUser(), newUser()
	// The resource's creation event is resource-level and must be excluded.
	e.createResource(owner, store.NewID().String())
	var tenantEvents []uuid.UUID
	for range 3 {
		ev, err := store.InsertAudit(e.ctx, e.db.Pool, store.AuditEvent{
			TenantID: testTenant, Action: mcpav1.AuditAction_AUDIT_ACTION_TENANT_BUDGET_CHANGED,
			ActorKind: mcpav1.ActorKind_ACTOR_KIND_CONSOLE, ActorUserID: owner, TargetID: testTenant,
		})
		if err != nil {
			t.Fatal(err)
		}
		tenantEvents = append(tenantEvents, ev.ID)
	}
	if _, err := store.InsertAudit(e.ctx, e.db.Pool, store.AuditEvent{
		TenantID: "tenant-other", Action: mcpav1.AuditAction_AUDIT_ACTION_TENANT_BUDGET_CHANGED,
		ActorKind: mcpav1.ActorKind_ACTOR_KIND_CONSOLE, ActorUserID: owner, TargetID: "tenant-other",
	}); err != nil {
		t.Fatal(err)
	}

	// The tenant budget holder reads only this tenant's tenant-level events,
	// newest first and paged.
	list := func(user, after string) (*mcpav1.ListTenantAuditEventsResponse, error) {
		resp, err := e.svc.ListTenantAuditEvents(e.as(user), connect.NewRequest(&mcpav1.ListTenantAuditEventsRequest{
			TenantId: testTenant, Page: &mcpav1.Page{Size: 2, AfterId: after},
		}))
		if err != nil {
			return nil, err
		}
		return resp.Msg, nil
	}
	first, err := list(owner, "")
	if err != nil {
		t.Fatal(err)
	}
	second, err := list(owner, first.GetNextAfterId())
	if err != nil {
		t.Fatal(err)
	}
	var got []string
	for _, ev := range append(first.GetEvents(), second.GetEvents()...) {
		if ev.GetResourceId() != "" || ev.GetTenantId() != testTenant || ev.GetAction() != mcpav1.AuditAction_AUDIT_ACTION_TENANT_BUDGET_CHANGED {
			t.Fatalf("unexpected event %v", ev)
		}
		got = append(got, ev.GetId())
	}
	want := []string{tenantEvents[2].String(), tenantEvents[1].String(), tenantEvents[0].String()}
	if len(got) != len(want) || got[0] != want[0] || got[1] != want[1] || got[2] != want[2] || second.GetNextAfterId() != "" {
		t.Fatalf("events = %v (next %q), want %v", got, second.GetNextAfterId(), want)
	}

	// Without the tenant budget capability the query is denied, even for a
	// resource manager.
	e.dc.mu.Lock()
	e.dc.denied = map[string]bool{owner: true, outsider: true}
	e.dc.mu.Unlock()
	for _, user := range []string{owner, outsider} {
		if _, err := list(user, ""); reason(err) != mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED {
			t.Fatalf("%s without capability: %v", user, err)
		}
	}
}

func TestIssueCallKeyReplayAfterExpiry(t *testing.T) {
	e := newEnv(t)
	alice := newUser()
	res := e.createResource(alice, store.NewID().String())
	issue := func(requestKey string, expire time.Time) (*mcpav1.IssueCallKeyResponse, error) {
		resp, err := e.svc.IssueCallKey(e.as(alice), connect.NewRequest(&mcpav1.IssueCallKeyRequest{
			RequestKey: requestKey, ResourceId: res.GetId(), Name: "ci", ExpireTime: timestamppb.New(expire),
		}))
		if err != nil {
			return nil, err
		}
		return resp.Msg, nil
	}
	claims := func(requestKey string) int {
		t.Helper()
		var n int
		if err := e.db.Pool.QueryRow(e.ctx, `SELECT count(*) FROM idempotency_records WHERE operation = 'IssueCallKey' AND request_key = $1`,
			uuid.MustParse(requestKey)).Scan(&n); err != nil {
			t.Fatal(err)
		}
		return n
	}

	start := time.Now()
	expire := start.Add(time.Hour).Truncate(time.Second)
	key := store.NewID().String()
	first, err := issue(key, expire)
	if err != nil {
		t.Fatalf("issue: %v", err)
	}
	if first.GetSecret() == "" {
		t.Fatal("the first execution returned no secret")
	}

	// The key has expired, but the request key is still inside its
	// idempotency window: the replay returns the original key without a secret.
	e.svc.SetClock(func() time.Time { return start.Add(2 * time.Hour) })
	again, err := issue(key, expire)
	if err != nil {
		t.Fatalf("replay after expiry: %v", err)
	}
	if again.GetCallKey().GetId() != first.GetCallKey().GetId() || again.GetSecret() != "" {
		t.Fatalf("replay returned %s with secret %t, want %s without a secret",
			again.GetCallKey().GetId(), again.GetSecret() != "", first.GetCallKey().GetId())
	}
	if !again.GetCallKey().GetExpireTime().AsTime().Equal(expire) {
		t.Fatalf("replay expire_time = %v, want %v", again.GetCallKey().GetExpireTime().AsTime(), expire)
	}

	// A new request key with a past expiry is rejected and leaves no claim,
	// so the key can still issue a valid request.
	fresh := store.NewID().String()
	if _, err := issue(fresh, expire); reason(err) != mcpav1.ErrorReason_ERROR_REASON_INVALID_INPUT {
		t.Fatalf("new issuance with a past expiry returned %v, want INVALID_INPUT", err)
	}
	if n := claims(fresh); n != 0 {
		t.Fatalf("the rejected issuance left %d idempotency records", n)
	}
	if _, err := issue(fresh, start.Add(3*time.Hour)); err != nil {
		t.Fatalf("issue after a rejected request with the same key: %v", err)
	}
}

// TestListTenantGroupsRefreshesOncePerTTL fires concurrent ListTenantGroups
// calls through two services that share a database, like two replicas, and
// checks that Delcore ListGroups runs once for the tenant within the TTL.
func TestListTenantGroupsRefreshesOncePerTTL(t *testing.T) {
	db := storetest.Open(t)
	log := slog.New(slog.NewTextHandler(io.Discard, nil))
	dc := &fakeDelcore{listGroupsDelay: 150 * time.Millisecond}
	newService := func() *service.Service {
		return service.New(service.Config{
			Store: db, PublicURL: "https://mcpa.example.test", Log: log,
			Authorizer: authz.New(db, dc, time.Minute, log).WithRefreshTimeout(5 * time.Second),
		})
	}
	replicas := []*service.Service{newService(), newService()}
	user := auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, newUser(), "sub", func(context.Context) (string, error) {
		return "user-token", nil
	})
	ctx := auth.WithPrincipal(context.Background(), user)
	list := func(svc *service.Service) (int, error) {
		resp, err := svc.ListTenantGroups(ctx, connect.NewRequest(&mcpav1.ListTenantGroupsRequest{TenantId: testTenant}))
		if err != nil {
			return 0, err
		}
		if got := resp.Msg.GetGroups(); len(got) != 1 || got[0].GetId() != "group-1" || got[0].GetName() != "Engineering" {
			t.Errorf("groups %v", got)
		}
		return len(resp.Msg.GetGroups()), nil
	}

	const callers = 20
	var wg sync.WaitGroup
	for i := 0; i < callers; i++ {
		wg.Add(1)
		go func(svc *service.Service) {
			defer wg.Done()
			if _, err := list(svc); err != nil {
				t.Errorf("ListTenantGroups: %v", err)
			}
		}(replicas[i%len(replicas)])
	}
	wg.Wait()
	if got := dc.listGroupsCalls.Load(); got != 1 {
		t.Fatalf("%d concurrent calls made %d ListGroups calls, want 1", callers, got)
	}

	// A call within the TTL reuses the snapshot.
	if _, err := list(replicas[0]); err != nil {
		t.Fatal(err)
	}
	if got := dc.listGroupsCalls.Load(); got != 1 {
		t.Fatalf("a call within the TTL made %d ListGroups calls, want 1", got)
	}

	// A personal key reads the snapshot and never calls Delcore for groups.
	keyUser := newUser()
	dc.keyDecision = &delcore.KeyDecision{Allowed: true, TenantID: testTenant, UserID: keyUser}
	key := auth.WithPrincipal(context.Background(), auth.NewPersonalKeyPrincipal(keyUser, testTenant, "dlk_x"))
	resp, err := replicas[1].ListTenantGroups(key, connect.NewRequest(&mcpav1.ListTenantGroupsRequest{TenantId: testTenant}))
	if err != nil {
		t.Fatalf("personal key ListTenantGroups: %v", err)
	}
	if got := resp.Msg.GetGroups(); len(got) != 1 || got[0].GetId() != "group-1" {
		t.Fatalf("personal key groups %v", got)
	}
	if got := dc.listGroupsCalls.Load(); got != 1 {
		t.Fatalf("a personal key made %d ListGroups calls, want 1", got)
	}
}
