// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"encoding/json"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/worker"
	"github.com/jackc/pgx/v5/pgxpool"
)

func TestCatalogIdentityVersionReuseAndFeatureSnapshots(t *testing.T) {
	db, client, cfg := setup(t)
	ctx := context.Background()
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare catalog service", Service: &pb.Service{Key: "catalog-service", Name: "Catalog Service", ClientId: "service", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	serviceID := service.Msg.Service.Id
	renamed, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "rename catalog service", Service: &pb.Service{Key: "catalog-service", Name: "Renamed Service", ClientId: "service", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil || renamed.Msg.Service.Id != serviceID {
		t.Fatal("service identity changed", err)
	}
	meter := &pb.Meter{ServiceId: serviceID, Name: "requests", Key: "requests", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT}
	meterResult, err := client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare catalog meter", Meter: meter}, "owner"))
	if err != nil || meterResult.Msg.Meter.Id == "" {
		t.Fatal("meter identity missing", err)
	}
	changedMeter := *meter
	changedMeter.Aggregation = pb.MeterAggregation_METER_AGGREGATION_SUM
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "reject meter redefinition", Meter: &changedMeter}, "owner")); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("meter definition changed: %v", err)
	}
	access, err := client.PutFeature(ctx, req(&pb.PutFeatureRequest{RequestKey: key(), Reason: "declare access feature", Feature: &pb.Feature{ServiceId: serviceID, Key: "api-access", Name: "API access", Kind: pb.FeatureKind_FEATURE_KIND_ACCESS}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	limit, err := client.PutFeature(ctx, req(&pb.PutFeatureRequest{RequestKey: key(), Reason: "declare metered feature", Feature: &pb.Feature{ServiceId: serviceID, Key: "api-limit", Name: "API limit", Kind: pb.FeatureKind_FEATURE_KIND_METERED, MeterKey: "requests"}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	configuration, err := client.PutFeature(ctx, req(&pb.PutFeatureRequest{RequestKey: key(), Reason: "declare configuration feature", Feature: &pb.Feature{ServiceId: serviceID, Key: "retention", Name: "Retention", Kind: pb.FeatureKind_FEATURE_KIND_CONFIGURATION, ValueType: pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_INTEGER}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	enabled := true
	values := []*pb.FeatureValue{{FeatureId: access.Msg.Feature.Id, Enabled: &enabled}, {FeatureId: limit.Msg.Feature.Id, Limit: "10.00"}, {FeatureId: configuration.Msg.Feature.Id, Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Integer{Integer: 30}}}}
	planInput := &pb.Plan{Key: "free-catalog", Name: "Free Catalog", Free: true, DefaultFree: true, FeatureValues: values}
	plan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare free catalog plan", Plan: planInput, ReplaceFeatureValues: true}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	planID := plan.Msg.Plan.Id
	if plan.Msg.Plan.ActiveVersionId == "" {
		t.Fatal("plan version missing")
	}
	bad := *planInput
	bad.Id = planID
	bad.FeatureValues = []*pb.FeatureValue{{FeatureId: configuration.Msg.Feature.Id, Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Text{Text: "thirty"}}}}
	if _, err = client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "reject wrong configuration type", Plan: &bad, ReplaceFeatureValues: true}, "owner")); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("wrong configuration type accepted: %v", err)
	}
	priceInput := &pb.Price{Key: "requests-usd", PlanId: planID, ServiceId: serviceID, Meter: "requests", UnitPrice: "1.0", Included: "2.00", Currency: "USD"}
	priceA, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "select price A", Price: priceInput}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	versionA := activeVersion(t, ctx, db.Pool, planID)
	first, err := client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "First"}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	priceInput.UnitPrice = "1.00"
	priceAgain, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "replay semantic price A", Price: priceInput}, "owner"))
	if err != nil || priceAgain.Msg.Price.Id != priceA.Msg.Price.Id || priceAgain.Msg.Price.StableId != priceA.Msg.Price.StableId || activeVersion(t, ctx, db.Pool, planID) != versionA {
		t.Fatal("semantic no-op created a version", err)
	}
	priceInput.UnitPrice = "2"
	priceB, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "select price B", Price: priceInput}, "owner"))
	if err != nil || priceB.Msg.Price.Id == priceA.Msg.Price.Id {
		t.Fatal("price B did not version", err)
	}
	versionB := activeVersion(t, ctx, db.Pool, planID)
	if versionB == versionA {
		t.Fatal("plan price change did not version")
	}
	second, err := client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "Second"}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	priceInput.UnitPrice = "1"
	priceRestored, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "restore price A", Price: priceInput}, "owner"))
	if err != nil || priceRestored.Msg.Price.Id != priceA.Msg.Price.Id || activeVersion(t, ctx, db.Pool, planID) != versionA {
		t.Fatal("A to B to A did not reuse A", err)
	}
	third, err := client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "Third"}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ tenant, version, price string }{{first.Msg.Tenant.Id, versionA, priceA.Msg.Price.Id}, {second.Msg.Tenant.Id, versionB, priceB.Msg.Price.Id}, {third.Msg.Tenant.Id, versionA, priceA.Msg.Price.Id}} {
		var selected, actualPrice string
		if err = db.Pool.QueryRow(ctx, "SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1 AND ends_at IS NULL", tc.tenant).Scan(&selected); err != nil {
			t.Fatal(err)
		}
		if err = db.Pool.QueryRow(ctx, "SELECT price_id::text FROM entitlements WHERE tenant_id=$1 AND ends_at IS NULL", tc.tenant).Scan(&actualPrice); err != nil {
			t.Fatal(err)
		}
		if selected != tc.version || actualPrice != tc.price {
			t.Fatalf("tenant snapshot changed: got %s/%s, want %s/%s", selected, actualPrice, tc.version, tc.price)
		}
	}
	disabled := false
	planInput.Id = planID
	planInput.FeatureValues = []*pb.FeatureValue{{FeatureId: access.Msg.Feature.Id, Enabled: &disabled}, {FeatureId: limit.Msg.Feature.Id, Limit: "10"}, {FeatureId: configuration.Msg.Feature.Id, Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Integer{Integer: 30}}}}
	changedPlan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "change free feature value", Plan: planInput, ReplaceFeatureValues: true}, "owner"))
	if err != nil || changedPlan.Msg.Plan.ActiveVersionId == versionA {
		t.Fatal("feature change did not version", err)
	}
	changedVersion := changedPlan.Msg.Plan.ActiveVersionId
	if _, err = client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: first.Msg.Tenant.Id, PlanId: planID, PriceIds: []string{priceA.Msg.Price.Id}, ReturnUrl: cfg.ReturnURLs[0]}, "owner")); err != nil {
		t.Fatal("free feature refresh", err)
	}
	var refreshed string
	if err = db.Pool.QueryRow(ctx, "SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1 AND ends_at IS NULL", first.Msg.Tenant.Id).Scan(&refreshed); err != nil || refreshed != changedVersion {
		t.Fatal("free refresh did not select feature version", err)
	}
	if err = db.Pool.QueryRow(ctx, "SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1 AND ends_at IS NULL", third.Msg.Tenant.Id).Scan(&refreshed); err != nil || refreshed != versionA {
		t.Fatal("existing tenant moved with catalog edit", err)
	}
	planInput.FeatureValues = values
	planInput.Id = planID
	planInput.Name = "Renamed Free Catalog"
	renamedPlan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "rename plan", Plan: planInput, ReplaceFeatureValues: true}, "owner"))
	if err != nil || renamedPlan.Msg.Plan.Id != planID || renamedPlan.Msg.Plan.ActiveVersionId != versionA {
		t.Fatal("display rename changed billing version", err)
	}
	catalog, err := client.GetCatalog(ctx, req(&pb.GetCatalogRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	foundMeter, foundPlan, foundFeature := false, false, false
	for _, item := range catalog.Msg.Meters {
		if item.Id == meterResult.Msg.Meter.Id && item.Key == "requests" && item.EventType == "request" && item.Aggregation == pb.MeterAggregation_METER_AGGREGATION_COUNT {
			foundMeter = true
		}
	}
	for _, item := range catalog.Msg.Plans {
		if item.Id == planID && item.ActiveVersionId == versionA && len(item.FeatureValues) == 3 {
			foundPlan = true
		}
	}
	for _, item := range catalog.Msg.Features {
		if item.Id == configuration.Msg.Feature.Id && item.ValueType == pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_INTEGER {
			foundFeature = true
		}
	}
	if !foundMeter || !foundPlan || !foundFeature {
		t.Fatal("catalog read omitted stable definitions")
	}
	addon, err := client.PutAddOn(ctx, req(&pb.PutAddOnRequest{RequestKey: key(), Reason: "declare conflicting add-on", AddOn: &pb.AddOn{ServiceId: serviceID, Key: "conflicting-addon", Name: "Conflicting Add-on", FeatureValues: []*pb.FeatureValue{{FeatureId: access.Msg.Feature.Id, Enabled: &enabled}}}, ReplaceFeatureValues: true}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	addonPrice, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "declare add-on fee", Price: &pb.Price{Key: "addon-fee", AddOnId: addon.Msg.AddOn.Id, ServiceId: serviceID, Meter: "base", UnitPrice: "5", Included: "0", Currency: "USD", ExternalPriceId: "pri_conflict", Kind: pb.PriceKind_PRICE_KIND_RECURRING}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: third.Msg.Tenant.Id, PlanId: planID, PriceIds: []string{priceA.Msg.Price.Id, addonPrice.Msg.Price.Id}, ReturnUrl: cfg.ReturnURLs[0]}, "owner"))
	if connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("conflicting plan and add-on features accepted: %v", err)
	}
}

func TestScheduledFreeDowngradeKeepsRequestedVersion(t *testing.T) {
	db, client, cfg := setup(t)
	ctx := context.Background()
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare downgrade service", Service: &pb.Service{Key: "downgrade-service", Name: "Downgrade Service", ClientId: "service", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	serviceID := service.Msg.Service.Id
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare downgrade meter", Meter: &pb.Meter{ServiceId: serviceID, Name: "units", Key: "units"}}, "owner")); err != nil {
		t.Fatal(err)
	}
	if _, err = client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare default free plan", Plan: &pb.Plan{Key: "default-free", Name: "Default Free", Free: true, DefaultFree: true}}, "owner")); err != nil {
		t.Fatal(err)
	}
	me, err := client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	tenant := me.Msg.Tenants[0].Id
	plans := make([]string, 2)
	prices := make([]string, 2)
	for i, rank := range []int32{10, 5} {
		plan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare free tier", Plan: &pb.Plan{Key: []string{"high-free", "low-free"}[i], Name: []string{"High Free", "Low Free"}[i], Rank: rank, Free: true}}, "owner"))
		if err != nil {
			t.Fatal(err)
		}
		plans[i] = plan.Msg.Plan.Id
		price, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "declare tier allowance", Price: &pb.Price{Key: "units-usd", PlanId: plans[i], ServiceId: serviceID, Meter: "units", UnitPrice: "1", Included: "10", Currency: "USD"}}, "owner"))
		if err != nil {
			t.Fatal(err)
		}
		prices[i] = price.Msg.Price.Id
	}
	if _, err = client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: tenant, PlanId: plans[0], PriceIds: []string{prices[0]}, ReturnUrl: cfg.ReturnURLs[0]}, "owner")); err != nil {
		t.Fatal(err)
	}
	change, err := client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: tenant, PlanId: plans[1], PriceIds: []string{prices[1]}, ReturnUrl: cfg.ReturnURLs[0]}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	requestedVersion := activeVersion(t, ctx, db.Pool, plans[1])
	var before, timeState string
	var end time.Time
	if err = db.Pool.QueryRow(ctx, "SELECT cs.version_id::text,t.period_end,j.state FROM tenants t JOIN tenant_catalog_selections cs ON cs.tenant_id=t.id AND cs.ends_at IS NULL JOIN jobs j ON j.id=$2 WHERE t.id=$1", tenant, change.Msg.JobId).Scan(&before, &end, &timeState); err != nil {
		t.Fatal(err)
	}
	if before == requestedVersion || timeState != "pending" {
		t.Fatal("downgrade activated early")
	}
	if _, err = client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "change future tier", Price: &pb.Price{Key: "units-usd", PlanId: plans[1], ServiceId: serviceID, Meter: "units", UnitPrice: "2", Included: "20", Currency: "USD"}}, "owner")); err != nil {
		t.Fatal(err)
	}
	if activeVersion(t, ctx, db.Pool, plans[1]) == requestedVersion {
		t.Fatal("future tier did not version")
	}
	w := &worker.Worker{DB: db, Config: cfg}
	if err = w.Maintain(ctx, end.Add(time.Second)); err != nil {
		t.Fatal(err)
	}
	var selected, entitled string
	if err = db.Pool.QueryRow(ctx, "SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1 AND ends_at IS NULL", tenant).Scan(&selected); err != nil {
		t.Fatal(err)
	}
	if err = db.Pool.QueryRow(ctx, "SELECT price_id::text FROM entitlements WHERE tenant_id=$1 AND ends_at IS NULL", tenant).Scan(&entitled); err != nil {
		t.Fatal(err)
	}
	if selected != requestedVersion || entitled != prices[1] {
		t.Fatalf("scheduled change drifted: got %s/%s", selected, entitled)
	}
}

func TestCatalogPriceReplacesLegacyTuple(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare legacy price service", Service: &pb.Service{Key: "legacy-price-service", Name: "Legacy Price Service", ClientId: "legacy-price", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	plan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare legacy price plan", Plan: &pb.Plan{Key: "legacy-price-plan", Name: "Legacy Price Plan", Free: true, DefaultFree: true}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare legacy price meter", Meter: &pb.Meter{ServiceId: service.Msg.Service.Id, Name: "requests"}}, "owner")); err != nil {
		t.Fatal(err)
	}
	legacyPriceID := key()
	if _, err = db.Pool.Exec(ctx, `INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind)
VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`, legacyPriceID, plan.Msg.Plan.Id, service.Msg.Service.Id, "requests", "1", "0", "USD", "", time.Now().UTC(), 1); err != nil {
		t.Fatal(err)
	}
	price, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "replace legacy price", Price: &pb.Price{Key: "requests-catalog", PlanId: plan.Msg.Plan.Id, ServiceId: service.Msg.Service.Id, Meter: "requests", UnitPrice: "2", Included: "0", Currency: "USD"}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	var raw []byte
	if err = db.Pool.QueryRow(ctx, "SELECT definition FROM catalog_versions WHERE id=(SELECT active_version_id FROM catalog_entities WHERE id=$1)", plan.Msg.Plan.Id).Scan(&raw); err != nil {
		t.Fatal(err)
	}
	var definition struct {
		Prices []string `json:"prices"`
	}
	if err = json.Unmarshal(raw, &definition); err != nil {
		t.Fatal(err)
	}
	if len(definition.Prices) != 1 || definition.Prices[0] != price.Msg.Price.Id {
		t.Fatalf("legacy price remained active: prices=%v legacy=%s", definition.Prices, legacyPriceID)
	}
}

func TestCatalogReadMarksActivePriceVersion(t *testing.T) {
	_, client, _ := setup(t)
	ctx := context.Background()
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare active price service", Service: &pb.Service{Key: "active-price-service", Name: "Active Price Service", ClientId: "active-price", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	plan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare active price plan", Plan: &pb.Plan{Key: "active-price-plan", Name: "Active Price Plan", Free: true, DefaultFree: true}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare active price meter", Meter: &pb.Meter{ServiceId: service.Msg.Service.Id, Name: "requests"}}, "owner")); err != nil {
		t.Fatal(err)
	}
	priceInput := &pb.Price{Key: "active-price", PlanId: plan.Msg.Plan.Id, ServiceId: service.Msg.Service.Id, Meter: "requests", UnitPrice: "1", Included: "0", Currency: "USD"}
	first, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "create first active price", Price: priceInput}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	priceInput.UnitPrice = "2"
	second, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "create second active price", Price: priceInput}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	catalog, err := client.GetCatalog(ctx, req(&pb.GetCatalogRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	var firstActive, secondActive bool
	for _, price := range catalog.Msg.Prices {
		switch price.Id {
		case first.Msg.Price.Id:
			firstActive = price.Active
		case second.Msg.Price.Id:
			secondActive = price.Active
		}
	}
	if firstActive || !secondActive {
		t.Fatalf("catalog active markers were incorrect: first=%t second=%t", firstActive, secondActive)
	}
}

func TestMeterCatalogRejectsInvalidDefinitions(t *testing.T) {
	_, client, _ := setup(t)
	ctx := context.Background()
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare meter validation service", Service: &pb.Service{Key: "meter-validation-service", Name: "Meter Validation Service", ClientId: "meter-validation", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	serviceID := service.Msg.Service.Id
	cases := []struct {
		name   string
		meter  *pb.Meter
		legacy bool
	}{
		{name: "sum without value", meter: &pb.Meter{ServiceId: serviceID, Name: "sum-without-value", EventType: "usage", Aggregation: pb.MeterAggregation_METER_AGGREGATION_SUM}},
		{name: "count with value", meter: &pb.Meter{ServiceId: serviceID, Name: "count-with-value", EventType: "usage", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, ValueAttribute: "value"}},
		{name: "empty event type", meter: &pb.Meter{ServiceId: serviceID, Name: "empty-event", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT}},
		{name: "empty membership filter", meter: &pb.Meter{ServiceId: serviceID, Name: "empty-in", EventType: "usage", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, Filters: []*pb.MeterFilter{{Attribute: "region", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_IN}}}},
		{name: "legacy meter", meter: &pb.Meter{ServiceId: serviceID, Name: "legacy-meter"}, legacy: true},
	}
	for _, tc := range cases {
		_, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "validate " + tc.name, Meter: tc.meter}, "owner"))
		if tc.legacy {
			if err != nil {
				t.Fatalf("legacy meter rejected: %v", err)
			}
			continue
		}
		if connect.CodeOf(err) != connect.CodeInvalidArgument {
			t.Fatalf("%s was not rejected as invalid input: %v", tc.name, err)
		}
	}
	valid, err := client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "accept valid meter", Meter: &pb.Meter{ServiceId: serviceID, Name: "valid-meter", EventType: "usage", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, Filters: []*pb.MeterFilter{{Attribute: "region", Operator: pb.MeterFilterOperator_METER_FILTER_OPERATOR_EQUAL, Values: []string{"us"}}}}}, "owner"))
	if err != nil || valid.Msg.Meter.Id == "" {
		t.Fatalf("valid meter rejected: %v", err)
	}
}

func TestLegacyPlanMutationSnapshotsTenants(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	if _, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare default plan", Plan: &pb.Plan{Key: "legacy-default", Name: "Legacy Default", Free: true, DefaultFree: true}}, "owner")); err != nil {
		t.Fatal(err)
	}
	me, err := client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil || len(me.Msg.Tenants) == 0 {
		t.Fatal("personal tenant was not created", err)
	}
	tenantID := me.Msg.Tenants[0].Id
	legacyPlanID := key()
	if _, err = db.Pool.Exec(ctx, `INSERT INTO plans(id,name,rank,free,is_default,postpaid) VALUES($1,$2,$3,$4,$5,$6)`, legacyPlanID, "Legacy Plan", 100, true, false, false); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, "UPDATE tenants SET plan_id=$2 WHERE id=$1", tenantID, legacyPlanID); err != nil {
		t.Fatal(err)
	}
	updated, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "mutate legacy plan", Plan: &pb.Plan{Id: legacyPlanID, Name: "Updated Legacy Plan", Rank: 101, Free: false}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	var selected, selectedFree string
	if err = db.Pool.QueryRow(ctx, `SELECT cs.version_id::text,cv.definition->>'free'
FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id
WHERE cs.tenant_id=$1 AND cs.product_id=$2 AND cs.ends_at IS NULL`, tenantID, legacyPlanID).Scan(&selected, &selectedFree); err != nil {
		t.Fatal(err)
	}
	if selected == updated.Msg.Plan.ActiveVersionId || selectedFree != "true" {
		t.Fatalf("legacy tenant did not retain old plan version: selected=%s free=%s active=%s", selected, selectedFree, updated.Msg.Plan.ActiveVersionId)
	}
}

func TestSubscriptionRejectsPartialActivePriceSet(t *testing.T) {
	_, client, cfg := setup(t)
	ctx := context.Background()
	if _, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare default subscription plan", Plan: &pb.Plan{Key: "subscription-default", Name: "Subscription Default", Free: true, DefaultFree: true}}, "owner")); err != nil {
		t.Fatal(err)
	}
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare subscription service", Service: &pb.Service{Key: "subscription-service", Name: "Subscription Service", ClientId: "service", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	serviceID := service.Msg.Service.Id
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare subscription meter", Meter: &pb.Meter{ServiceId: serviceID, Name: "requests", Key: "requests"}}, "owner")); err != nil {
		t.Fatal(err)
	}
	plan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare subscription plan", Plan: &pb.Plan{Key: "partial-price-plan", Name: "Partial Price Plan", Rank: 1, Free: true}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	priceIDs := make([]string, 2)
	for i, priceKey := range []string{"requests-base", "requests-extra"} {
		price, createErr := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "declare subscription price", Price: &pb.Price{Key: priceKey, PlanId: plan.Msg.Plan.Id, ServiceId: serviceID, Meter: "requests", UnitPrice: "1", Included: "0", Currency: "USD"}}, "owner"))
		if createErr != nil {
			t.Fatal(createErr)
		}
		priceIDs[i] = price.Msg.Price.Id
	}
	me, err := client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: me.Msg.Tenants[0].Id, PlanId: plan.Msg.Plan.Id, PriceIds: []string{priceIDs[0]}, ReturnUrl: cfg.ReturnURLs[0]}, "owner"))
	if connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("partial active price set accepted: %v", err)
	}
}

func TestSubscriptionComparesLegacyAndCatalogPricesInOneDomain(t *testing.T) {
	db, client, cfg := setup(t)
	ctx := context.Background()
	if _, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare default mixed plan", Plan: &pb.Plan{Key: "mixed-default", Name: "Mixed Default", Free: true, DefaultFree: true}}, "owner")); err != nil {
		t.Fatal(err)
	}
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare mixed price service", Service: &pb.Service{Key: "mixed-price-service", Name: "Mixed Price Service", ClientId: "mixed-price", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	serviceID := service.Msg.Service.Id
	for _, meter := range []string{"legacy-units", "requests"} {
		if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare mixed price meter", Meter: &pb.Meter{ServiceId: serviceID, Name: meter}}, "owner")); err != nil {
			t.Fatal(err)
		}
	}
	plan, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare mixed price plan", Plan: &pb.Plan{Key: "mixed-price-plan", Name: "Mixed Price Plan", Rank: 1, Free: true}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	legacyPriceID := key()
	if _, err = db.Pool.Exec(ctx, `INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind)
VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`, legacyPriceID, plan.Msg.Plan.Id, serviceID, "legacy-units", "1", "0", "USD", "", time.Now().UTC(), 1); err != nil {
		t.Fatal(err)
	}
	catalogPrice, err := client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "declare keyed mixed price", Price: &pb.Price{Key: "requests-usd", PlanId: plan.Msg.Plan.Id, ServiceId: serviceID, Meter: "requests", UnitPrice: "2", Included: "0", Currency: "USD"}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	me, err := client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	tenant := me.Msg.Tenants[0].Id
	_, err = client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: tenant, PlanId: plan.Msg.Plan.Id, PriceIds: []string{legacyPriceID}, ReturnUrl: cfg.ReturnURLs[0]}, "owner"))
	if connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("legacy-only selection omitted the active catalog price: %v", err)
	}
	if _, err = client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: tenant, PlanId: plan.Msg.Plan.Id, PriceIds: []string{legacyPriceID, catalogPrice.Msg.Price.Id}, ReturnUrl: cfg.ReturnURLs[0]}, "owner")); err != nil {
		t.Fatalf("complete legacy and catalog price set rejected: %v", err)
	}
}

func TestLegacyAddOnMutationSnapshotsTenants(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	if _, err := client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "declare default add-on plan", Plan: &pb.Plan{Key: "addon-default", Name: "Add-on Default", Free: true, DefaultFree: true}}, "owner")); err != nil {
		t.Fatal(err)
	}
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare legacy add-on service", Service: &pb.Service{Key: "legacy-addon-service", Name: "Legacy Add-on Service", ClientId: "legacy-addon", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	serviceID := service.Msg.Service.Id
	feature, err := client.PutFeature(ctx, req(&pb.PutFeatureRequest{RequestKey: key(), Reason: "declare add-on feature", Feature: &pb.Feature{ServiceId: serviceID, Key: "priority", Name: "Priority", Kind: pb.FeatureKind_FEATURE_KIND_ACCESS}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	me, err := client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil || len(me.Msg.Tenants) == 0 {
		t.Fatal("personal tenant was not created", err)
	}
	tenantID := me.Msg.Tenants[0].Id
	// Recreate the rows an upgraded database holds: migration 00006 backfills
	// only the add-on identity, without a version or tenant selection.
	addonID, priceID := key(), key()
	if _, err = db.Pool.Exec(ctx, "INSERT INTO addons(id,service_id,name) VALUES($1,$2,$3)", addonID, serviceID, "Legacy Add-on"); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, "INSERT INTO catalog_entities(id,kind,scope,key) VALUES($1,'addon',$2,$3)", addonID, serviceID, "legacy-addon-fixture"); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, `INSERT INTO prices(id,addon_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind)
VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`, priceID, addonID, serviceID, "base", "5", "0", "USD", "pri_legacy_addon", time.Now().UTC(), 2); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,$4)", key(), tenantID, priceID, time.Now().UTC().Add(-time.Hour)); err != nil {
		t.Fatal(err)
	}
	enabled := true
	updated, err := client.PutAddOn(ctx, req(&pb.PutAddOnRequest{RequestKey: key(), Reason: "mutate legacy add-on", AddOn: &pb.AddOn{Id: addonID, ServiceId: serviceID, Name: "Legacy Add-on", FeatureValues: []*pb.FeatureValue{{FeatureId: feature.Msg.Feature.Id, Enabled: &enabled}}}, ReplaceFeatureValues: true}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	var selected string
	var features, prices int
	if err = db.Pool.QueryRow(ctx, `SELECT cs.version_id::text,jsonb_array_length(cv.definition->'features'),jsonb_array_length(cv.definition->'prices')
FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id
WHERE cs.tenant_id=$1 AND cs.product_id=$2 AND cs.ends_at IS NULL`, tenantID, addonID).Scan(&selected, &features, &prices); err != nil {
		t.Fatal("legacy add-on tenant has no pinned add-on version", err)
	}
	if selected == updated.Msg.AddOn.ActiveVersionId || features != 0 || prices != 1 {
		t.Fatalf("legacy tenant did not retain the pre-mutation add-on: selected=%s active=%s features=%d prices=%d", selected, updated.Msg.AddOn.ActiveVersionId, features, prices)
	}
}

func TestLegacyMeterRequestPersistsDefaultDefinition(t *testing.T) {
	_, client, _ := setup(t)
	ctx := context.Background()
	service, err := client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "declare legacy meter service", Service: &pb.Service{Key: "legacy-meter-service", Name: "Legacy Meter Service", ClientId: "legacy-meter", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	serviceID := service.Msg.Service.Id
	legacy, err := client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare name-only meter", Meter: &pb.Meter{ServiceId: serviceID, Name: "units"}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	if legacy.Msg.Meter.Id == "" || legacy.Msg.Meter.Aggregation != pb.MeterAggregation_METER_AGGREGATION_SUM {
		t.Fatalf("name-only meter response was not normalized: %+v", legacy.Msg.Meter)
	}
	catalog, err := client.GetCatalog(ctx, req(&pb.GetCatalogRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, meter := range catalog.Msg.Meters {
		if meter.Name != "units" {
			continue
		}
		found = true
		if meter.Id != legacy.Msg.Meter.Id || meter.Key != "units" || meter.Aggregation != pb.MeterAggregation_METER_AGGREGATION_SUM {
			t.Fatalf("catalog read differs from the name-only meter response: got %+v, want %+v", meter, legacy.Msg.Meter)
		}
	}
	if !found {
		t.Fatal("catalog read omitted the name-only meter")
	}
	typed, err := client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare typed meter", Meter: &pb.Meter{ServiceId: serviceID, Name: "requests", EventType: "request", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	reregistered, err := client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "register typed meter by name", Meter: &pb.Meter{ServiceId: serviceID, Name: "requests"}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	if reregistered.Msg.Meter.Id != typed.Msg.Meter.Id || reregistered.Msg.Meter.Aggregation != pb.MeterAggregation_METER_AGGREGATION_COUNT || reregistered.Msg.Meter.EventType != "request" {
		t.Fatalf("name-only request misreported a stored meter: %+v", reregistered.Msg.Meter)
	}
}

func activeVersion(t *testing.T, ctx context.Context, db *pgxpool.Pool, product string) string {
	t.Helper()
	var version string
	if err := db.QueryRow(ctx, "SELECT active_version_id::text FROM catalog_entities WHERE id=$1", product).Scan(&version); err != nil {
		t.Fatal(err)
	}
	return version
}
