// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"connectrpc.com/connect"
	"context"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"strings"
	"testing"
)

func TestServiceTenantLifecycle(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	_, e := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "register lifecycle consumer", Service: &pb.Service{Name: "Git Tracker fixture", ClientId: "service", Capabilities: []string{"view"}, Scopes: []string{"authorize"}}}, "owner"))
	if e != nil {
		t.Fatal(e)
	}
	_, e = client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "bootstrap lifecycle fixture", Plan: &pb.Plan{Name: "free", Free: true, DefaultFree: true}}, "owner"))
	if e != nil {
		t.Fatal(e)
	}
	tenant, e := client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "Lifecycle fixture"}, "owner"))
	if e != nil {
		t.Fatal(e)
	}
	q := &pb.CheckTenantLifecycleRequest{TenantIds: []string{tenant.Msg.Tenant.Id, key()}}
	if _, e = client.CheckTenantLifecycle(ctx, req(q, "owner")); connect.CodeOf(e) != connect.CodePermissionDenied {
		t.Fatal("user obtained service lifecycle access", e)
	}
	_, e = db.Pool.Exec(ctx, "UPDATE tenants SET deleting_at=now() WHERE id=$1", tenant.Msg.Tenant.Id)
	if e != nil {
		t.Fatal(e)
	}
	before, e := client.CheckTenantLifecycle(ctx, req(q, "service"))
	if e != nil || len(before.Msg.FinalizedTenantIds) != 0 {
		t.Fatal("pending or unknown tenant finalized", e)
	}
	_, e = db.Pool.Exec(ctx, "UPDATE tenants SET deleted_at=now() WHERE id=$1", tenant.Msg.Tenant.Id)
	if e != nil {
		t.Fatal(e)
	}
	after, e := client.CheckTenantLifecycle(ctx, req(q, "service"))
	if e != nil || len(after.Msg.FinalizedTenantIds) != 1 || after.Msg.FinalizedTenantIds[0] != tenant.Msg.Tenant.Id {
		t.Fatal("finalized tenant missing", e)
	}
	if _, e = client.CheckTenantLifecycle(ctx, req(&pb.CheckTenantLifecycleRequest{TenantIds: []string{"invalid"}}, "service")); connect.CodeOf(e) != connect.CodeInvalidArgument {
		t.Fatal("invalid identifier accepted", e)
	}
}

func TestServiceTenantStates(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	// "service" holds the authorize scope; "limited" is registered without it, so
	// a token claiming the scope cannot substitute for registration.
	for _, v := range []struct {
		clientID string
		scopes   []string
	}{{"service", []string{"authorize"}}, {"limited", []string{"usage:write"}}} {
		if _, e := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "register tenant state fixture", Service: &pb.Service{Name: "Fixture " + v.clientID, ClientId: v.clientID, Capabilities: []string{"view"}, Scopes: v.scopes}}, "owner")); e != nil {
			t.Fatal(e)
		}
	}
	if _, e := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "bootstrap tenant state fixture", Plan: &pb.Plan{Name: "free", Free: true, DefaultFree: true}}, "owner")); e != nil {
		t.Fatal(e)
	}
	newTenant := func(name string) string {
		r, e := client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: name}, "owner"))
		if e != nil {
			t.Fatal(e)
		}
		return r.Msg.Tenant.Id
	}
	set := func(id, assignments string) {
		if _, e := db.Pool.Exec(ctx, "UPDATE tenants SET "+assignments+" WHERE id=$1", id); e != nil {
			t.Fatal(e)
		}
	}
	states := func(ids ...string) []pb.TenantState {
		r, e := client.GetTenantStates(ctx, req(&pb.GetTenantStatesRequest{TenantIds: ids}, "service"))
		if e != nil {
			t.Fatal(e)
		}
		if r.Msg.Metadata.GetCorrelationId() == "" || r.Header().Get("X-Delcore-Correlation-Id") == "" {
			t.Fatal("correlation metadata missing")
		}
		if len(r.Msg.States) != len(ids) {
			t.Fatal("state count differs from request", len(r.Msg.States), len(ids))
		}
		out := make([]pb.TenantState, len(ids))
		for i, entry := range r.Msg.States {
			if entry.TenantId != ids[i] {
				t.Fatal("state order differs from request")
			}
			out[i] = entry.State
		}
		return out
	}

	active, blocked, deleting, deleted := newTenant("Active"), newTenant("Blocked"), newTenant("Deleting"), newTenant("Deleted")
	blockedDeleting, deletedPending, everything := newTenant("Blocked deleting"), newTenant("Deleted pending"), newTenant("Everything")
	set(blocked, "operator_block=true")
	set(deleting, "deleting_at=now()")
	set(deleted, "deleting_at=now(), deleted_at=now()")
	set(blockedDeleting, "operator_block=true, deleting_at=now()")
	set(deletedPending, "deleted_at=now()")
	set(everything, "operator_block=true, deleting_at=now(), deleted_at=now()")
	unknown := key()
	ids := []string{active, blocked, deleting, deleted, unknown, blockedDeleting, deletedPending, everything}
	want := []pb.TenantState{
		pb.TenantState_TENANT_STATE_ACTIVE, pb.TenantState_TENANT_STATE_BLOCKED, pb.TenantState_TENANT_STATE_DELETING, pb.TenantState_TENANT_STATE_DELETED,
		pb.TenantState_TENANT_STATE_UNKNOWN, pb.TenantState_TENANT_STATE_DELETING, pb.TenantState_TENANT_STATE_DELETED, pb.TenantState_TENANT_STATE_DELETED,
	}
	got := states(ids...)
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("tenant %d state = %s, want %s", i, got[i], want[i])
		}
	}

	// Billing conditions alone never block a tenant and do not alter the result.
	billing := newTenant("Billing conditions")
	set(billing, "renewal_failed=true, period_start=now() - interval '400 days', period_end=now() - interval '300 days'")
	if _, e := db.Pool.Exec(ctx, "INSERT INTO disputes(external_id,tenant_id,active) VALUES('adj_state_fixture',$1,true)", billing); e != nil {
		t.Fatal(e)
	}
	if _, e := db.Pool.Exec(ctx, "INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2,'usage',-100,'state-fixture-debt',now())", key(), billing); e != nil {
		t.Fatal(e)
	}
	if got := states(billing); got[0] != pb.TenantState_TENANT_STATE_ACTIVE {
		t.Fatal("billing conditions changed tenant state", got[0])
	}
	set(billing, "operator_block=true")
	if got := states(billing); got[0] != pb.TenantState_TENANT_STATE_BLOCKED {
		t.Fatal("operator block not reported", got[0])
	}

	// Duplicate requested IDs retain request positions.
	if got := states(active, active, blocked); got[0] != pb.TenantState_TENANT_STATE_ACTIVE || got[1] != pb.TenantState_TENANT_STATE_ACTIVE || got[2] != pb.TenantState_TENANT_STATE_BLOCKED {
		t.Fatal("duplicate IDs mishandled", got)
	}

	// The read does not change tenant state.
	var after bool
	if e := db.Pool.QueryRow(ctx, "SELECT operator_block FROM tenants WHERE id=$1", active).Scan(&after); e != nil || after {
		t.Fatal("state read changed tenant", e)
	}

	// Batch boundaries and malformed input.
	many := func(n int) []string {
		out := make([]string, n)
		for i := range out {
			out[i] = key()
		}
		return out
	}
	for _, n := range []int{1, 100} {
		for _, s := range states(many(n)...) {
			if s != pb.TenantState_TENANT_STATE_UNKNOWN {
				t.Fatal("unknown tenant reported as known", s)
			}
		}
	}
	for name, ids := range map[string][]string{"empty": nil, "over limit": many(101), "malformed": {"invalid"}, "uppercase": {strings.ToUpper(active)}, "mixed malformed": {active, "invalid"}, "nil UUID": {"00000000-0000-0000-0000-000000000000"}} {
		if _, e := client.GetTenantStates(ctx, req(&pb.GetTenantStatesRequest{TenantIds: ids}, "service")); connect.CodeOf(e) != connect.CodeInvalidArgument {
			t.Fatal(name, "input accepted", e)
		}
	}

	// Only registered services whose registration and token both carry authorize.
	for token, code := range map[string]connect.Code{"owner": connect.CodePermissionDenied, "member": connect.CodePermissionDenied, "noscope": connect.CodePermissionDenied, "ghost": connect.CodePermissionDenied, "limited": connect.CodePermissionDenied, "expired": connect.CodeUnauthenticated} {
		resp, e := client.GetTenantStates(ctx, req(&pb.GetTenantStatesRequest{TenantIds: []string{active}}, token))
		if connect.CodeOf(e) != code || resp != nil {
			t.Fatal(token, "token result", e)
		}
	}
	noAuth := connect.NewRequest(&pb.GetTenantStatesRequest{TenantIds: []string{active}})
	if _, e := client.GetTenantStates(ctx, noAuth); connect.CodeOf(e) != connect.CodeUnauthenticated {
		t.Fatal("anonymous caller read states", e)
	}

	// A dependency failure returns an error, never an active or deleted answer.
	if _, e := db.Pool.Exec(ctx, "ALTER TABLE tenants RENAME TO tenants_unavailable"); e != nil {
		t.Fatal(e)
	}
	resp, e := client.GetTenantStates(ctx, req(&pb.GetTenantStatesRequest{TenantIds: []string{active}}, "service"))
	if _, restore := db.Pool.Exec(ctx, "ALTER TABLE tenants_unavailable RENAME TO tenants"); restore != nil {
		t.Fatal(restore)
	}
	if e == nil || resp != nil {
		t.Fatal("dependency failure produced a state", e)
	}
	var detail *pb.ErrorDetail
	for _, d := range e.(*connect.Error).Details() {
		if v, err := d.Value(); err == nil {
			detail, _ = v.(*pb.ErrorDetail)
		}
	}
	if detail == nil || detail.Reason != pb.ErrorReason_ERROR_REASON_DEPENDENCY || detail.CorrelationId == "" {
		t.Fatal("dependency failure lacks safe typed detail", detail)
	}
}
