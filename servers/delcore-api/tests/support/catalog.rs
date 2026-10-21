// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Fixture, key};
use serde_json::json;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn legacy_plan_mutation_preserves_policy_and_backdates_selection() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = me["tenants"][0]["id"].as_str().unwrap();
    let plan = key();
    sqlx::query("INSERT INTO plans(id,name,rank,free,is_default,postpaid) VALUES($1::text::uuid,'Legacy Plan',100,true,false,false)")
        .bind(&plan).execute(&f.db).await.unwrap();
    sqlx::query("UPDATE tenants SET plan_id=$2::text::uuid WHERE id=$1::text::uuid")
        .bind(tenant)
        .bind(&plan)
        .execute(&f.db)
        .await
        .unwrap();
    let updated = f.rpc("owner", "PutPlan", json!({"requestKey":key(),"reason":"mutate legacy plan","plan":{"id":plan,"name":"Updated Legacy Plan","rank":101,"free":false}})).await;
    let (selected, free, backdated): (String, String, bool) = sqlx::query_as(
        "SELECT cs.version_id::text,cv.definition->>'free',cs.starts_at=t.period_start FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id JOIN tenants t ON t.id=cs.tenant_id WHERE cs.tenant_id=$1::text::uuid AND cs.product_id=$2::text::uuid AND cs.ends_at IS NULL")
        .bind(tenant).bind(&plan).fetch_one(&f.db).await.unwrap();
    assert_ne!(selected, updated["plan"]["activeVersionId"]);
    assert_eq!(free, "true");
    assert!(
        backdated,
        "late usage must retain the previous policy for the entire period"
    );
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn legacy_addon_mutation_backdates_selection_and_preserves_old_prices() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let service = f.service().await;
    let feature = f.rpc("owner", "PutFeature", json!({"requestKey":key(),"reason":"declare access","feature":{"serviceId":service,"key":"priority","name":"Priority","kind":"FEATURE_KIND_ACCESS"}})).await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await;
    let tenant = me["tenants"][0]["id"].as_str().unwrap();
    let addon = key();
    let price = key();
    sqlx::query("INSERT INTO addons(id,service_id,name) VALUES($1::text::uuid,$2::text::uuid,'Legacy Add-on')")
        .bind(&addon).bind(&service).execute(&f.db).await.unwrap();
    sqlx::query("INSERT INTO catalog_entities(id,kind,scope,key) VALUES($1::text::uuid,'addon',$2,'legacy-addon')")
        .bind(&addon).bind(&service).execute(&f.db).await.unwrap();
    sqlx::query("INSERT INTO prices(id,addon_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,kind) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,'base',5,0,'USD','pri_legacy',now(),2)")
        .bind(&price).bind(&addon).bind(&service).execute(&f.db).await.unwrap();
    sqlx::query("INSERT INTO entitlements(id,tenant_id,price_id,starts_at,ends_at) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,now()-interval '1 day',now()+interval '1 day')")
        .bind(key()).bind(tenant).bind(&price).execute(&f.db).await.unwrap();
    let updated = f.rpc("owner", "PutAddOn", json!({"requestKey":key(),"reason":"mutate legacy addon","replaceFeatureValues":true,"addOn":{"id":addon,"serviceId":service,"name":"Legacy Add-on","featureValues":[{"featureId":feature["feature"]["id"],"enabled":true}]}})).await;
    let (version, features, prices, covered): (String, i32, i32, bool) = sqlx::query_as(
        "SELECT cs.version_id::text,jsonb_array_length(cv.definition->'features'),jsonb_array_length(cv.definition->'prices'),cs.starts_at=e.starts_at AND cs.ends_at=e.ends_at FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id JOIN entitlements e ON e.tenant_id=cs.tenant_id AND e.price_id=$3::text::uuid WHERE cs.tenant_id=$1::text::uuid AND cs.product_id=$2::text::uuid")
        .bind(tenant).bind(&addon).bind(&price).fetch_one(&f.db).await.unwrap();
    assert_ne!(version, updated["addOn"]["activeVersionId"]);
    assert_eq!((features, prices), (0, 1));
    assert!(
        covered,
        "a future-ending entitlement remains held and retains its full historical interval"
    );
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn price_versions_reuse_content_without_moving_existing_subscriptions() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let service = f.service().await;
    let catalog = f.rpc("owner", "GetCatalog", json!({})).await;
    let plan = catalog["plans"][0]["id"].as_str().unwrap();
    f.rpc("owner", "PutMeter", json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":service,"name":"requests","key":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"}})).await;
    let mut versions = Vec::new();
    let mut prices = Vec::new();
    let mut tenants = Vec::new();
    for amount in ["1.00", "2", "1"] {
        let price = f.rpc("owner", "CreatePrice", json!({"requestKey":key(),"reason":"select price version","price":{"key":"requests-usd","planId":plan,"serviceId":service,"meter":"requests","unitPrice":amount,"included":"2","currency":"USD"}})).await;
        prices.push(price["price"]["id"].as_str().unwrap().to_owned());
        let version: String = sqlx::query_scalar(
            "SELECT active_version_id::text FROM catalog_entities WHERE id=$1::text::uuid",
        )
        .bind(plan)
        .fetch_one(&f.db)
        .await
        .unwrap();
        versions.push(version);
        let tenant = f
            .rpc(
                "owner",
                "CreateTenant",
                json!({"requestKey":key(),"name":"Version snapshot"}),
            )
            .await;
        tenants.push(tenant["tenant"]["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(versions[0], versions[2]);
    assert_eq!(prices[0], prices[2]);
    assert_ne!(versions[0], versions[1]);
    assert_ne!(prices[0], prices[1]);
    for i in 0..tenants.len() {
        let selected: String = sqlx::query_scalar("SELECT version_id::text FROM tenant_catalog_selections WHERE tenant_id=$1::text::uuid AND ends_at IS NULL")
            .bind(&tenants[i]).fetch_one(&f.db).await.unwrap();
        let price: String = sqlx::query_scalar("SELECT price_id::text FROM entitlements WHERE tenant_id=$1::text::uuid AND ends_at IS NULL")
            .bind(&tenants[i]).fetch_one(&f.db).await.unwrap();
        assert_eq!(selected, versions[i]);
        assert_eq!(price, prices[i]);
    }
}

struct CatalogDirectory(std::path::PathBuf);
impl CatalogDirectory {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("delcore-catalog-{}", key()));
        std::fs::create_dir(&directory).unwrap();
        Self(directory)
    }
    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap();
    }
}
impl Drop for CatalogDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn yaml_reconciliation_rolls_back_earlier_writes_when_reference_is_invalid() {
    let f = Fixture::setup().await;
    f.bootstrap().await;
    let directory = CatalogDirectory::new();
    let service = "kind: service\nkey: catalog-api\nname: Catalog API\nclient_id: catalog-api\ncapabilities: [execute]\nscopes: [authorize]\n";
    directory.write("service.yaml", service);
    let first = delcore_api::catalog::sync(
        &f.db,
        &directory.0,
        &delcore_api::telemetry::Metrics::default(),
    )
    .await
    .unwrap();
    assert!(first.changed);
    let unchanged = delcore_api::catalog::sync(
        &f.db,
        &directory.0,
        &delcore_api::telemetry::Metrics::default(),
    )
    .await
    .unwrap();
    assert!(!unchanged.changed);
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit WHERE action='catalog.reconcile'")
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert_eq!(audits, 1);
    directory.write(
        "service.yaml",
        &service.replace("name: Catalog API", "name: Rolled Back"),
    );
    directory.write("feature.yaml", "kind: feature\nservice: catalog-api\nkey: invalid-limit\nname: Invalid Limit\ntype: metered\nmeter: missing\n");
    assert!(
        delcore_api::catalog::sync(
            &f.db,
            &directory.0,
            &delcore_api::telemetry::Metrics::default()
        )
        .await
        .is_err()
    );
    let name: String =
        sqlx::query_scalar("SELECT name FROM services WHERE client_id='catalog-api'")
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert_eq!(name, "Catalog API");
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit WHERE action='catalog.reconcile'")
            .fetch_one(&f.db)
            .await
            .unwrap();
    assert_eq!(
        audits, 1,
        "rejected and unchanged applications must not produce audits"
    );
    let features: i64 = sqlx::query_scalar("SELECT count(*) FROM features")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(features, 0);
    std::fs::remove_file(directory.0.join("feature.yaml")).unwrap();
    directory.write("service.yaml", service);
    assert!(
        !delcore_api::catalog::sync(
            &f.db,
            &directory.0,
            &delcore_api::telemetry::Metrics::default()
        )
        .await
        .unwrap()
        .changed
    );
}
