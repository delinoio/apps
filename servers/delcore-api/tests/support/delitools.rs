// SPDX-License-Identifier: FSL-1.1-ALv2
//! DeliTools consumer integration: call-time AI pricing, markup settlement,
//! tenant-key authorization, the reserved identity and retention independence.
use super::{Fixture, key};
use chrono::{Duration, Utc};
use delcore_api::accounting::Decimal;
use serde_json::{Value, json};

fn d(s: &str) -> Decimal {
    Decimal::parse(s).unwrap()
}
const TRANSLATOR: &str = "translator.usage";
const EXTRACTOR: &str = "extractor.usage";

struct Setup {
    service: String,
}
async fn register(f: &Fixture) -> Setup {
    f.bootstrap().await;
    let service = f.rpc("owner","PutService",json!({"requestKey":key(),"reason":"register delitools","service":{"key":"delitools","name":"delitools","clientId":"service","capabilities":["translator","extractor"],"scopes":["authorize","usage:write","usage:read"]}})).await["service"]["id"].as_str().unwrap().to_owned();
    for (name, event, markup) in [
        ("translator-usd", TRANSLATOR, "1.15"),
        ("extractor-usd", EXTRACTOR, "1.30"),
    ] {
        f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare ai meter","meter":{"serviceId":service,"key":name,"name":name,"eventType":event,"aggregation":"METER_AGGREGATION_SUM","valueSource":"METER_VALUE_SOURCE_AI_TOKEN_USD","markup":markup}})).await;
    }
    Setup { service }
}
fn attrs(tokens: [&str; 4], prices: [&str; 4], effective: &str) -> Value {
    let mut a = json!({"model":{"text":"vendor/model"},"price_effective_at":{"text":effective}});
    for (i, name) in ["input", "output", "cache_read", "cache_write"]
        .iter()
        .enumerate()
    {
        if !tokens[i].is_empty() {
            a[format!("{name}_tokens")] = json!({"number":tokens[i]});
        }
        if !prices[i].is_empty() {
            a[format!("{name}_usd_per_token")] = json!({"number":prices[i]});
        }
    }
    a
}
fn full() -> Value {
    attrs(
        ["1000", "500", "200", "100"],
        ["0.000002", "0.00001", "0.0000005", "0.0000025"],
        &(Utc::now() - Duration::hours(2)).to_rfc3339(),
    )
}
fn event(tenant: &str, service: &str, id: &str, kind: &str, attributes: Value) -> Value {
    json!({"requestKey":key(),"event":{"tenantId":tenant,"serviceId":service,"eventKey":id,"eventType":kind,"occurredAt":Utc::now().to_rfc3339(),"attributes":attributes}})
}
async fn tenant_of(f: &Fixture, actor: &str) -> String {
    f.rpc(actor, "GetCurrentUser", json!({})).await["tenants"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn credit(f: &Fixture, tenant: &str, amount: &str) {
    sqlx::query("INSERT INTO ledger(id,tenant_id,kind,amount,source_key,created_at) VALUES($1,$2::text::uuid,'credit',$3::text::numeric,$4,now())")
        .bind(uuid::Uuid::now_v7()).bind(tenant).bind(amount).bind(key()).execute(&f.db).await.unwrap();
}
async fn sum(f: &Fixture, tenant: &str, meter: &str, kinds: &[&str]) -> Decimal {
    let kinds: Vec<String> = kinds.iter().map(|k| (*k).into()).collect();
    let text: String = sqlx::query_scalar("SELECT COALESCE(sum(amount),0)::text FROM ledger WHERE tenant_id=$1::text::uuid AND meter=$2 AND kind=ANY($3)")
        .bind(tenant).bind(meter).bind(kinds).fetch_one(&f.db).await.unwrap();
    d(&text)
}
async fn balance(f: &Fixture, tenant: &str) -> Decimal {
    let text: String = sqlx::query_scalar("SELECT COALESCE(sum(amount),0)::text FROM ledger WHERE tenant_id=$1::text::uuid AND kind IN ('credit','usage','refund','correction')")
        .bind(tenant).fetch_one(&f.db).await.unwrap();
    d(&text)
}
async fn authorize(f: &Fixture, body: Value) -> Value {
    let mut response = f.rpc("service", "Authorize", body).await;
    // Protobuf JSON omits a false boolean.
    response["allowed"] = json!(response["allowed"].as_bool().unwrap_or(false));
    response
}
fn reasons(v: &Value) -> Vec<String> {
    v["reasons"]
        .as_array()
        .map(|r| r.iter().map(|x| x.to_string()).collect())
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn free_and_postpaid_tenants_are_rated_at_report_prices_with_both_markups() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let free = tenant_of(&f, "owner").await;
    credit(&f, &free, "1").await;
    // A subscribed (postpaid) tenant whose pinned plan version has no price at all.
    let pro = f.rpc("owner","PutPlan",json!({"requestKey":key(),"reason":"paid plan","plan":{"key":"pro","name":"pro","rank":5,"free":false,"postpaid":true}})).await;
    let subscribed = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Subscribed"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query("UPDATE tenants SET plan_id=$2::text::uuid,external_subscription_id='sub_test',period_start=date_trunc('month',now()),period_end=date_trunc('month',now())+interval '1 month' WHERE id=$1::text::uuid")
        .bind(&subscribed).bind(pro["plan"]["id"].as_str().unwrap()).execute(&f.db).await.unwrap();
    sqlx::query("INSERT INTO billing_periods VALUES($1::text::uuid,date_trunc('month',now()),date_trunc('month',now())+interval '1 month',true)").bind(&subscribed).execute(&f.db).await.unwrap();
    sqlx::query("DELETE FROM tenant_catalog_selections WHERE tenant_id=$1::text::uuid")
        .bind(&subscribed)
        .execute(&f.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tenant_catalog_selections(id,tenant_id,product_id,version_id,starts_at) VALUES($1,$2::text::uuid,$3::text::uuid,$4::text::uuid,now()-interval '40 days')")
        .bind(uuid::Uuid::now_v7()).bind(&subscribed).bind(pro["plan"]["id"].as_str().unwrap()).bind(pro["plan"]["activeVersionId"].as_str().unwrap()).execute(&f.db).await.unwrap();
    let prices: i32 = sqlx::query_scalar("SELECT jsonb_array_length(definition->'prices') FROM catalog_versions WHERE id=$1::text::uuid").bind(pro["plan"]["activeVersionId"].as_str().unwrap()).fetch_one(&f.db).await.unwrap();
    assert_eq!(prices, 0, "the pinned version knows no prices");

    for (tenant, id) in [(&free, "free-t"), (&subscribed, "sub-t")] {
        f.rpc(
            "service",
            "IngestEvent",
            event(tenant, &s.service, id, TRANSLATOR, full()),
        )
        .await;
    }
    f.rpc(
        "service",
        "IngestEvent",
        event(&free, &s.service, "free-x", EXTRACTOR, full()),
    )
    .await;
    f.process_metering().await;
    // 1000*0.000002 + 500*0.00001 + 200*0.0000005 + 100*0.0000025 = 0.00735
    assert_eq!(
        sum(&f, &free, "translator-usd", &["usage"]).await,
        d("-0.0084525")
    );
    assert_eq!(
        sum(&f, &free, "extractor-usd", &["usage"]).await,
        d("-0.009555")
    );
    assert_eq!(balance(&f, &free).await, d("0.9819925"));
    // Postpaid with no credits: the whole charge is a postpaid entry.
    assert_eq!(
        sum(&f, &subscribed, "translator-usd", &["postpaid"]).await,
        d("-0.0084525")
    );
    assert_eq!(
        sum(&f, &subscribed, "translator-usd", &["usage"]).await,
        d("0")
    );
    // No plan price was attributed to the call-time events.
    let attributed: i64 = sqlx::query_scalar("SELECT count(*) FROM metering_event_prices")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(attributed, 0);
    // The aggregate is the exact USD amount.
    let aggregates = f
        .rpc(
            "service",
            "ListMeterAggregates",
            json!({"tenantId":free,"meterKey":"translator-usd"}),
        )
        .await;
    assert_eq!(aggregates["aggregates"][0]["value"], "0.0084525");
    // A usage price can never be attached to a call-time meter.
    let pro_id = pro["plan"]["id"].as_str().unwrap();
    f.error("owner","CreatePrice",json!({"requestKey":key(),"reason":"plan price","price":{"planId":pro_id,"serviceId":s.service,"meter":"translator-usd","unitPrice":"1","included":"0","currency":"USD"}}),"invalid_argument").await;
    // USD only: other currencies are rejected for delitools prices too.
    f.error("owner","CreatePrice",json!({"requestKey":key(),"reason":"plan price","price":{"planId":pro_id,"serviceId":s.service,"meter":"translator-usd","unitPrice":"1","included":"0","currency":"EUR"}}),"invalid_argument").await;
    // Period rounding: the closed period total rounds once to cents.
    f.worker
        .maintain(Utc::now() + Duration::days(40))
        .await
        .unwrap();
    let total = sum(&f, &free, "translator-usd", &["usage", "correction"]).await;
    assert_eq!(total, d("-0.01"));
    let total = sum(&f, &free, "extractor-usd", &["usage", "correction"]).await;
    assert_eq!(total, d("-0.01"));
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn later_prices_corrections_and_replays_reconcile_the_ledger() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let tenant = tenant_of(&f, "owner").await;
    credit(&f, &tenant, "100").await;
    let at = Utc::now().to_rfc3339();
    let old = attrs(
        ["1000", "", "", ""],
        ["0.000001", "", "", ""],
        &(Utc::now() - Duration::days(2)).to_rfc3339(),
    );
    let new = attrs(
        ["1000", "", "", ""],
        ["0.000003", "", "", ""],
        &(Utc::now() - Duration::hours(1)).to_rfc3339(),
    );
    let ingest = |id: &str, attributes: &Value| json!({"requestKey":key(),"event":{"tenantId":tenant,"serviceId":s.service,"eventKey":id,"eventType":TRANSLATOR,"occurredAt":at,"attributes":attributes}});
    let first = f.rpc("service", "IngestEvent", ingest("a", &old)).await;
    let again = f.rpc("service", "IngestEvent", ingest("a", &old)).await;
    assert_eq!(first["eventId"], again["eventId"]);
    f.error(
        "service",
        "IngestEvent",
        ingest("a", &new),
        "already_exists",
    )
    .await;
    f.rpc("service", "IngestEvent", ingest("b", &new)).await;
    f.process_metering().await;
    // 1000 * 0.000001 * 1.15 = 0.00115 and 1000 * 0.000003 * 1.15 = 0.00345
    let charged = sum(&f, &tenant, "translator-usd", &["usage"]).await;
    assert_eq!(charged, d("-0.0046"));
    // Replays and idle processing change nothing.
    f.rpc("service", "IngestEvent", ingest("b", &new)).await;
    assert_eq!(f.worker.process_metering(100).await.unwrap(), 0);
    assert_eq!(
        sum(&f, &tenant, "translator-usd", &["usage", "correction"]).await,
        d("-0.0046")
    );
    // A correction keeps the frozen price and replaces the measured tokens.
    let replace = json!({"requestKey":key(),"tenantId":tenant,"eventKey":"a","expectedRevision":1,"kind":"EVENT_CORRECTION_KIND_REPLACE","eventType":TRANSLATOR,"attributes":attrs(["3000","","",""],["0.000001","","",""],&(Utc::now() - Duration::days(2)).to_rfc3339())});
    assert_eq!(
        f.rpc("service", "CorrectEvent", replace.clone()).await["revision"],
        2
    );
    assert_eq!(
        f.rpc("service", "CorrectEvent", replace).await["revision"],
        2
    );
    f.process_metering().await;
    assert_eq!(
        sum(&f, &tenant, "translator-usd", &["usage", "correction"]).await,
        d("-0.0069")
    );
    // A correction cannot reprice the accepted report, cheaper or dearer.
    for price in ["0.000002", "0.0000005"] {
        f.error("service","CorrectEvent",json!({"requestKey":key(),"tenantId":tenant,"eventKey":"a","expectedRevision":2,"kind":"EVENT_CORRECTION_KIND_REPLACE","eventType":TRANSLATOR,"attributes":attrs(["3000","","",""],[price,"","",""],&(Utc::now() - Duration::days(2)).to_rfc3339())}),"invalid_argument").await;
    }
    f.process_metering().await;
    assert_eq!(
        sum(&f, &tenant, "translator-usd", &["usage", "correction"]).await,
        d("-0.0069")
    );
    // A correction without a price for nonzero tokens is invalid and changes nothing.
    f.error("service","CorrectEvent",json!({"requestKey":key(),"tenantId":tenant,"eventKey":"a","expectedRevision":2,"kind":"EVENT_CORRECTION_KIND_REPLACE","eventType":TRANSLATOR,"attributes":attrs(["3000","","",""],["","","",""],&(Utc::now() - Duration::days(2)).to_rfc3339())}),"invalid_argument").await;
    // Cancelling removes the charge exactly; the other event is untouched.
    f.rpc("service","CorrectEvent",json!({"requestKey":key(),"tenantId":tenant,"eventKey":"a","expectedRevision":2,"kind":"EVENT_CORRECTION_KIND_CANCEL"})).await;
    f.process_metering().await;
    assert_eq!(
        sum(&f, &tenant, "translator-usd", &["usage", "correction"]).await,
        d("-0.00345")
    );
    assert_eq!(balance(&f, &tenant).await, d("99.99655"));
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn missing_or_malformed_price_evidence_is_rejected_without_estimates() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let tenant = tenant_of(&f, "owner").await;
    let ok_time = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let mut cases = vec![
        attrs(["1", "", "", ""], ["", "", "", ""], &ok_time),
        attrs(["", "1", "", ""], ["1", "", "", ""], &ok_time),
        attrs(["1", "", "", ""], ["1", "", "", ""], "yesterday"),
        attrs(
            ["1", "", "", ""],
            ["1", "", "", ""],
            &(Utc::now() + Duration::days(1)).to_rfc3339(),
        ),
        attrs(["1.5", "", "", ""], ["1", "", "", ""], &ok_time),
        attrs(["-1", "", "", ""], ["1", "", "", ""], &ok_time),
        attrs(["1", "", "", ""], ["-1", "", "", ""], &ok_time),
        // Noncanonical spellings are malformed, not normalized.
        attrs(["1.0", "", "", ""], ["1", "", "", ""], &ok_time),
        attrs(["1", "", "", ""], ["0.0100", "", "", ""], &ok_time),
    ];
    let mut wrong_type = attrs(["1", "", "", ""], ["1", "", "", ""], &ok_time);
    wrong_type["input_tokens"] = json!({"text":"1"});
    cases.push(wrong_type);
    let mut no_model = attrs(["1", "", "", ""], ["1", "", "", ""], &ok_time);
    no_model.as_object_mut().unwrap().remove("model");
    cases.push(no_model);
    for (i, attributes) in cases.into_iter().enumerate() {
        let body = f
            .error(
                "service",
                "IngestEvent",
                event(
                    &tenant,
                    &s.service,
                    &format!("bad-{i}"),
                    TRANSLATOR,
                    attributes,
                ),
                "invalid_argument",
            )
            .await;
        assert!(body["message"].as_str().is_some());
    }
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM metering_events")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(stored, 0);
    // Zero tokens are valid and cost nothing.
    f.rpc(
        "service",
        "IngestEvent",
        event(
            &tenant,
            &s.service,
            "zero",
            EXTRACTOR,
            attrs(["", "", "", ""], ["", "", "", ""], &ok_time),
        ),
    )
    .await;
    f.process_metering().await;
    assert_eq!(
        sum(&f, &tenant, "extractor-usd", &["usage", "correction"]).await,
        d("0")
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn credits_blocking_and_billing_checks_apply_to_call_time_usage() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let tenant = tenant_of(&f, "owner").await;
    let me = f.rpc("owner", "GetCurrentUser", json!({})).await["userId"].clone();
    let paid = |mode: Option<&str>| {
        let mut body = json!({"tenantId":tenant,"capability":"translator","paid":true});
        match mode {
            Some(m) => body["consumerAuthorization"] = json!(m),
            None => body["userId"] = me.clone(),
        }
        body
    };
    let key_mode = Some("CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY");
    // Free tenants are call-time customers; with no credit they are blocked on balance only.
    for mode in [None, key_mode] {
        let r = authorize(&f, paid(mode)).await;
        assert_eq!(r["allowed"], false);
        assert_eq!(reasons(&r), vec!["\"BLOCK_REASON_BALANCE\"".to_string()]);
    }
    credit(&f, &tenant, "0.01").await;
    for mode in [None, key_mode] {
        assert_eq!(authorize(&f, paid(mode)).await["allowed"], true);
    }
    // Two reports of 0.0084525 push the balance below zero; reporting never blocks.
    for id in ["u1", "u2"] {
        f.rpc(
            "service",
            "IngestEvent",
            event(&tenant, &s.service, id, TRANSLATOR, full()),
        )
        .await;
    }
    f.process_metering().await;
    assert_eq!(balance(&f, &tenant).await, d("-0.006905"));
    for mode in [None, key_mode] {
        let r = authorize(&f, paid(mode)).await;
        assert_eq!(r["allowed"], false);
        assert_eq!(reasons(&r), vec!["\"BLOCK_REASON_BALANCE\"".to_string()]);
    }
    // Operator blocking and deleting tenants still apply independently.
    credit(&f, &tenant, "10").await;
    assert_eq!(authorize(&f, paid(key_mode)).await["allowed"], true);
    sqlx::query("UPDATE tenants SET operator_block=true WHERE id=$1::text::uuid")
        .bind(&tenant)
        .execute(&f.db)
        .await
        .unwrap();
    assert!(
        reasons(&authorize(&f, paid(key_mode)).await)
            .contains(&"\"BLOCK_REASON_OPERATOR\"".to_string())
    );
    sqlx::query(
        "UPDATE tenants SET operator_block=false,deleting_at=now() WHERE id=$1::text::uuid",
    )
    .bind(&tenant)
    .execute(&f.db)
    .await
    .unwrap();
    assert!(
        reasons(&authorize(&f, paid(key_mode)).await)
            .contains(&"\"BLOCK_REASON_DELETING\"".to_string())
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn included_usage_of_other_meters_does_not_offset_a_depleted_call_time_balance() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    // A legacy meter of the same service carries a plan price with included usage.
    let plan = f.rpc("owner", "GetCatalog", json!({})).await["plans"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare meter","meter":{"serviceId":s.service,"name":"requests","key":"requests","eventType":"request","aggregation":"METER_AGGREGATION_COUNT"}})).await;
    f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"plan price","price":{"planId":plan,"serviceId":s.service,"meter":"requests","unitPrice":"1","included":"5","currency":"USD"}})).await;
    let tenant = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Allowance"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let held: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM allowance_grants WHERE tenant_id=$1::text::uuid AND amount>0",
    )
    .bind(&tenant)
    .fetch_one(&f.db)
    .await
    .unwrap();
    assert!(held > 0, "the tenant holds included usage of another meter");
    let paid = json!({"tenantId":tenant,"capability":"translator","paid":true,"consumerAuthorization":"CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY"});
    // Call-time usage never consumes that allowance, so the empty balance still blocks.
    let r = authorize(&f, paid.clone()).await;
    assert_eq!(r["allowed"], false);
    assert_eq!(reasons(&r), vec!["\"BLOCK_REASON_BALANCE\"".to_string()]);
    credit(&f, &tenant, "0.01").await;
    assert_eq!(authorize(&f, paid).await["allowed"], true);
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn authorization_modes_for_service_keys_personal_keys_and_key_management() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let org = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Org"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut users = std::collections::BTreeMap::new();
    for (actor, roles) in [
        ("admin", "ARRAY[2]"),
        ("billing", "ARRAY[3]"),
        ("member", "ARRAY[4]"),
        ("former", "ARRAY[4]"),
    ] {
        let id = f.rpc(actor, "GetCurrentUser", json!({})).await["userId"]
            .as_str()
            .unwrap()
            .to_owned();
        sqlx::query(&format!("INSERT INTO members(tenant_id,user_id,roles) VALUES($1::text::uuid,$2::text::uuid,{roles})")).bind(&org).bind(&id).execute(&f.db).await.unwrap();
        users.insert(actor, id);
    }
    users.insert(
        "stranger",
        f.rpc("stranger", "GetCurrentUser", json!({})).await["userId"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    users.insert(
        "owner",
        f.rpc("owner", "GetCurrentUser", json!({})).await["userId"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    // Only "member" and "former" hold the translator capability, through a group.
    f.rpc("owner","SetGroup",json!({"requestKey":key(),"tenantId":org,"group":{"name":"translators","userIds":[users["member"],users["former"]],"permissions":[{"serviceId":s.service,"capability":"translator"}]}})).await;
    let mode = |m: &str, user: &str, capability: &str, paid: bool| {
        let mut b =
            json!({"tenantId":org,"capability":capability,"paid":paid,"consumerAuthorization":m});
        if !user.is_empty() {
            b["userId"] = json!(users[user]);
        }
        b
    };
    let key_mode = "CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY";
    let personal = "CONSUMER_AUTHORIZATION_PERSONAL_KEY_MANAGEMENT";
    let service_keys = "CONSUMER_AUTHORIZATION_SERVICE_KEY_MANAGEMENT";

    // (a) Service keys act for the tenant without any member.
    assert_eq!(
        authorize(&f, mode(key_mode, "", "translator", false)).await["allowed"],
        true
    );
    assert_eq!(
        authorize(&f, mode(key_mode, "", "extractor", false)).await["allowed"],
        true
    );
    let unknown = authorize(&f, mode(key_mode, "", "unregistered", false)).await;
    assert_eq!(unknown["allowed"], false);
    assert_eq!(
        reasons(&unknown),
        vec!["\"BLOCK_REASON_PERMISSION\"".to_string()]
    );
    f.error(
        "service",
        "Authorize",
        mode(key_mode, "member", "translator", false),
        "invalid_argument",
    )
    .await;
    f.error(
        "service",
        "Authorize",
        mode(key_mode, "", "", false),
        "invalid_argument",
    )
    .await;
    let ghost =
        json!({"tenantId":key(),"capability":"translator","consumerAuthorization":key_mode});
    assert_eq!(authorize(&f, ghost).await["allowed"], false);
    // The numeric enum value is equivalent to its name.
    let numeric = json!({"tenantId":org,"capability":"translator","consumerAuthorization":1});
    assert_eq!(authorize(&f, numeric).await["allowed"], true);

    // (b) Personal keys follow the named user's current access and capability.
    let personal_key = |user: &str, capability: &str| json!({"tenantId":org,"userId":users[user],"capability":capability});
    assert_eq!(
        authorize(&f, personal_key("member", "translator")).await["allowed"],
        true
    );
    assert_eq!(
        authorize(&f, personal_key("member", "extractor")).await["allowed"],
        false
    );
    assert_eq!(
        authorize(&f, personal_key("admin", "translator")).await["allowed"],
        false
    );
    assert_eq!(
        authorize(&f, personal_key("stranger", "translator")).await["allowed"],
        false
    );
    assert_eq!(
        authorize(&f, personal_key("former", "translator")).await["allowed"],
        true
    );
    sqlx::query("DELETE FROM group_members WHERE user_id=$1::text::uuid")
        .bind(&users["former"])
        .execute(&f.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM members WHERE user_id=$1::text::uuid AND tenant_id=$2::text::uuid")
        .bind(&users["former"])
        .bind(&org)
        .execute(&f.db)
        .await
        .unwrap();
    assert_eq!(
        authorize(&f, personal_key("former", "translator")).await["allowed"],
        false
    );
    // The tenant service key is independent of that removal.
    assert_eq!(
        authorize(&f, mode(key_mode, "", "translator", false)).await["allowed"],
        true
    );

    // (c) Management checks use current membership and roles.
    for (user, personal_ok, service_ok) in [
        ("owner", true, true),
        ("admin", true, true),
        ("billing", true, false),
        ("member", true, false),
        ("stranger", false, false),
        ("former", false, false),
    ] {
        assert_eq!(
            authorize(&f, mode(personal, user, "", false)).await["allowed"],
            personal_ok,
            "personal {user}"
        );
        assert_eq!(
            authorize(&f, mode(service_keys, user, "", false)).await["allowed"],
            service_ok,
            "service {user}"
        );
    }
    f.error(
        "service",
        "Authorize",
        mode(personal, "member", "translator", false),
        "invalid_argument",
    )
    .await;
    f.error(
        "service",
        "Authorize",
        mode(service_keys, "admin", "", true),
        "invalid_argument",
    )
    .await;
    f.error(
        "service",
        "Authorize",
        mode(service_keys, "", "", false),
        "invalid_argument",
    )
    .await;
    f.error(
        "service",
        "Authorize",
        json!({"tenantId":org,"capability":"translator","consumerAuthorization":9}),
        "invalid_argument",
    )
    .await;

    // Withdrawn users and finally deleted tenants lose every mode.
    sqlx::query("UPDATE users SET withdrawing_at=now() WHERE id=$1::text::uuid")
        .bind(&users["admin"])
        .execute(&f.db)
        .await
        .unwrap();
    assert_eq!(
        authorize(&f, mode(service_keys, "admin", "", false)).await["allowed"],
        false
    );
    sqlx::query("UPDATE tenants SET deleted_at=now() WHERE id=$1::text::uuid")
        .bind(&org)
        .execute(&f.db)
        .await
        .unwrap();
    assert_eq!(
        authorize(&f, mode(key_mode, "", "translator", false)).await["allowed"],
        false
    );
    assert_eq!(
        authorize(&f, mode(personal, "member", "", false)).await["allowed"],
        false
    );

    // Modes grant no platform authority: user tokens, including operators, cannot use Authorize.
    f.error(
        "owner",
        "Authorize",
        mode(key_mode, "", "translator", false),
        "permission_denied",
    )
    .await;
    // Other services cannot use the modes, but keep the unchanged user check.
    f.rpc("owner","PutService",json!({"requestKey":key(),"reason":"other","service":{"name":"other-api","clientId":"other","capabilities":["translator"],"scopes":["authorize"]}})).await;
    let tenant2 = f
        .rpc(
            "owner",
            "CreateTenant",
            json!({"requestKey":key(),"name":"Org2"}),
        )
        .await["tenant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for m in [key_mode, personal, service_keys] {
        let user = if m == key_mode {
            json!("")
        } else {
            json!(users["owner"])
        };
        f.error("other","Authorize",json!({"tenantId":tenant2,"userId":user,"capability":if m==key_mode {"translator"} else {""},"consumerAuthorization":m}),"permission_denied").await;
    }
    let plain = f
        .rpc(
            "other",
            "Authorize",
            json!({"tenantId":tenant2,"userId":users["owner"],"capability":"translator"}),
        )
        .await;
    assert!(
        plain["allowed"].as_bool().unwrap_or(false),
        "the unchanged user check still works"
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn authorize_accepts_the_binary_protobuf_consumer_field() {
    use buffa::Message;
    use delino_protocol::delcore::v1::{
        AuthorizeRequest, AuthorizeResponse, ConsumerAuthorization,
    };
    let f = Fixture::setup().await;
    register(&f).await;
    let org = tenant_of(&f, "owner").await;
    let request = AuthorizeRequest {
        tenant_id: org,
        capability: "translator".into(),
        consumer_authorization: ConsumerAuthorization::CONSUMER_AUTHORIZATION_TENANT_SERVICE_KEY
            .into(),
        ..Default::default()
    };
    let response = f
        .http
        .post(format!("{}/delcore.v1.DelcoreService/Authorize", f.base))
        .bearer_auth(f.token("service"))
        .header("content-type", "application/proto")
        .header("connect-protocol-version", "1")
        .body(request.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap();
    assert!(
        AuthorizeResponse::decode_from_slice(bytes.as_ref())
            .unwrap()
            .allowed
    );
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn reserved_identity_is_bound_to_its_service_id() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let put =
        |service: Value| json!({"requestKey":key(),"reason":"service change","service":service});
    // Unchanged re-registration is idempotent and keeps the ID.
    let same = f.rpc("owner","PutService",put(json!({"id":s.service,"name":"delitools","clientId":"service","capabilities":["translator","extractor","archive"],"scopes":["authorize","usage:write","usage:read"]}))).await;
    assert_eq!(same["service"]["id"], s.service);
    // The reserved service cannot be renamed or moved to another key.
    for body in [
        json!({"id":s.service,"name":"DeliTools","clientId":"service","capabilities":["x"]}),
        json!({"id":s.service,"name":"renamed","clientId":"service","capabilities":["x"]}),
        json!({"id":s.service,"key":"renamed","name":"delitools","clientId":"service","capabilities":["x"]}),
        json!({"key":"delitools","name":"renamed","clientId":"another","capabilities":["x"]}),
    ] {
        f.error("owner", "PutService", put(body), "failed_precondition")
            .await;
    }
    // No other service can take its name or key.
    let other = f
        .rpc(
            "owner",
            "PutService",
            put(json!({"name":"search","clientId":"search","capabilities":["x"]})),
        )
        .await["service"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for body in [
        json!({"id":other,"name":"delitools","clientId":"search","capabilities":["x"]}),
        json!({"id":other,"key":"delitools","name":"search","clientId":"search","capabilities":["x"]}),
        json!({"name":"delitools","key":"another","clientId":"third","capabilities":["x"]}),
        json!({"name":"DeliTools","clientId":"fourth","capabilities":["x"]}),
    ] {
        f.error("owner", "PutService", put(body), "failed_precondition")
            .await;
    }
    // Other services keep their normal rename behavior.
    let renamed = f
        .rpc(
            "owner",
            "PutService",
            put(json!({"id":other,"name":"Search v2","clientId":"search","capabilities":["x"]})),
        )
        .await;
    assert_eq!(renamed["service"]["name"], "Search v2");
    let name: String = sqlx::query_scalar("SELECT name FROM services WHERE id=$1::text::uuid")
        .bind(&s.service)
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!(name, "delitools");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn serving_delitools_requires_the_explicit_switch_and_long_windows() {
    let f = Fixture::setup_with(&[
        ("DELITOOLS_ENABLED", "false"),
        ("USAGE_LATENESS_SECONDS", "604800"),
        ("REPORTING_GRACE_SECONDS", "604800"),
    ])
    .await;
    f.bootstrap().await;
    f.error("owner","PutService",json!({"requestKey":key(),"reason":"register","service":{"key":"delitools","name":"delitools","clientId":"service","capabilities":["translator"]}}),"failed_precondition").await;
    // Other services are unaffected.
    f.service().await;
    let mut conn = f.db.acquire().await.unwrap();
    delcore_api::delitools::ensure_configured(&mut conn, &f.config)
        .await
        .unwrap();
    // A registered service with the switch off is a startup error.
    sqlx::query(
        "INSERT INTO catalog_entities(id,kind,scope,key) VALUES($1,'service','','delitools')",
    )
    .bind(uuid::Uuid::now_v7())
    .execute(&mut *conn)
    .await
    .unwrap();
    assert!(
        delcore_api::delitools::ensure_configured(&mut conn, &f.config)
            .await
            .is_err()
    );
    let mut enabled = f.config.clone();
    enabled.delitools_enabled = true;
    delcore_api::delitools::ensure_configured(&mut conn, &enabled)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn call_time_meter_definitions_are_validated_and_immutable() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let other = f.rpc("owner","PutService",json!({"requestKey":key(),"reason":"other","service":{"name":"other-api","clientId":"other","capabilities":["x"],"scopes":["authorize"]}})).await["service"]["id"].as_str().unwrap().to_owned();
    let meter = |service: &str, name: &str, extra: Value| {
        let mut m = json!({"serviceId":service,"key":name,"name":name,"eventType":"x.usage","aggregation":"METER_AGGREGATION_SUM","valueSource":"METER_VALUE_SOURCE_AI_TOKEN_USD","markup":"1.15"});
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        json!({"requestKey":key(),"reason":"declare meter","meter":m})
    };
    // Only the reserved service may declare them.
    f.error(
        "owner",
        "PutMeter",
        meter(&other, "ai-usd", json!({})),
        "invalid_argument",
    )
    .await;
    for extra in [
        json!({"markup":""}),
        json!({"markup":"0.99"}),
        json!({"markup":"1e1"}),
        json!({"valueAttribute":"n"}),
        json!({"aggregation":"METER_AGGREGATION_COUNT"}),
        json!({"valueSource":"METER_VALUE_SOURCE_ATTRIBUTE"}),
    ] {
        f.error(
            "owner",
            "PutMeter",
            meter(&s.service, "bad-usd", extra),
            "invalid_argument",
        )
        .await;
    }
    // Identical re-declaration is idempotent; a different markup is a new definition.
    let a = f
        .rpc(
            "owner",
            "PutMeter",
            meter(
                &s.service,
                "translator-usd",
                json!({"eventType":TRANSLATOR}),
            ),
        )
        .await;
    let b = f
        .rpc(
            "owner",
            "PutMeter",
            meter(
                &s.service,
                "translator-usd",
                json!({"eventType":TRANSLATOR, "markup":"1.150"}),
            ),
        )
        .await;
    assert_eq!(a["meter"]["id"], b["meter"]["id"]);
    f.error(
        "owner",
        "PutMeter",
        meter(
            &s.service,
            "translator-usd",
            json!({"eventType":TRANSLATOR, "markup":"1.2"}),
        ),
        "invalid_argument",
    )
    .await;
    let catalog = f.rpc("owner", "GetCatalog", json!({})).await;
    let listed = catalog["meters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["key"] == "translator-usd")
        .unwrap()
        .clone();
    assert_eq!(listed["valueSource"], "METER_VALUE_SOURCE_AI_TOKEN_USD");
    assert_eq!(listed["markup"], "1.15");
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn a_meter_with_usage_prices_cannot_become_call_time() {
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let plan = f.rpc("owner", "GetCatalog", json!({})).await["plans"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // An untyped legacy meter can already carry a usage price.
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare legacy meter","meter":{"serviceId":s.service,"name":"legacy","key":"legacy"}})).await;
    f.rpc("owner","CreatePrice",json!({"requestKey":key(),"reason":"plan price","price":{"planId":plan,"serviceId":s.service,"meter":"legacy","unitPrice":"1","included":"5","currency":"USD"}})).await;
    let convert = json!({"requestKey":key(),"reason":"convert meter","meter":{"serviceId":s.service,"name":"legacy","key":"legacy","eventType":"legacy.usage","aggregation":"METER_AGGREGATION_SUM","valueSource":"METER_VALUE_SOURCE_AI_TOKEN_USD","markup":"1.15"}});
    f.error("owner", "PutMeter", convert, "invalid_argument")
        .await;
    // A meter without usage prices still converts.
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"declare legacy meter","meter":{"serviceId":s.service,"name":"unpriced","key":"unpriced"}})).await;
    f.rpc("owner","PutMeter",json!({"requestKey":key(),"reason":"convert meter","meter":{"serviceId":s.service,"name":"unpriced","key":"unpriced","eventType":"unpriced.usage","aggregation":"METER_AGGREGATION_SUM","valueSource":"METER_VALUE_SOURCE_AI_TOKEN_USD","markup":"1.15"}})).await;
    // The development catalog applies the same rule.
    let directory = std::env::temp_dir().join(format!("delcore-call-time-{}", key()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("service.yaml"), "kind: service\nkey: delitools\nname: delitools\nclient_id: service\ncapabilities: [translator, extractor]\nscopes: [authorize, usage:write, usage:read]\n").unwrap();
    std::fs::write(directory.join("meter.yaml"), "kind: meter\nservice: delitools\nkey: legacy\nevent_type: legacy.usage\naggregation: sum\nvalue_source: ai_token_usd\nmarkup: \"1.15\"\n").unwrap();
    let metrics = delcore_api::telemetry::Metrics::default();
    let result = delcore_api::catalog::sync(&f.db, &directory, &metrics).await;
    std::fs::remove_dir_all(&directory).unwrap();
    let error = format!(
        "{:#}",
        result.expect_err("a priced meter cannot become call-time")
    );
    assert!(error.contains("usage prices"), "{error}");
    // sqlx queues the rollback of the rejected synchronization on its pooled
    // connection until that connection is next used. End the leftover session so it
    // cannot block the fixture's schema cleanup.
    sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname=current_database() AND state='idle in transaction' AND pid<>pg_backend_pid()")
        .execute(&f.db)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn committed_sample_catalog_declares_the_delitools_meters() {
    let f = Fixture::setup().await;
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog");
    let metrics = delcore_api::telemetry::Metrics::default();
    assert!(
        delcore_api::catalog::sync(&f.db, &directory, &metrics)
            .await
            .unwrap()
            .changed
    );
    assert!(
        !delcore_api::catalog::sync(&f.db, &directory, &metrics)
            .await
            .unwrap()
            .changed
    );
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT ce.key,cv.definition->>'markup' FROM catalog_entities ce JOIN catalog_versions cv ON cv.id=ce.active_version_id WHERE ce.kind='meter' AND cv.definition->>'value_source'='2' ORDER BY ce.key")
        .fetch_all(&f.db).await.unwrap();
    assert_eq!(
        rows,
        vec![
            ("extractor-usd".into(), "1.3".into()),
            ("translator-usd".into(), "1.15".into())
        ]
    );
    let mut conn = f.db.acquire().await.unwrap();
    delcore_api::delitools::ensure_configured(&mut conn, &f.config)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable TEST_DATABASE_URL"]
async fn accepted_events_and_ledger_are_independent_of_consumer_deletion_windows() {
    use delcore_api::{events, metering};
    let f = Fixture::setup().await;
    let s = register(&f).await;
    let tenant = tenant_of(&f, "owner").await;
    credit(&f, &tenant, "5").await;
    // An old but accepted report: 29 days, within the 30-day window.
    let occurred = Utc::now() - Duration::days(29);
    let snapshot = attrs(
        ["1000", "500", "200", "100"],
        ["0.000002", "0.00001", "0.0000005", "0.0000025"],
        &(occurred - Duration::days(1)).to_rfc3339(),
    );
    let body = json!({"requestKey":key(),"event":{"tenantId":tenant,"serviceId":s.service,"eventKey":"old","eventType":TRANSLATOR,"occurredAt":occurred.to_rfc3339(),"attributes":snapshot}});
    let accepted = f.rpc("service", "IngestEvent", body.clone()).await;
    f.process_metering().await;
    let ledger = |f: &Fixture| {
        let db = f.db.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ledger")
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    let before = ledger(&f).await;
    // Delcore's reporting window is the only admission rule; windows are at least 30 days.
    assert!(
        f.config.usage_lateness_seconds >= 2_592_000
            && f.config.reporting_grace_seconds >= 2_592_000
    );
    // Long after the consumer would have deleted its own pending record, maintenance
    // and retention keep every accepted event revision, contribution and ledger entry.
    f.worker
        .maintain(Utc::now() + Duration::days(45))
        .await
        .unwrap();
    let events_kept: i64 = sqlx::query_scalar("SELECT count(*) FROM metering_events")
        .fetch_one(&f.db)
        .await
        .unwrap();
    let revisions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM metering_event_revisions WHERE attributes<>'{}'::jsonb",
    )
    .fetch_one(&f.db)
    .await
    .unwrap();
    let contributions: i64 = sqlx::query_scalar("SELECT count(*) FROM metering_contributions")
        .fetch_one(&f.db)
        .await
        .unwrap();
    assert_eq!((events_kept, revisions, contributions), (1, 1, 1));
    assert!(ledger(&f).await >= before);
    assert_eq!(
        sum(&f, &tenant, "translator-usd", &["usage"]).await,
        d("-0.0084525")
    );
    // A late redelivery of an already accepted report replays its identity and never
    // charges again, even after the admission window has passed.
    let mut tx = f.db.begin().await.unwrap();
    let policy = events::Policy {
        lateness: Duration::seconds(f.config.usage_lateness_seconds),
        reporting_grace: Duration::seconds(f.config.reporting_grace_seconds),
        limits: metering::Limits::default(),
    };
    let attributes = delcore_api::rpc::events::attributes(&body["event"]["attributes"]).unwrap();
    let microsecond = occurred.timestamp_micros();
    let input = events::Input {
        tenant_id: tenant.clone(),
        service_id: s.service.clone(),
        key: "old".into(),
        event_type: TRANSLATOR.into(),
        occurred_at: chrono::DateTime::from_timestamp_micros(microsecond).unwrap(),
        attributes,
    };
    let replay = events::ingest(
        &mut tx,
        Utc::now() + Duration::days(45),
        &policy,
        input.clone(),
    )
    .await
    .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event_id, accepted["eventId"].as_str().unwrap());
    // A new report with that occurrence is outside the window.
    let mut fresh = input;
    fresh.key = "new".into();
    assert_eq!(
        events::ingest(&mut tx, Utc::now() + Duration::days(45), &policy, fresh)
            .await
            .unwrap_err()
            .reason,
        14
    );
    tx.rollback().await.unwrap();
}
