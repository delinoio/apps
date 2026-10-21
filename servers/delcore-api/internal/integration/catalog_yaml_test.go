// SPDX-License-Identifier: FSL-1.1-ALv2

//go:build integration

package integration

import (
	"context"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"

	pb "github.com/delinoio/apps/protos/delcore/v1"
	"github.com/delinoio/apps/servers/delcore-api/internal/catalog"
	"github.com/delinoio/apps/servers/delcore-api/internal/postgres"
)

func catalogFixture(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	files, err := os.ReadDir("../../catalog")
	if err != nil {
		t.Fatal(err)
	}
	for _, file := range files {
		body, err := os.ReadFile(filepath.Join("../../catalog", file.Name()))
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(filepath.Join(root, file.Name()), body, 0600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}
func catalogText(t *testing.T, root, name string) string {
	t.Helper()
	body, err := os.ReadFile(filepath.Join(root, name))
	if err != nil {
		t.Fatal(err)
	}
	return string(body)
}
func setCatalogText(t *testing.T, root, name, body string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(root, name), []byte(body), 0600); err != nil {
		t.Fatal(err)
	}
}
func catalogIdentity(t *testing.T, db *postgres.Store, kind, scope, key string) (string, string) {
	t.Helper()
	var id, version string
	err := db.Pool.QueryRow(context.Background(), "SELECT id::text,COALESCE(active_version_id::text,'') FROM catalog_entities WHERE kind=$1 AND scope=$2 AND key=$3", kind, scope, key).Scan(&id, &version)
	if err != nil {
		t.Fatal(err)
	}
	return id, version
}

func TestDevelopmentCatalogSyncAtomicVersionedAndAuthoritative(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	sync := func() error { return catalog.Sync(ctx, db, root, logger) }
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	serviceID, _ := catalogIdentity(t, db, "service", "", "example-api")
	planID, initial := catalogIdentity(t, db, "plan", "", "example-free")
	priceID, priceVersion := catalogIdentity(t, db, "price", "plan:"+planID, "request-usd")
	if priceVersion == "" || initial == "" {
		t.Fatal("catalog versions missing")
	}
	tenant, err := client.CreateTenant(ctx, req(&pb.CreateTenantRequest{RequestKey: key(), Name: "Pinned Catalog"}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	tenantID := tenant.Msg.Tenant.Id
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	if id, version := catalogIdentity(t, db, "plan", "", "example-free"); id != planID || version != initial {
		t.Fatal("replay changed plan")
	}
	if id, version := catalogIdentity(t, db, "price", "plan:"+planID, "request-usd"); id != priceID || version != priceVersion {
		t.Fatal("replay changed price")
	}
	if err := os.Mkdir(filepath.Join(root, "nested"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(filepath.Join(root, "06-price.yaml"), filepath.Join(root, "nested", "price.yml")); err != nil {
		t.Fatal(err)
	}
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	if _, version := catalogIdentity(t, db, "plan", "", "example-free"); version != initial {
		t.Fatal("file relocation changed version")
	}
	// Operator changes are allowed, but the next development startup restores
	// each declared field and active version from files.
	_, err = client.PutService(ctx, req(&pb.PutServiceRequest{RequestKey: key(), Reason: "test operator drift", Service: &pb.Service{Key: "example-api", Name: "API drift", ClientId: "example-api-local", Capabilities: []string{"execute"}, Scopes: []string{"authorize"}}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.CreatePrice(ctx, req(&pb.CreatePriceRequest{RequestKey: key(), Reason: "test operator price drift", Price: &pb.Price{Key: "request-usd", PlanId: planID, ServiceId: serviceID, Meter: "requests", UnitPrice: "0.02", Included: "100", Currency: "USD"}}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	if _, version := catalogIdentity(t, db, "plan", "", "example-free"); version == initial {
		t.Fatal("operator price edit did not change version")
	}
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	var name string
	if err = db.Pool.QueryRow(ctx, "SELECT name FROM services WHERE id=$1", serviceID).Scan(&name); err != nil || name != "Example API" {
		t.Fatal("service drift not restored", err)
	}
	if _, version := catalogIdentity(t, db, "plan", "", "example-free"); version != initial {
		t.Fatal("API price drift not restored to existing version")
	}
	price := catalogText(t, filepath.Join(root, "nested"), "price.yml")
	setCatalogText(t, filepath.Join(root, "nested"), "price.yml", strings.Replace(price, "unit_price: \"0.01\"", "unit_price: \"0.03\"", 1))
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	_, changed := catalogIdentity(t, db, "plan", "", "example-free")
	if changed == initial {
		t.Fatal("billing change did not version")
	}
	var selected string
	if err := db.Pool.QueryRow(ctx, "SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1 AND ends_at IS NULL AND product_id=$2", tenantID, planID).Scan(&selected); err != nil || selected != initial {
		t.Fatal("YAML edit moved an existing subscription", err)
	}
	setCatalogText(t, filepath.Join(root, "nested"), "price.yml", price)
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	if _, version := catalogIdentity(t, db, "plan", "", "example-free"); version != initial {
		t.Fatal("A to B to A failed to reuse A")
	}
	plan := catalogText(t, root, "05-plan.yaml")
	setCatalogText(t, root, "05-plan.yaml", strings.Replace(plan, "name: Example Free", "name: Renamed Free", 1))
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	if _, version := catalogIdentity(t, db, "plan", "", "example-free"); version != initial {
		t.Fatal("display name changed billing version")
	}
	addonID, _ := catalogIdentity(t, db, "addon", serviceID, "priority")
	if err := os.Remove(filepath.Join(root, "07-addon.yaml")); err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(filepath.Join(root, "08-addon-price.yaml")); err != nil {
		t.Fatal(err)
	}
	if err := sync(); err != nil {
		t.Fatal(err)
	}
	if id, _ := catalogIdentity(t, db, "addon", serviceID, "priority"); id != addonID {
		t.Fatal("omitted entity was deleted")
	}
	// A later bad reference must roll back changes made earlier in the same
	// transaction, including a service display edit.
	service := catalogText(t, root, "01-service.yaml")
	setCatalogText(t, root, "01-service.yaml", strings.Replace(service, "name: Example API", "name: Rolled Back", 1))
	feature := catalogText(t, root, "04-limit.yaml")
	setCatalogText(t, root, "04-limit.yaml", strings.Replace(feature, "meter: requests", "meter: missing", 1))
	if err := sync(); err == nil {
		t.Fatal("unknown reference accepted")
	}
	if err := db.Pool.QueryRow(ctx, "SELECT name FROM services WHERE id=$1", serviceID).Scan(&name); err != nil || name != "Example API" {
		t.Fatal("invalid reference left partial service update", err)
	}
	setCatalogText(t, root, "04-limit.yaml", feature)
	setCatalogText(t, root, "01-service.yaml", service)
	meter := catalogText(t, root, "02-meter.yml")
	setCatalogText(t, root, "02-meter.yml", strings.Replace(meter, "aggregation: count", "aggregation: sum\nvalue_attribute: amount", 1))
	if err := sync(); err == nil {
		t.Fatal("immutable meter definition changed")
	}
	setCatalogText(t, root, "02-meter.yml", meter)
	if err := sync(); err != nil {
		t.Fatal("recovery after invalid file", err)
	}
}

func TestDevelopmentCatalogIgnoresTraversalOrderAndSwapsPlanRanks(t *testing.T) {
	db, _, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	if err := os.Rename(filepath.Join(root, "01-service.yaml"), filepath.Join(root, "zz-service.yaml")); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(filepath.Join(root, "05-plan.yaml"), filepath.Join(root, "zz-plan.yaml")); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(filepath.Join(root, "06-price.yaml"), filepath.Join(root, "aa-price.yaml")); err != nil {
		t.Fatal(err)
	}
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal("cross-file references depend on traversal order", err)
	}
	other := "kind: plan\nkey: other-free\nname: Other Free\nrank: 1\nfree: true\ndefault_free: false\n"
	setCatalogText(t, root, "other.yaml", other)
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	original := catalogText(t, root, "zz-plan.yaml")
	setCatalogText(t, root, "zz-plan.yaml", strings.Replace(original, "rank: 0", "rank: 1", 1))
	setCatalogText(t, root, "other.yaml", strings.Replace(other, "rank: 1", "rank: 0", 1))
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal("rank swap failed despite a valid complete set", err)
	}
	for key, want := range map[string]int{"example-free": 1, "other-free": 0} {
		id, _ := catalogIdentity(t, db, "plan", "", key)
		var rank int
		if err := db.Pool.QueryRow(ctx, "SELECT rank FROM plans WHERE id=$1", id).Scan(&rank); err != nil || rank != want {
			t.Fatalf("%s rank %d, want %d: %v", key, rank, want, err)
		}
	}
}

func TestDevelopmentCatalogPriceOnlyRebuildsOmittedOwner(t *testing.T) {
	db, _, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	planID, original := catalogIdentity(t, db, "plan", "", "example-free")
	priceOnly := t.TempDir()
	body := catalogText(t, root, "06-price.yaml")
	setCatalogText(t, priceOnly, "only-price.yml", strings.Replace(body, "unit_price: \"0.01\"", "unit_price: \"0.04\"", 1))
	if err := catalog.Sync(ctx, db, priceOnly, logger); err != nil {
		t.Fatal(err)
	}
	if _, version := catalogIdentity(t, db, "plan", "", "example-free"); version == original {
		t.Fatal("price-only file did not rebuild omitted owner")
	}
	var rank int
	if err := db.Pool.QueryRow(ctx, "SELECT rank FROM plans WHERE id=$1", planID).Scan(&rank); err != nil || rank != 0 {
		t.Fatal("omitted plan data changed", err)
	}
}

func TestDevelopmentCatalogCannotMutateOmittedDefaultPlan(t *testing.T) {
	db, _, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	originalID, originalVersion := catalogIdentity(t, db, "plan", "", "example-free")
	other := t.TempDir()
	setCatalogText(t, other, "replacement.yaml", "kind: plan\nkey: replacement\nname: Replacement\nrank: 1\nfree: true\ndefault_free: true\n")
	if err := catalog.Sync(ctx, db, other, logger); err == nil {
		t.Fatal("omitted default plan was silently changed")
	}
	var isDefault bool
	if err := db.Pool.QueryRow(ctx, "SELECT is_default FROM plans WHERE id=$1", originalID).Scan(&isDefault); err != nil || !isDefault {
		t.Fatal("omitted default plan changed", err)
	}
	if _, version := catalogIdentity(t, db, "plan", "", "example-free"); version != originalVersion {
		t.Fatal("omitted plan version changed")
	}
	var count int
	if err := db.Pool.QueryRow(ctx, "SELECT count(*) FROM plans WHERE name='Replacement'").Scan(&count); err != nil || count != 0 {
		t.Fatal("rejected catalog partially applied", err)
	}
}

func TestDevelopmentCatalogSnapshotsLegacyTenantsBeforePlanEdit(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	legacy := "kind: plan\nkey: legacy-plan\nname: Legacy Plan\nrank: 7\nfree: true\n"
	setCatalogText(t, root, "legacy.yaml", legacy)
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	me, err := client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil || len(me.Msg.Tenants) == 0 {
		t.Fatal("personal tenant was not created", err)
	}
	tenantID := me.Msg.Tenants[0].Id
	planID, original := catalogIdentity(t, db, "plan", "", "legacy-plan")
	// A tenant created before catalog versioning has a plan but no selection row.
	if _, err = db.Pool.Exec(ctx, "DELETE FROM tenant_catalog_selections WHERE tenant_id=$1", tenantID); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, "UPDATE tenants SET plan_id=$2 WHERE id=$1", tenantID, planID); err != nil {
		t.Fatal(err)
	}
	setCatalogText(t, root, "legacy.yaml", "kind: plan\nkey: legacy-plan\nname: Legacy Plan\nrank: 7\nfree: false\npostpaid: true\n")
	if err = catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	_, active := catalogIdentity(t, db, "plan", "", "legacy-plan")
	if active == original {
		t.Fatal("plan edit did not create a new active version")
	}
	var selected, selectedFree string
	if err = db.Pool.QueryRow(ctx, `SELECT cs.version_id::text,cv.definition->>'free'
FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id
WHERE cs.tenant_id=$1 AND cs.product_id=$2 AND cs.ends_at IS NULL`, tenantID, planID).Scan(&selected, &selectedFree); err != nil {
		t.Fatal("legacy tenant was not snapshotted before the plan changed: ", err)
	}
	if selected == active || selectedFree != "true" {
		t.Fatalf("legacy tenant did not keep its previous plan policy: selected=%s free=%s active=%s", selected, selectedFree, active)
	}
}

// A feature-only edit leaves the plan and add-on rows unchanged but replaces
// their grants, so tenants without selections must be pinned first.
func TestDevelopmentCatalogSnapshotsLegacyTenantsBeforeFeatureOnlyEdit(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	setCatalogText(t, root, "legacy.yaml", "kind: plan\nkey: legacy-plan\nname: Legacy Plan\nrank: 7\nfree: true\nfeatures:\n  - service: example-api\n    key: api-access\n    enabled: true\n")
	setCatalogText(t, root, "07-addon.yaml", catalogText(t, root, "07-addon.yaml")+"features:\n  - service: example-api\n    key: monthly-requests\n    limit: \"10\"\n")
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	me, err := client.GetCurrentUser(ctx, req(&pb.GetCurrentUserRequest{}, "owner"))
	if err != nil || len(me.Msg.Tenants) == 0 {
		t.Fatal("personal tenant was not created", err)
	}
	tenantID := me.Msg.Tenants[0].Id
	serviceID, _ := catalogIdentity(t, db, "service", "", "example-api")
	planID, originalPlan := catalogIdentity(t, db, "plan", "", "legacy-plan")
	addonID, originalAddOn := catalogIdentity(t, db, "addon", serviceID, "priority")
	_, addonPrice := catalogIdentity(t, db, "price", "addon:"+addonID, "priority-monthly")
	// A tenant created before catalog versioning has a plan and an add-on
	// entitlement but no selection rows.
	if _, err = db.Pool.Exec(ctx, "DELETE FROM tenant_catalog_selections WHERE tenant_id=$1", tenantID); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, "UPDATE tenants SET plan_id=$2 WHERE id=$1", tenantID, planID); err != nil {
		t.Fatal(err)
	}
	if _, err = db.Pool.Exec(ctx, "INSERT INTO entitlements(id,tenant_id,price_id,starts_at) VALUES($1,$2,$3,now()-interval '1 day')", key(), tenantID, addonPrice); err != nil {
		t.Fatal(err)
	}
	setCatalogText(t, root, "legacy.yaml", "kind: plan\nkey: legacy-plan\nname: Legacy Plan\nrank: 7\nfree: true\nfeatures:\n  - service: example-api\n    key: api-access\n    enabled: false\n")
	setCatalogText(t, root, "07-addon.yaml", strings.Replace(catalogText(t, root, "07-addon.yaml"), "limit: \"10\"", "limit: \"20\"", 1))
	if err = catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		name, productID, original, want string
	}{
		{"plan", planID, originalPlan, `{"enabled": true}`},
		{"add-on", addonID, originalAddOn, `{"limit": "10"}`},
	} {
		var active string
		if err = db.Pool.QueryRow(ctx, "SELECT active_version_id::text FROM catalog_entities WHERE id=$1", tc.productID).Scan(&active); err != nil {
			t.Fatal(err)
		}
		if active == tc.original {
			t.Fatalf("%s feature edit did not create a new active version", tc.name)
		}
		var selected, value string
		if err = db.Pool.QueryRow(ctx, `SELECT cs.version_id::text,vf.value::text
FROM tenant_catalog_selections cs JOIN catalog_version_features vf ON vf.version_id=cs.version_id
WHERE cs.tenant_id=$1 AND cs.product_id=$2 AND cs.ends_at IS NULL`, tenantID, tc.productID).Scan(&selected, &value); err != nil {
			t.Fatalf("legacy tenant was not snapshotted before the %s grants changed: %v", tc.name, err)
		}
		if selected == active || value != tc.want {
			t.Fatalf("legacy tenant did not keep its previous %s grants: selected=%s value=%s active=%s", tc.name, selected, value, active)
		}
	}
}

func TestDevelopmentCatalogStoresExistsFilterInCanonicalForm(t *testing.T) {
	db, client, _ := setup(t)
	ctx := context.Background()
	root := catalogFixture(t)
	logger := slog.New(slog.NewTextHandler(io.Discard, nil))
	setCatalogText(t, root, "02-meter.yml", strings.Replace(catalogText(t, root, "02-meter.yml"), "filters:\n", "filters:\n  - attribute: trace\n    operator: exists\n", 1))
	if err := catalog.Sync(ctx, db, root, logger); err != nil {
		t.Fatal(err)
	}
	got, err := client.GetCatalog(ctx, req(&pb.GetCatalogRequest{}, "owner"))
	if err != nil {
		t.Fatal(err)
	}
	var meter *pb.Meter
	for _, m := range got.Msg.Meters {
		if m.Key == "requests" {
			meter = m
		}
	}
	if meter == nil {
		t.Fatal("synchronized meter is missing from the catalog")
	}
	var found bool
	for _, f := range meter.Filters {
		if f.Operator == pb.MeterFilterOperator_METER_FILTER_OPERATOR_EXISTS {
			found = true
			if len(f.Values) != 1 || f.Values[0] != "true" {
				t.Fatalf("existence filter values = %v, want [true]", f.Values)
			}
		}
	}
	if !found {
		t.Fatal("existence filter was not stored")
	}
	if _, err = client.PutMeter(ctx, req(&pb.PutMeterRequest{RequestKey: key(), Reason: "resubmit synchronized meter", Meter: meter}, "owner")); err != nil {
		t.Fatal("synchronized meter cannot be submitted back through PutMeter: ", err)
	}
}
