// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"connectrpc.com/connect"
	"context"
	pb "github.com/delinoio/apps/protos/delcore/v1"
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
