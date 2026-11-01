// SPDX-License-Identifier: FSL-1.1-ALv2

package authz

import (
	"context"
	"testing"

	delcorev1 "github.com/delinoio/apps/protos/delcore/v1"
	mcpav1 "github.com/delinoio/apps/protos/mcpa/v1"
	"github.com/delinoio/apps/servers/mcpa-api/internal/apperr"
	"github.com/delinoio/apps/servers/mcpa-api/internal/auth"
	"github.com/delinoio/apps/servers/mcpa-api/internal/delcore"
	"github.com/delinoio/apps/servers/mcpa-api/internal/store"
)

const (
	manage = mcpav1.ResourceRole_RESOURCE_ROLE_MANAGE
	invoke = mcpav1.ResourceRole_RESOURCE_ROLE_INVOKE
	none   = mcpav1.ResourceRole_RESOURCE_ROLE_UNSPECIFIED
	user   = mcpav1.PrincipalKind_PRINCIPAL_KIND_USER
	group  = mcpav1.PrincipalKind_PRINCIPAL_KIND_GROUP
)

func TestSatisfies(t *testing.T) {
	cases := []struct {
		have, need mcpav1.ResourceRole
		want       bool
	}{
		{manage, manage, true},
		{manage, invoke, true},
		{invoke, invoke, true},
		{invoke, manage, false},
		{none, invoke, false},
		{manage, none, false},
		{mcpav1.ResourceRole(99), invoke, false},
	}
	for _, c := range cases {
		if got := Satisfies(c.have, c.need); got != c.want {
			t.Errorf("Satisfies(%v, %v) = %v", c.have, c.need, got)
		}
	}
	if Strongest(invoke, manage, invoke) != manage || Strongest() != none || Strongest(invoke) != invoke {
		t.Fatal("Strongest")
	}
}

func TestEffectiveDirectAndGroups(t *testing.T) {
	entries := []store.ACLEntry{
		{PrincipalKind: user, PrincipalID: "alice", Role: invoke},
		{PrincipalKind: group, PrincipalID: "ops", Role: manage},
		{PrincipalKind: group, PrincipalID: "devs", Role: invoke},
		// A group ID equal to a user ID must not match the user directly.
		{PrincipalKind: group, PrincipalID: "carol", Role: manage},
	}
	cases := []struct {
		name   string
		user   string
		groups []string
		want   mcpav1.ResourceRole
	}{
		{"direct only", "alice", nil, invoke},
		{"group upgrades direct", "alice", []string{"ops"}, manage},
		{"group only", "bob", []string{"devs"}, invoke},
		{"strongest group", "bob", []string{"devs", "ops"}, manage},
		{"no entry", "dave", []string{"other"}, none},
		{"kind separation", "carol", nil, none},
		{"left group", "bob", nil, none},
	}
	for _, c := range cases {
		if got := Effective(entries, c.user, c.groups); got != c.want {
			t.Errorf("%s: got %v, want %v", c.name, got, c.want)
		}
	}
}

func TestHasOtherManager(t *testing.T) {
	entries := []store.ACLEntry{
		{PrincipalKind: user, PrincipalID: "alice", Role: manage},
		{PrincipalKind: user, PrincipalID: "bob", Role: invoke},
	}
	if HasOtherManager(entries, user, "alice") {
		t.Fatal("alice is the last manager")
	}
	if !HasOtherManager(entries, user, "bob") {
		t.Fatal("removing bob keeps alice")
	}
	withGroup := append(entries, store.ACLEntry{PrincipalKind: group, PrincipalID: "ops", Role: manage})
	if !HasOtherManager(withGroup, user, "alice") {
		t.Fatal("a group with MANAGE keeps the resource managed")
	}
}

type fakeDelcore struct {
	allowed bool
	key     delcore.KeyDecision
	calls   []delcore.Capability
}

func (f *fakeDelcore) Authorize(_ context.Context, _, _ string, c delcore.Capability, paid bool) (delcore.Decision, error) {
	if paid {
		panic("management must use paid=false")
	}
	f.calls = append(f.calls, c)
	return delcore.Decision{Allowed: f.allowed}, nil
}

func (f *fakeDelcore) AuthorizeAPIKey(_ context.Context, _ string, c delcore.Capability, paid bool) (delcore.KeyDecision, error) {
	if paid {
		panic("management must use paid=false")
	}
	f.calls = append(f.calls, c)
	return f.key, nil
}

func (f *fakeDelcore) ListGroups(context.Context, string, string) ([]*delcorev1.Group, error) {
	return nil, nil
}

func TestCapabilityRequirement(t *testing.T) {
	ctx := context.Background()
	f := &fakeDelcore{allowed: false}
	a := &Authorizer{dc: f}
	console := auth.NewUserPrincipal(mcpav1.ActorKind_ACTOR_KIND_CONSOLE, "u1", "s", nil)
	if err := a.Capability(ctx, console, "t1", delcore.CapabilityCreateResources); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED {
		t.Fatalf("denied capability: %v", err)
	}
	f.allowed = true
	if err := a.Capability(ctx, console, "t1", delcore.CapabilityCreateResources); err != nil {
		t.Fatal(err)
	}

	key := auth.NewPersonalKeyPrincipal("u1", "t1", "dlk_x")
	f.key = delcore.KeyDecision{Allowed: true, TenantID: "t1", UserID: "u1"}
	if err := a.Capability(ctx, key, "t1", delcore.CapabilityAccessResources); err != nil {
		t.Fatal(err)
	}
	if err := a.Capability(ctx, key, "t2", delcore.CapabilityAccessResources); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED {
		t.Fatalf("personal key in another tenant: %v", err)
	}
	f.key.Allowed = false
	if err := a.Capability(ctx, key, "t1", delcore.CapabilityAccessResources); apperr.As(err).Reason != mcpav1.ErrorReason_ERROR_REASON_PERMISSION_DENIED {
		t.Fatalf("key without grant: %v", err)
	}
}
