// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package service_test

import (
	"context"
	"io"
	"log/slog"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

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
}

func (f *fakeDelcore) Authorize(context.Context, string, string, delcore.Capability, bool) (delcore.Decision, error) {
	f.mu.Lock()
	hook := f.onAuthorize
	f.onAuthorize = nil
	f.mu.Unlock()
	if hook != nil {
		hook()
	}
	return delcore.Decision{Allowed: true}, nil
}

func (f *fakeDelcore) AuthorizeAPIKey(context.Context, string, delcore.Capability, bool) (delcore.KeyDecision, error) {
	return delcore.KeyDecision{}, nil
}

func (f *fakeDelcore) ListGroups(context.Context, string, string) ([]*delcorev1.Group, error) {
	return nil, nil
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
