// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"slices"
	"sync"
	"testing"
	"time"

	"connectrpc.com/connect"
	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/protos/delcore/v1/delcorev1connect"
	"github.com/delinoio/apps/servers/delcore-api/internal/config"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
	"github.com/delinoio/apps/servers/delcore-api/internal/provider"
	"github.com/delinoio/apps/servers/delcore-api/internal/rpc"
	"github.com/delinoio/apps/servers/delcore-api/internal/worker"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"
)

// featureFixture declares one metered service with access, metered, and
// configuration features and pins three tenants to different plan versions:
// personal (before features existed), alpha (version 1), and beta (version 2).
// The server clock starts in the previous UTC month so tests can move into the
// next billing period without running ahead of the database clock that gates
// worker job claims.
type featureFixture struct {
	db       *postgres.Store
	client   delcorev1connect.DelcoreServiceClient
	cfg      config.Config
	worker   *worker.Worker
	owner    string
	service  string
	rival    string
	plan     string
	personal string
	alpha    string
	beta     string
	features map[string]string
	start    time.Time
	clockMu  sync.Mutex
	clock    time.Time
}

func (f *featureFixture) now() time.Time {
	f.clockMu.Lock()
	defer f.clockMu.Unlock()
	return f.clock
}

func (f *featureFixture) setClock(at time.Time) {
	f.clockMu.Lock()
	defer f.clockMu.Unlock()
	f.clock = at.UTC()
}

func monthStart(at time.Time) time.Time {
	at = at.UTC()
	return time.Date(at.Year(), at.Month(), 1, 0, 0, 0, 0, time.UTC)
}

func newFeatureFixture(t *testing.T) *featureFixture {
	t.Helper()
	db, _, cfg := setup(t)
	f := &featureFixture{db: db, cfg: cfg, features: map[string]string{}}
	f.start = monthStart(time.Now()).AddDate(0, 0, -3).Add(time.Hour)
	f.setClock(f.start)
	server := &rpc.Server{DB: db, Auth: verifier{}, Config: cfg, Log: slog.New(slog.NewTextHandler(io.Discard, nil)), Now: f.now}
	path, handler := server.Handler()
	mux := http.NewServeMux()
	mux.Handle(path, handler)
	httpServer := httptest.NewServer(mux)
	t.Cleanup(httpServer.Close)
	f.client = delcorev1connect.NewDelcoreServiceClient(httpServer.Client(), httpServer.URL)
	f.worker = &worker.Worker{DB: db, Config: cfg, Log: slog.New(slog.NewTextHandler(io.Discard, nil))}
	ctx := context.Background()
	f.owner, f.personal = bootstrap(t, f.client)
	service, err := f.client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "register featured service", Service: &pb.Service{Key: "featured", Name: "featured", ClientId: "service", Capabilities: []string{"execute"}, Scopes: []string{"authorize", "usage:write", "usage:read"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	f.service = service.Msg.Service.Id
	rival, err := f.client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "register rival service", Service: &pb.Service{Key: "rival", Name: "rival", ClientId: "other", Capabilities: []string{"execute"}, Scopes: []string{"authorize", "usage:write", "usage:read"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	f.rival = rival.Msg.Service.Id
	for _, m := range []*pb.Meter{
		{Name: "calls", EventType: "call", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, GroupBy: []string{"model"}},
		{Name: "latency", EventType: "sample", Aggregation: pb.MeterAggregation_METER_AGGREGATION_AVG, ValueAttribute: "ms", GroupBy: []string{"model"}},
	} {
		m.ServiceId, m.Key = f.service, m.Name
		if _, err = f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare feature meter", Meter: m}, "owner")); err != nil {
			t.Fatal(err)
		}
	}
	configuration := func(valueType pb.ConfigurationValueType) *pb.Feature {
		return &pb.Feature{Kind: pb.FeatureKind_FEATURE_KIND_CONFIGURATION, ValueType: valueType}
	}
	for featureKey, feature := range map[string]*pb.Feature{
		"api":            {Kind: pb.FeatureKind_FEATURE_KIND_ACCESS},
		"beta":           {Kind: pb.FeatureKind_FEATURE_KIND_ACCESS},
		"ungranted":      {Kind: pb.FeatureKind_FEATURE_KIND_ACCESS},
		"addon-only":     {Kind: pb.FeatureKind_FEATURE_KIND_ACCESS},
		"calls-limit":    {Kind: pb.FeatureKind_FEATURE_KIND_METERED, MeterKey: "calls"},
		"latency-limit":  {Kind: pb.FeatureKind_FEATURE_KIND_METERED, MeterKey: "latency"},
		"config-boolean": configuration(pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_BOOLEAN),
		"config-integer": configuration(pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_INTEGER),
		"config-decimal": configuration(pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_DECIMAL),
		"config-string":  configuration(pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING),
		"config-list":    configuration(pb.ConfigurationValueType_CONFIGURATION_VALUE_TYPE_STRING_LIST),
	} {
		feature.ServiceId, feature.Key, feature.Name = f.service, featureKey, featureKey
		declared, err := f.client.PutFeature(ctx, req(&pb.PutFeatureRequest{RequestKey: key(), Reason: "declare feature", Feature: feature}, "owner"))
		if err != nil {
			t.Fatal(featureKey, err)
		}
		f.features[featureKey] = declared.Msg.Feature.Id
	}
	if _, err = f.client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "declare rival meter", Meter: &pb.Meter{ServiceId: f.rival, Key: "calls", Name: "calls", EventType: "call", Aggregation: pb.MeterAggregation_METER_AGGREGATION_COUNT, GroupBy: []string{"model"}}}, "owner")); err != nil {
		t.Fatal(err)
	}
	if _, err = f.client.PutFeature(ctx, req(&pb.PutFeatureRequest{RequestKey: key(), Reason: "declare rival feature", Feature: &pb.Feature{ServiceId: f.rival, Key: "rival-only", Name: "rival-only", Kind: pb.FeatureKind_FEATURE_KIND_ACCESS}}, "owner")); err != nil {
		t.Fatal(err)
	}
	on, off := true, false
	planV1 := []*pb.FeatureValue{
		{FeatureId: f.features["api"], Enabled: &on},
		{FeatureId: f.features["beta"], Enabled: &off},
		{FeatureId: f.features["calls-limit"], Limit: "3"},
		{FeatureId: f.features["latency-limit"], Limit: "100"},
		{FeatureId: f.features["config-boolean"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Boolean{Boolean: true}}},
		{FeatureId: f.features["config-integer"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Integer{Integer: 30}}},
		{FeatureId: f.features["config-decimal"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Decimal{Decimal: "1.250"}}},
		{FeatureId: f.features["config-string"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Text{Text: "eu"}}},
		{FeatureId: f.features["config-list"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_StringList{StringList: &pb.StringList{Values: []string{"small", "large"}}}}},
	}
	plan, err := f.client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "grant version 1 features", Plan: &pb.Plan{Key: "free", Name: "free", Free: true, DefaultFree: true, FeatureValues: planV1}, ReplaceFeatureValues: true}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	f.plan = plan.Msg.Plan.Id
	f.alpha = f.createTenant(t, "Alpha")
	planV2 := []*pb.FeatureValue{
		{FeatureId: f.features["api"], Enabled: &on},
		{FeatureId: f.features["calls-limit"], Limit: "5"},
		{FeatureId: f.features["latency-limit"], Limit: "100"},
		{FeatureId: f.features["config-boolean"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Boolean{Boolean: false}}},
		{FeatureId: f.features["config-integer"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Integer{Integer: 90}}},
		{FeatureId: f.features["config-decimal"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Decimal{Decimal: "2"}}},
		{FeatureId: f.features["config-string"], Configuration: &pb.ConfigurationValue{Value: &pb.ConfigurationValue_Text{Text: "us"}}},
	}
	if _, err = f.client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "grant version 2 features", Plan: &pb.Plan{Key: "free", Name: "free", Free: true, DefaultFree: true, FeatureValues: planV2}, ReplaceFeatureValues: true}, "owner")); err != nil {
		t.Fatal(err)
	}
	f.beta = f.createTenant(t, "Beta")
	return f
}

func (f *featureFixture) createTenant(t *testing.T, name string) string {
	t.Helper()
	created, err := f.client.CreateTenant(context.Background(), req(&pb.CreateTenantRequest{RequestKey: key(), Name: name}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	return created.Msg.Tenant.Id
}

func attrText(s string) *pb.AttributeValue {
	return &pb.AttributeValue{Value: &pb.AttributeValue_Text{Text: s}}
}

func attrNumber(s string) *pb.AttributeValue {
	return &pb.AttributeValue{Value: &pb.AttributeValue_Number{Number: s}}
}

func model(name string) map[string]*pb.AttributeValue {
	return map[string]*pb.AttributeValue{"model": attrText(name)}
}

func typedReason(err error) pb.ErrorReason {
	var ce *connect.Error
	if !errors.As(err, &ce) {
		return pb.ErrorReason_ERROR_REASON_UNSPECIFIED
	}
	for _, detail := range ce.Details() {
		if value, e := detail.Value(); e == nil {
			if typed, ok := value.(*pb.ErrorDetail); ok {
				return typed.Reason
			}
		}
	}
	return pb.ErrorReason_ERROR_REASON_UNSPECIFIED
}

type authorization struct {
	token, tenant, user, capability, feature string
	paid                                     bool
	groups                                   map[string]*pb.AttributeValue
}

func (f *featureFixture) authorize(a authorization) (*pb.AuthorizeResponse, error) {
	if a.token == "" {
		a.token = "service"
	}
	if a.user == "" {
		a.user = f.owner
	}
	if a.capability == "" {
		a.capability = "execute"
	}
	response, err := f.client.Authorize(context.Background(), req(&pb.AuthorizeRequest{TenantId: a.tenant, UserId: a.user, Capability: a.capability, Paid: a.paid, FeatureKey: a.feature, GroupValues: a.groups}, a.token))
	if err != nil {
		return nil, err
	}
	return response.Msg, nil
}

// feature authorizes one feature for the owner and fails on transport errors.
func (f *featureFixture) feature(t *testing.T, tenant, featureKey string, groups map[string]*pb.AttributeValue) *pb.AuthorizeResponse {
	t.Helper()
	response, err := f.authorize(authorization{tenant: tenant, feature: featureKey, groups: groups})
	if err != nil {
		t.Fatalf("authorize %s: %v", featureKey, err)
	}
	return response
}

// ingest reports one event of the featured service one minute before the
// server clock and returns the accepted event identity.
func (f *featureFixture) ingest(t *testing.T, token, tenant, eventKey, eventType string, attributes map[string]*pb.AttributeValue) *pb.IngestEventResponse {
	t.Helper()
	service := f.service
	if token == "other" {
		service = f.rival
	}
	response, err := f.client.IngestEvent(context.Background(), req(&pb.IngestEventRequest{RequestKey: key(), Event: &pb.Event{TenantId: tenant, ServiceId: service, EventKey: eventKey, EventType: eventType, OccurredAt: timestamppb.New(f.now().Add(-time.Minute)), Attributes: attributes}}, token))
	if err != nil {
		t.Fatalf("ingest %s: %v", eventKey, err)
	}
	return response.Msg
}

func (f *featureFixture) correct(t *testing.T, tenant, eventKey string, expected int32, kind pb.EventCorrectionKind, eventType string, attributes map[string]*pb.AttributeValue) {
	t.Helper()
	if _, err := f.client.CorrectEvent(context.Background(), req(&pb.CorrectEventRequest{RequestKey: key(), TenantId: tenant, EventKey: eventKey, ExpectedRevision: expected, Kind: kind, EventType: eventType, Attributes: attributes}, "service")); err != nil {
		t.Fatalf("correct %s: %v", eventKey, err)
	}
}

func (f *featureFixture) process(t *testing.T) {
	t.Helper()
	if _, err := f.worker.ProcessMetering(context.Background(), 10000); err != nil {
		t.Fatal(err)
	}
}

func hasReason(reasons []pb.BlockReason, want pb.BlockReason) bool {
	return slices.Contains(reasons, want)
}

// expectMetered checks a metered result and its block decision.
func expectMetered(t *testing.T, label string, got *pb.AuthorizeResponse, usage, limit string, blocked bool) {
	t.Helper()
	if got.Feature == nil || !got.Feature.Entitled || got.Feature.Kind != pb.FeatureKind_FEATURE_KIND_METERED || got.Feature.Usage != usage || got.Feature.Limit != limit {
		t.Fatalf("%s: metered result = %v", label, got.Feature)
	}
	if blocked != hasReason(got.Reasons, pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED) || got.Allowed == blocked {
		t.Fatalf("%s: allowed=%v reasons=%v, want blocked=%v", label, got.Allowed, got.Reasons, blocked)
	}
}

// subscribeAddOn confirms a paid add-on subscription on the free base plan for
// tenant, giving it a recorded Paddle period from periodStart to periodEnd.
func (f *featureFixture) subscribeAddOn(t *testing.T, tenant, price string, periodStart, periodEnd time.Time) {
	t.Helper()
	ctx := context.Background()
	change, err := f.client.ChangeSubscription(ctx, req(&pb.ChangeSubscriptionRequest{RequestKey: key(), TenantId: tenant, PlanId: f.plan, PriceIds: []string{price}, ReturnUrl: f.cfg.ReturnURLs[0]}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	payload := fmt.Sprintf(`{"id":"txn_feature_initial","status":"completed","currency_code":"USD","customer_id":"ctm_feature","subscription_id":"sub_feature","custom_data":{"delcore_job_id":%q,"delcore_tenant_id":%q},"items":[{"price":{"id":"pri_boost"},"quantity":1}],"billing_period":{"starts_at":%q,"ends_at":%q}}`,
		change.Msg.JobId, tenant, periodStart.Format(time.RFC3339Nano), periodEnd.Format(time.RFC3339Nano))
	var transaction provider.Transaction
	if err = json.Unmarshal([]byte(payload), &transaction); err != nil {
		t.Fatal(err)
	}
	if err = f.worker.ApplyTransaction(ctx, transaction, f.now()); err != nil {
		t.Fatal(err)
	}
}

func (f *featureFixture) renew(t *testing.T, periodStart, periodEnd time.Time) {
	t.Helper()
	payload := fmt.Sprintf(`{"id":"txn_feature_renewal","origin":"subscription_recurring","status":"completed","currency_code":"USD","customer_id":"ctm_feature","subscription_id":"sub_feature","items":[{"price":{"id":"pri_boost"},"quantity":1}],"billing_period":{"starts_at":%q,"ends_at":%q}}`,
		periodStart.Format(time.RFC3339Nano), periodEnd.Format(time.RFC3339Nano))
	var transaction provider.Transaction
	if err := json.Unmarshal([]byte(payload), &transaction); err != nil {
		t.Fatal(err)
	}
	if err := f.worker.ApplyTransaction(context.Background(), transaction, periodStart); err != nil {
		t.Fatal(err)
	}
}

// addOn declares the boost add-on that alone grants addon-only, with its
// recurring price.
func (f *featureFixture) addOn(t *testing.T, enabled bool) (string, string) {
	t.Helper()
	ctx := context.Background()
	addOn, err := f.client.PutAddOn(ctx, req(&pb.PutAddOnRequest{RequestKey: key(), Reason: "declare boost add-on", AddOn: &pb.AddOn{ServiceId: f.service, Key: "boost", Name: "Boost", FeatureValues: []*pb.FeatureValue{{FeatureId: f.features["addon-only"], Enabled: &enabled}}}, ReplaceFeatureValues: true}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	price, err := f.client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "declare boost fee", Price: &pb.Price{Key: "boost-fee", AddOnId: addOn.Msg.AddOn.Id, ServiceId: f.service, Meter: "base", UnitPrice: "5", Included: "0", Currency: "USD", ExternalPriceId: "pri_boost", Kind: pb.PriceKind_PRICE_KIND_RECURRING}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	return addOn.Msg.AddOn.Id, price.Msg.Price.Id
}

func TestFeatureAuthorizationKindsAndPinnedVersions(t *testing.T) {
	f := newFeatureFixture(t)
	ctx := context.Background()

	// The personal tenant selected the plan before any feature was granted.
	got := f.feature(t, f.personal, "api", nil)
	if got.Allowed || !hasReason(got.Reasons, pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED) || got.Feature == nil || got.Feature.Entitled || got.Feature.Kind != pb.FeatureKind_FEATURE_KIND_ACCESS || got.Feature.FeatureId != f.features["api"] {
		t.Fatalf("pre-feature selection granted access: %v", got)
	}

	// Access features follow the pinned grant.
	if got = f.feature(t, f.alpha, "api", nil); !got.Allowed || len(got.Reasons) != 0 || !got.Feature.Entitled {
		t.Fatalf("enabled access denied: %v", got)
	}
	for _, featureKey := range []string{"beta", "ungranted"} {
		if got = f.feature(t, f.alpha, featureKey, nil); got.Allowed || !slices.Equal(got.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED}) || got.Feature.Entitled {
			t.Fatalf("%s access granted: %v", featureKey, got)
		}
	}
	// Unknown keys and features of another service are typed blocks.
	for _, featureKey := range []string{"missing-feature", "rival-only"} {
		if got = f.feature(t, f.alpha, featureKey, nil); got.Allowed || !slices.Equal(got.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_UNKNOWN}) || got.Feature != nil {
			t.Fatalf("%s was not unknown: %v", featureKey, got)
		}
	}
	rival, err := f.authorize(authorization{token: "other", tenant: f.alpha, feature: "rival-only"})
	if err != nil || rival.Allowed || !slices.Equal(rival.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED}) {
		t.Fatalf("rival feature without a grant = %v %v", rival, err)
	}

	// Every configuration type returns its typed pinned value.
	for featureKey, want := range map[string]map[string]*pb.ConfigurationValue{
		"config-boolean": {f.alpha: {Value: &pb.ConfigurationValue_Boolean{Boolean: true}}, f.beta: {Value: &pb.ConfigurationValue_Boolean{Boolean: false}}},
		"config-integer": {f.alpha: {Value: &pb.ConfigurationValue_Integer{Integer: 30}}, f.beta: {Value: &pb.ConfigurationValue_Integer{Integer: 90}}},
		"config-decimal": {f.alpha: {Value: &pb.ConfigurationValue_Decimal{Decimal: "1.25"}}, f.beta: {Value: &pb.ConfigurationValue_Decimal{Decimal: "2"}}},
		"config-string":  {f.alpha: {Value: &pb.ConfigurationValue_Text{Text: "eu"}}, f.beta: {Value: &pb.ConfigurationValue_Text{Text: "us"}}},
		"config-list":    {f.alpha: {Value: &pb.ConfigurationValue_StringList{StringList: &pb.StringList{Values: []string{"small", "large"}}}}},
	} {
		for _, tenant := range []string{f.alpha, f.beta} {
			got = f.feature(t, tenant, featureKey, nil)
			expected := want[tenant]
			if expected == nil {
				if got.Allowed || got.Feature.Entitled || got.Feature.Configuration != nil || !hasReason(got.Reasons, pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED) {
					t.Fatalf("%s without grant = %v", featureKey, got)
				}
				continue
			}
			if !got.Allowed || !got.Feature.Entitled || got.Feature.Kind != pb.FeatureKind_FEATURE_KIND_CONFIGURATION || !proto.Equal(got.Feature.Configuration, expected) {
				t.Fatalf("%s for %s = %v, want %v", featureKey, tenant, got.Feature, expected)
			}
		}
	}

	// Metered limits come from each tenant's pinned plan version.
	expectMetered(t, "alpha limit", f.feature(t, f.alpha, "calls-limit", model("a")), "0", "3", false)
	expectMetered(t, "beta limit", f.feature(t, f.beta, "calls-limit", model("a")), "0", "5", false)
	if got = f.feature(t, f.beta, "beta", nil); !slices.Equal(got.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED}) {
		t.Fatalf("version 2 dropped beta but granted it: %v", got)
	}

	// Add-on grants are pinned like base-plan grants.
	addOnID, price := f.addOn(t, true)
	gamma := f.createTenant(t, "Gamma")
	f.subscribeAddOn(t, gamma, price, f.start.Add(-24*time.Hour), f.start.Add(24*time.Hour))
	if got = f.feature(t, gamma, "addon-only", nil); !got.Allowed || !got.Feature.Entitled {
		t.Fatalf("add-on grant denied: %v", got)
	}
	if got = f.feature(t, f.alpha, "addon-only", nil); got.Allowed {
		t.Fatalf("tenant without the add-on was granted: %v", got)
	}
	var selected string
	if err = f.db.Pool.QueryRow(ctx, "SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1 AND product_id=$2 AND ends_at IS NULL", gamma, addOnID).Scan(&selected); err != nil {
		t.Fatal(err)
	}
	f.addOn(t, false)
	if active := activeVersion(t, ctx, f.db.Pool, addOnID); active == selected {
		t.Fatal("add-on feature change did not create a version")
	}
	if got = f.feature(t, gamma, "addon-only", nil); !got.Allowed {
		t.Fatalf("existing add-on selection moved to the new version: %v", got)
	}
	expectMetered(t, "gamma keeps the base plan grant", f.feature(t, gamma, "calls-limit", model("a")), "0", "5", false)
}

func TestFeatureAuthorizationMeteredLimits(t *testing.T) {
	f := newFeatureFixture(t)
	ctx := context.Background()
	call := func(n int, name string) string {
		eventKey := fmt.Sprintf("call-%s-%d", name, n)
		f.ingest(t, "service", f.alpha, eventKey, "call", model(name))
		return eventKey
	}
	call(1, "a")
	call(2, "a")
	f.process(t)
	expectMetered(t, "below limit", f.feature(t, f.alpha, "calls-limit", model("a")), "2", "3", false)

	// Authorization never reserves capacity: concurrent checks below the limit
	// both pass and the resulting reports can overspend the limit.
	var wg sync.WaitGroup
	results := make([]*pb.AuthorizeResponse, 2)
	failures := make([]error, 2)
	for i := range results {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			results[i], failures[i] = f.authorize(authorization{tenant: f.alpha, feature: "calls-limit", groups: model("a")})
		}(i)
	}
	wg.Wait()
	for i := range results {
		if failures[i] != nil || !results[i].Allowed {
			t.Fatalf("concurrent authorization %d = %v %v", i, results[i], failures[i])
		}
	}
	third, fourth := call(3, "a"), call(4, "a")
	// Accepted but unprocessed events are not counted: the decision uses the
	// last applied aggregate while the backlog is pending.
	expectMetered(t, "backlog uses the applied value", f.feature(t, f.alpha, "calls-limit", model("a")), "2", "3", false)
	f.process(t)
	got := f.feature(t, f.alpha, "calls-limit", model("a"))
	expectMetered(t, "overspend after processing", got, "4", "3", true)
	if !slices.Equal(got.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED}) || !got.Feature.PeriodStart.AsTime().Equal(monthStart(f.start)) || !got.Feature.PeriodEnd.AsTime().Equal(monthStart(f.start).AddDate(0, 1, 0)) {
		t.Fatalf("limit result = %v", got)
	}
	// Limits apply to each group separately.
	expectMetered(t, "independent group", f.feature(t, f.alpha, "calls-limit", model("b")), "0", "3", false)
	// The tenant lifecycle state is billing-independent and unchanged.
	states, err := f.client.GetTenantStates(ctx, req(&pb.GetTenantStatesRequest{TenantIds: []string{f.alpha}}, "service"))
	if err != nil || states.Msg.States[0].State != pb.TenantState_TENANT_STATE_ACTIVE {
		t.Fatalf("feature limit changed tenant state: %v %v", states, err)
	}

	// Corrections lower the applied aggregate only after processing; reaching
	// exactly the limit still blocks, and falling below it allows again.
	f.correct(t, f.alpha, fourth, 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_CANCEL, "", nil)
	expectMetered(t, "pending cancellation", f.feature(t, f.alpha, "calls-limit", model("a")), "4", "3", true)
	f.process(t)
	expectMetered(t, "at limit", f.feature(t, f.alpha, "calls-limit", model("a")), "3", "3", true)
	f.correct(t, f.alpha, third, 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "call", model("b"))
	f.process(t)
	expectMetered(t, "reallowed after a correction", f.feature(t, f.alpha, "calls-limit", model("a")), "2", "3", false)
	expectMetered(t, "moved group", f.feature(t, f.alpha, "calls-limit", model("b")), "1", "3", false)

	// Average limits use the same six-place rounded value; a lower average
	// allows again.
	sample := func(eventKey, name, ms string) {
		f.ingest(t, "service", f.alpha, eventKey, "sample", map[string]*pb.AttributeValue{"model": attrText(name), "ms": attrNumber(ms)})
	}
	sample("slow", "a", "150")
	sample("fast", "a", "50")
	sample("r1", "r", "100")
	sample("r2", "r", "0")
	sample("r3", "r", "0")
	f.process(t)
	expectMetered(t, "average at limit", f.feature(t, f.alpha, "latency-limit", model("a")), "100", "100", true)
	expectMetered(t, "rounded average", f.feature(t, f.alpha, "latency-limit", model("r")), "33.333333", "100", false)
	f.correct(t, f.alpha, "slow", 1, pb.EventCorrectionKind_EVENT_CORRECTION_KIND_REPLACE, "sample", map[string]*pb.AttributeValue{"model": attrText("a"), "ms": attrNumber("140")})
	f.process(t)
	expectMetered(t, "average decrease", f.feature(t, f.alpha, "latency-limit", model("a")), "95", "100", false)

	// Usage resets with the next UTC calendar month of a free tenant.
	next := monthStart(time.Now())
	f.setClock(next.Add(min(time.Since(next)/2, time.Hour)))
	got = f.feature(t, f.alpha, "calls-limit", model("a"))
	expectMetered(t, "next free period", got, "0", "3", false)
	if !got.Feature.PeriodStart.AsTime().Equal(next) {
		t.Fatalf("next period starts at %v", got.Feature.PeriodStart.AsTime())
	}
}

func TestFeatureAuthorizationPaddlePeriodReset(t *testing.T) {
	f := newFeatureFixture(t)
	_, price := f.addOn(t, true)
	gamma := f.createTenant(t, "Gamma")
	periodStart, periodEnd := f.start.Add(-24*time.Hour), f.start.Add(24*time.Hour)
	f.subscribeAddOn(t, gamma, price, periodStart, periodEnd)
	for i := range 5 {
		f.ingest(t, "service", gamma, fmt.Sprintf("paid-call-%d", i), "call", model("a"))
	}
	f.process(t)
	got := f.feature(t, gamma, "calls-limit", model("a"))
	expectMetered(t, "paid period limit", got, "5", "5", true)
	if !got.Feature.PeriodStart.AsTime().Equal(periodStart) || !got.Feature.PeriodEnd.AsTime().Equal(periodEnd) {
		t.Fatalf("paid usage did not follow the Paddle period: %v", got.Feature)
	}
	renewedEnd := periodEnd.AddDate(0, 1, 0)
	f.renew(t, periodEnd, renewedEnd)
	f.setClock(periodEnd.Add(time.Hour))
	got = f.feature(t, gamma, "calls-limit", model("a"))
	expectMetered(t, "renewed period", got, "0", "5", false)
	if !got.Feature.PeriodStart.AsTime().Equal(periodEnd) || !got.Feature.PeriodEnd.AsTime().Equal(renewedEnd) {
		t.Fatalf("renewal did not reset the period: %v", got.Feature)
	}
}

// A paid plan whose confirmed period ended before a renewal was recorded has
// no current billing period. The metered feature then cannot be measured: it
// fails closed with the renewal reason instead of an internal error, and the
// paid billing result stays unchanged.
func TestFeatureAuthorizationWithoutRecordedPaidPeriod(t *testing.T) {
	f := newFeatureFixture(t)
	ctx := context.Background()
	paid, err := f.client.PutPlan(ctx, req(&pb.PutPlanRequest{RequestKey: key(), Reason: "paid plan with a metered feature", Plan: &pb.Plan{Key: "paid", Name: "paid", Rank: 10, FeatureValues: []*pb.FeatureValue{{FeatureId: f.features["calls-limit"], Limit: "5"}}}, ReplaceFeatureValues: true}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	now := f.now()
	expiredStart, expiredEnd := now.AddDate(0, -1, 0).Add(-time.Hour), now.Add(-time.Hour)
	tx, err := f.db.Pool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer tx.Rollback(ctx)
	var version string
	if err = tx.QueryRow(ctx, "SELECT active_version_id::text FROM catalog_entities WHERE id=$1", paid.Msg.Plan.Id).Scan(&version); err != nil {
		t.Fatal(err)
	}
	if _, err = tx.Exec(ctx, "UPDATE tenants SET plan_id=$2,period_start=$3,period_end=$4,external_subscription_id='sub_lapsed' WHERE id=$1", f.beta, paid.Msg.Plan.Id, expiredStart, expiredEnd); err != nil {
		t.Fatal(err)
	}
	if err = postgres.ReplaceTenantSelections(ctx, tx, f.beta, []string{version}, expiredStart); err != nil {
		t.Fatal(err)
	}
	if _, err = tx.Exec(ctx, "INSERT INTO billing_periods VALUES($1,$2,$3,false)", f.beta, expiredStart, expiredEnd); err != nil {
		t.Fatal(err)
	}
	if err = tx.Commit(ctx); err != nil {
		t.Fatal(err)
	}

	paidPlain, err := f.authorize(authorization{tenant: f.beta, paid: true})
	if err != nil || paidPlain.Allowed || !hasReason(paidPlain.Reasons, pb.BlockReason_BLOCK_REASON_RENEWAL) {
		t.Fatalf("lapsed paid period without a feature = %v %v", paidPlain, err)
	}
	unmeasured := func(label string, got *pb.AuthorizeResponse) {
		t.Helper()
		if got.Feature == nil || !got.Feature.Entitled || got.Feature.Kind != pb.FeatureKind_FEATURE_KIND_METERED || got.Feature.Limit != "5" || got.Feature.Usage != "" || got.Feature.PeriodStart != nil || got.Feature.PeriodEnd != nil {
			t.Fatalf("%s: feature result = %v", label, got.Feature)
		}
	}
	paidFeature, err := f.authorize(authorization{tenant: f.beta, paid: true, feature: "calls-limit", groups: model("a")})
	if err != nil || paidFeature.Allowed || !slices.Equal(paidFeature.Reasons, paidPlain.Reasons) {
		t.Fatalf("paid feature without a recorded period = %v %v, want reasons %v", paidFeature, err, paidPlain.Reasons)
	}
	unmeasured("paid", paidFeature)
	plain, err := f.authorize(authorization{tenant: f.beta, feature: "calls-limit", groups: model("a")})
	if err != nil || plain.Allowed || !slices.Equal(plain.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_RENEWAL}) {
		t.Fatalf("non-paid feature without a recorded period = %v %v", plain, err)
	}
	unmeasured("non-paid", plain)
	// Access features do not depend on the billing period.
	if got, err := f.authorize(authorization{tenant: f.beta, feature: "api"}); err != nil || got.Allowed || !slices.Equal(got.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_NOT_ENTITLED}) {
		t.Fatalf("ungranted access feature on the paid plan = %v %v", got, err)
	}
}

func TestFeatureAuthorizationKeepsPermissionAndBillingChecks(t *testing.T) {
	f := newFeatureFixture(t)
	ctx := context.Background()
	strip := func(r *pb.AuthorizeResponse) []byte {
		t.Helper()
		clone := proto.Clone(r).(*pb.AuthorizeResponse)
		clone.Metadata = nil
		raw, err := proto.MarshalOptions{Deterministic: true}.Marshal(clone)
		if err != nil {
			t.Fatal(err)
		}
		return raw
	}
	allowed, err := proto.MarshalOptions{Deterministic: true}.Marshal(&pb.AuthorizeResponse{Allowed: true})
	if err != nil {
		t.Fatal(err)
	}
	// Without a feature key the non-paid result is unchanged and carries no
	// feature field; consumers such as Git Tracker rely on this shape.
	plain, err := f.authorize(authorization{tenant: f.alpha})
	if err != nil || string(strip(plain)) != string(allowed) || plain.Metadata.GetCorrelationId() == "" {
		t.Fatalf("empty feature response changed: %v %v", plain, err)
	}
	paidPlain, err := f.authorize(authorization{tenant: f.alpha, paid: true})
	if err != nil || paidPlain.Feature != nil || paidPlain.Allowed {
		t.Fatalf("paid check without entitlement = %v %v", paidPlain, err)
	}
	paidFeature, err := f.authorize(authorization{tenant: f.alpha, paid: true, feature: "api"})
	if err != nil || !slices.Equal(paidFeature.Reasons, paidPlain.Reasons) || paidFeature.Allowed || !paidFeature.Feature.Entitled {
		t.Fatalf("an entitled feature changed paid reasons: %v vs %v (%v)", paidFeature, paidPlain, err)
	}

	// Permission failures win and hide feature details.
	stranger, err := f.client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "stranger"))
	if err != nil {
		t.Fatal(err)
	}
	for label, a := range map[string]authorization{
		"non-member":         {tenant: f.alpha, user: stranger.Msg.UserId, feature: "api"},
		"unknown capability": {tenant: f.alpha, capability: "administer", feature: "calls-limit", groups: model("a")},
	} {
		got, err := f.authorize(a)
		if err != nil || got.Allowed || !slices.Equal(got.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_PERMISSION}) || got.Feature != nil {
			t.Fatalf("%s = %v %v", label, got, err)
		}
	}

	// Operator blocking keeps its paid-only semantics with feature access.
	if _, err = f.client.SetTenantBlocked(ctx, req(&pb.SetTenantBlockedRequest{RequestKey: key(), TenantId: f.alpha, Blocked: true, Reason: "abuse review"}, "owner")); err != nil {
		t.Fatal(err)
	}
	if got, err := f.authorize(authorization{tenant: f.alpha, feature: "api"}); err != nil || !got.Allowed {
		t.Fatalf("feature access added billing checks to a non-paid call: %v %v", got, err)
	}
	for i := range 3 {
		f.ingest(t, "service", f.alpha, fmt.Sprintf("blocked-call-%d", i), "call", model("a"))
	}
	f.process(t)
	got, err := f.authorize(authorization{tenant: f.alpha, paid: true, feature: "calls-limit", groups: model("a")})
	if err != nil || got.Allowed || !hasReason(got.Reasons, pb.BlockReason_BLOCK_REASON_OPERATOR) || !hasReason(got.Reasons, pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED) || got.Feature.Usage != "3" {
		t.Fatalf("operator block and feature limit = %v %v", got, err)
	}
	if got, err = f.authorize(authorization{tenant: f.alpha, feature: "calls-limit", groups: model("a")}); err != nil || got.Allowed || !slices.Equal(got.Reasons, []pb.BlockReason{pb.BlockReason_BLOCK_REASON_FEATURE_LIMIT_REACHED}) {
		t.Fatalf("non-paid feature limit = %v %v", got, err)
	}

	// Service scope remains required.
	if _, err = f.authorize(authorization{token: "noscope", tenant: f.alpha, feature: "api"}); connect.CodeOf(err) != connect.CodePermissionDenied {
		t.Fatalf("service without authorize scope = %v", err)
	}
}

func TestFeatureAuthorizationRejectsInvalidGroups(t *testing.T) {
	f := newFeatureFixture(t)
	for label, a := range map[string]authorization{
		"missing group":               {tenant: f.alpha, feature: "calls-limit"},
		"extra group":                 {tenant: f.alpha, feature: "calls-limit", groups: map[string]*pb.AttributeValue{"model": attrText("a"), "region": attrText("eu")}},
		"wrong group name":            {tenant: f.alpha, feature: "calls-limit", groups: map[string]*pb.AttributeValue{"region": attrText("eu")}},
		"empty scalar":                {tenant: f.alpha, feature: "calls-limit", groups: map[string]*pb.AttributeValue{"model": {}}},
		"bad number":                  {tenant: f.alpha, feature: "calls-limit", groups: map[string]*pb.AttributeValue{"model": attrNumber("1e3")}},
		"access with groups":          {tenant: f.alpha, feature: "api", groups: model("a")},
		"groups without a key":        {tenant: f.alpha, groups: model("a")},
		"configuration groups":        {tenant: f.alpha, feature: "config-string", groups: model("a")},
		"validated before permission": {tenant: f.alpha, user: key(), feature: "calls-limit"},
	} {
		if _, err := f.authorize(a); connect.CodeOf(err) != connect.CodeInvalidArgument || typedReason(err) != pb.ErrorReason_ERROR_REASON_INVALID_ATTRIBUTE {
			t.Fatalf("%s: %v (%v)", label, err, typedReason(err))
		}
	}
	if _, err := f.authorize(authorization{tenant: f.alpha, feature: "Not A Key"}); connect.CodeOf(err) != connect.CodeInvalidArgument || typedReason(err) != pb.ErrorReason_ERROR_REASON_INVALID_INPUT {
		t.Fatalf("invalid feature key: %v", err)
	}
	// A numeric group value is a different group from the reported text value.
	f.ingest(t, "service", f.alpha, "typed-call", "call", model("7"))
	f.process(t)
	expectMetered(t, "text group", f.feature(t, f.alpha, "calls-limit", model("7")), "1", "3", false)
	expectMetered(t, "numeric group", f.feature(t, f.alpha, "calls-limit", map[string]*pb.AttributeValue{"model": attrNumber("7")}), "0", "3", false)
}
