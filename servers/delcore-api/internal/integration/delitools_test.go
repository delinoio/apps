//go:build integration

// SPDX-License-Identifier: FSL-1.1-ALv2
package integration

import (
	"connectrpc.com/connect"
	"context"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/worker"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"
	"testing"
	"time"
)

func TestDeliToolsPricesAndKeyAuthority(t *testing.T) {
	db, c, cfg := setup(t)
	ctx := context.Background()
	service, err := c.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "register DeliTools", Service: &pb.Service{Name: "delitools", ClientId: "service", Capabilities: []string{"translator", "extractor"}, Scopes: []string{"authorize", "usage:write", "usage:read"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	plan, err := c.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "configure free plan", Plan: &pb.Plan{Name: "free", Free: true, DefaultFree: true}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	for _, meter := range []string{"translator", "extractor"} {
		if _, err = c.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "register AI meter", Meter: &pb.Meter{ServiceId: service.Msg.Service.Id, Name: meter}}, "owner")); err != nil {
			t.Fatal(err)
		}
		if _, err = c.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "configure entitlement", Price: &pb.Price{PlanId: plan.Msg.Plan.Id, ServiceId: service.Msg.Service.Id, Meter: meter, UnitPrice: "999", Included: "999", Currency: "USD"}}, "owner")); err != nil {
			t.Fatal(err)
		}
	}
	me, err := c.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	org, err := c.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "AI tenant"}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	tenant := org.Msg.Tenant.Id
	zero, err := c.Authorize(ctx, req(&pb.AuthorizeRequest{TenantId: tenant, Paid: true, Capability: "translator", ConsumerAuthorization: pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_TENANT_KEY}, "service"))
	if err != nil || zero.Msg.Allowed {
		t.Fatalf("AI allowances authorized zero credit: %v %v", zero, err)
	}
	w := &worker.Worker{DB: db, Config: cfg}
	checkNotice := func(want bool) {
		t.Helper()
		if err := w.Maintain(ctx, time.Now()); err != nil {
			t.Fatal(err)
		}
		var active bool
		if err := db.Pool.QueryRow(ctx, "SELECT EXISTS(SELECT 1 FROM billing_notices WHERE tenant_id=$1 AND reason='balance' AND active)", tenant).Scan(&active); err != nil || active != want {
			t.Fatal("DeliTools balance notice", active, want, err)
		}
	}
	checkNotice(true)
	checkNotice(true)
	if _, err = db.Pool.Exec(ctx, `INSERT INTO ledger(id,tenant_id,meter,kind,amount,source_key,created_at) VALUES($1,$2,'','credit',10,$3,now())`, key(), tenant, key()); err != nil {
		t.Fatal(err)
	}
	checkNotice(false)
	auth := &pb.AuthorizeRequest{TenantId: tenant, Paid: true, Capability: "translator", ConsumerAuthorization: pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_TENANT_KEY}
	a, err := c.Authorize(ctx, req(auth, "service"))
	if err != nil || !a.Msg.Allowed {
		t.Fatalf("tenant key: %v %v", a, err)
	}
	for i, tc := range []struct{ meter, price string }{{"translator", "0.001"}, {"translator", "0.002"}, {"extractor", "0.001"}} {
		now := timestamppb.New(time.Now())
		u := &pb.Usage{TenantId: tenant, ServiceId: service.Msg.Service.Id, EventKey: key(), Meter: tc.meter, Quantity: "1", OccurredAt: now, AiUsage: &pb.AiUsage{Model: "openai/gpt-6-luna", PriceEffectiveAt: now, Tokens: []*pb.AiTokenUsage{{Kind: pb.AiTokenKind_AI_TOKEN_KIND_INPUT, Tokens: 100, UsdPerToken: tc.price}}}}
		r, err := c.ReportUsage(ctx, req(&pb.ReportUsageRequest{RequestKey: key(), Usage: u}, "service"))
		if err != nil {
			t.Fatalf("report %d: %v", i, err)
		}
		for _, statement := range []string{
			"UPDATE ai_usage_prices SET cost=0 WHERE usage_id=$1",
			"UPDATE ai_usage_prices SET details='{}' WHERE usage_id=$1",
			"DELETE FROM ai_usage_prices WHERE usage_id=$1",
			"DELETE FROM usage_events WHERE id=$1",
		} {
			if _, err := db.Pool.Exec(ctx, statement, r.Msg.UsageId); err == nil {
				t.Fatal("mutable AI evidence", statement)
			}
		}
		replay, err := c.ReportUsage(ctx, req(&pb.ReportUsageRequest{RequestKey: key(), Usage: u}, "service"))
		if err != nil || replay.Msg.UsageId != r.Msg.UsageId {
			t.Fatal("duplicate effect", err)
		}
		listed, listErr := c.ListUsage(ctx, req(&pb.ListUsageRequest{TenantId: tenant}, "service"))
		if listErr != nil {
			t.Fatal(listErr)
		}
		found := false
		for _, event := range listed.Msg.Events {
			if event.Id == r.Msg.UsageId {
				found = true
				if !proto.Equal(event.AiUsage, u.AiUsage) {
					t.Fatal("listed AI evidence differs")
				}
			}
		}
		if !found {
			t.Fatal("accepted event missing from list")
		}
		correction := proto.Clone(u).(*pb.Usage)
		correction.EventKey = key()
		correction.CorrectionOf = r.Msg.UsageId
		correction.AiUsage.Tokens[0].UsdPerToken = "0.004"
		if _, err = c.CorrectUsage(ctx, req(&pb.CorrectUsageRequest{RequestKey: key(), Usage: correction}, "service")); connect.CodeOf(err) != connect.CodeInvalidArgument {
			t.Fatal("correction repriced request", err)
		}
		correction.AiUsage.Tokens[0].UsdPerToken = tc.price
		if _, err = c.CorrectUsage(ctx, req(&pb.CorrectUsageRequest{RequestKey: key(), Usage: correction}, "service")); err != nil {
			t.Fatal("unchanged snapshot rejected", err)
		}
		u.AiUsage.Tokens[0].Tokens++
		if _, err = c.ReportUsage(ctx, req(&pb.ReportUsageRequest{RequestKey: key(), Usage: u}, "service")); connect.CodeOf(err) != connect.CodeAlreadyExists {
			t.Fatal("changed evidence accepted", err)
		}
	}
	var cost string
	if err = db.Pool.QueryRow(ctx, "SELECT (-sum(amount))::text FROM ledger WHERE tenant_id=$1 AND kind='usage'", tenant).Scan(&cost); err != nil {
		t.Fatal(err)
	}
	if cost != "0.47500" && cost != "0.475" {
		t.Fatal("call-time prices or markups not applied", cost)
	}
	if _, err = db.Pool.Exec(ctx, "DELETE FROM members WHERE tenant_id=$1", tenant); err != nil {
		t.Fatal(err)
	}
	a, err = c.Authorize(ctx, req(auth, "service"))
	if err != nil || !a.Msg.Allowed {
		t.Fatal("service key depended on removed issuer", err)
	}
	auth.ConsumerAuthorization = pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_UNSPECIFIED
	auth.UserId = me.Msg.UserId
	a, err = c.Authorize(ctx, req(auth, "service"))
	if err != nil || a.Msg.Allowed {
		t.Fatal("removed user's execution accepted", err)
	}
	auth.ConsumerAuthorization = pb.ConsumerAuthorization_CONSUMER_AUTHORIZATION_SERVICE_KEYS
	auth.Paid = false
	auth.Capability = ""
	a, err = c.Authorize(ctx, req(auth, "service"))
	if err != nil || a.Msg.Allowed {
		t.Fatal("removed admin managed keys", err)
	}
	if _, err := db.Pool.Exec(ctx, "UPDATE tenants SET deleted_at=$2 WHERE id=$1", tenant, time.Now().AddDate(-10, 0, 0)); err != nil {
		t.Fatal(err)
	}
	w.Config.RawUsageRetentionSeconds = 3600
	w.Config.AccountingRetentionSeconds = 3600
	if err := w.Maintain(ctx, time.Now()); err != nil {
		t.Fatal("accounting retention", err)
	}
	var retained int
	if err := db.Pool.QueryRow(ctx, "SELECT count(*) FROM ai_usage_prices").Scan(&retained); err != nil || retained != 0 {
		t.Fatal("retained AI evidence", retained, err)
	}

}
