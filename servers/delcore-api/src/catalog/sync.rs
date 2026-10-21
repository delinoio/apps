// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use crate::postgres::{self, catalog as db};
use chrono::Utc;
use sqlx::{PgConnection, PgPool};

#[derive(Clone, Debug, Default)]
pub struct SyncResult {
    pub files: usize,
    pub changed: bool,
    pub entities_created: i64,
    pub versions_created: i64,
    pub correlation: String,
}
const DIGEST: &str = r#"SELECT md5(jsonb_build_array(
 (SELECT jsonb_agg(jsonb_build_array(id,kind,scope,key,active_version_id) ORDER BY id) FROM catalog_entities),
 (SELECT jsonb_agg(id ORDER BY id) FROM catalog_versions),
 (SELECT jsonb_agg(jsonb_build_array(id,name,client_id,capabilities,scopes) ORDER BY id) FROM services),
 (SELECT jsonb_agg(jsonb_build_array(id,name,rank,free,is_default,postpaid) ORDER BY id) FROM plans),
 (SELECT jsonb_agg(jsonb_build_array(service_id,name) ORDER BY service_id,name) FROM meters),
 (SELECT jsonb_agg(jsonb_build_array(id,service_id,name) ORDER BY id) FROM addons),
 (SELECT jsonb_agg(jsonb_build_array(id,name) ORDER BY id) FROM features),
 (SELECT jsonb_agg(id ORDER BY id) FROM prices),
 (SELECT jsonb_agg(jsonb_build_array(product_id,feature_id,value) ORDER BY product_id,feature_id) FROM catalog_product_features))::text),
 (SELECT count(*) FROM catalog_entities),(SELECT count(*) FROM catalog_versions)"#;

/// Reconcile the full development catalog after schema and process-lock checks.
/// The transaction owns all mutations, including legacy subscription snapshots.
pub async fn sync(
    pool: &PgPool,
    directory: &Path,
    metrics: &crate::telemetry::Metrics,
) -> Result<SyncResult> {
    use crate::telemetry::Outcome;
    use tracing::Instrument;
    let started = std::time::Instant::now();
    let correlation = uuid::Uuid::now_v7().to_string();
    let span = tracing::info_span!("delcore.catalog.reconcile", delcore.correlation_id = %correlation, delcore.outcome = tracing::field::Empty, delcore.catalog.files = tracing::field::Empty, delcore.catalog.entities_created = tracing::field::Empty, delcore.catalog.versions_created = tracing::field::Empty);
    let result = apply(pool, directory, correlation.clone())
        .instrument(span.clone())
        .await;
    let outcome = match &result {
        Ok(r) if r.changed => Outcome::Applied,
        Ok(_) => Outcome::Unchanged,
        Err(_) => Outcome::Rejected,
    };
    metrics.catalog_applied(outcome, started.elapsed());
    span.record("delcore.outcome", outcome.as_str());
    if let Ok(r) = &result {
        span.record("delcore.catalog.files", r.files);
        span.record("delcore.catalog.entities_created", r.entities_created);
        span.record("delcore.catalog.versions_created", r.versions_created);
    }
    if let Err(error) = &result {
        tracing::error!(parent: &span, correlation_id = %correlation,
            category = "catalog_rejected", directory = %directory.display(),
            error = %diagnostic(error), "development catalog synchronization failed");
    }
    result
}

// Preserve file and validation context, but never print SQL values or protocol
// payloads included in dependency errors.
fn diagnostic(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| {
            if let Some(error) = cause.downcast_ref::<sqlx::Error>() {
                match error {
                    sqlx::Error::Database(db) => format!(
                        "database failure (SQLSTATE {})",
                        db.code().as_deref().unwrap_or("unknown")
                    ),
                    _ => "database dependency failure".to_owned(),
                }
            } else {
                cause.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(": ")
}

async fn apply(pool: &PgPool, directory: &Path, correlation: String) -> Result<SyncResult> {
    let items = load(directory)?;
    let mut result = SyncResult {
        files: items.len(),
        correlation,
        ..Default::default()
    };
    let mut tx = postgres::begin(pool).await?;
    let before: (String, i64, i64) = sqlx::query_as(DIGEST).fetch_one(&mut *tx).await?;
    snapshot_legacy(&mut tx, &items).await?;
    validate_database(&mut tx, &items).await?;
    let mut counts = BTreeMap::new();
    for kind in ["service", "meter", "feature", "plan", "addon", "price"] {
        let mut count = 0;
        for item in items.iter().filter(|item| item.kind == kind) {
            apply_item(&mut tx, item)
                .await
                .with_context(|| format!("catalog file {}", item.path.display()))?;
            count += 1;
        }
        counts.insert(kind, count);
    }
    // Rebuild only after all prices and grants have their final state. Prices
    // can refer to omitted products, which must also get a new active version.
    let mut products = BTreeSet::new();
    for item in &items {
        if matches!(item.kind.as_str(), "plan" | "addon") {
            let scope = if item.kind == "addon" {
                resolve(&mut tx, "service", "", &item.service).await?
            } else {
                String::new()
            };
            let id = resolve(&mut tx, &item.kind, &scope, &item.key).await?;
            replace_grants(&mut tx, &id, &item.features, &scope)
                .await
                .with_context(|| format!("catalog file {}", item.path.display()))?;
            products.insert((item.kind.clone(), id));
        }
        if item.kind == "price" {
            let (kind, scope, key) = if let Some(addon) = &item.add_on {
                (
                    "addon",
                    resolve(&mut tx, "service", "", &addon.service).await?,
                    &addon.key,
                )
            } else {
                ("plan", String::new(), &item.plan)
            };
            products.insert((kind.to_owned(), resolve(&mut tx, kind, &scope, key).await?));
        }
    }
    for (kind, id) in products {
        db::rebuild_product_version(&mut tx, &kind, &id).await?;
    }
    let after: (String, i64, i64) = sqlx::query_as(DIGEST).fetch_one(&mut *tx).await?;
    result.changed = before.0 != after.0;
    result.entities_created = after.1 - before.1;
    result.versions_created = after.2 - before.2;
    if result.changed {
        let reason = format!(
            "files={} services={} meters={} features={} plans={} addons={} prices={} entities_created={} versions_created={}",
            items.len(),
            counts["service"],
            counts["meter"],
            counts["feature"],
            counts["plan"],
            counts["addon"],
            counts["price"],
            result.entities_created,
            result.versions_created
        );
        postgres::audit::audit(
            &mut tx,
            postgres::audit::ACTOR_CATALOG,
            "",
            "catalog.reconcile",
            &reason,
            &result.correlation,
            Utc::now(),
        )
        .await?;
    }
    tx.commit().await?;
    tracing::info!(category="catalog_reconciliation",correlation_id=%result.correlation,outcome=if result.changed {"applied"} else {"unchanged"},files=result.files,entities_created=result.entities_created,versions_created=result.versions_created,"catalog synchronized");
    Ok(result)
}
async fn resolve(c: &mut PgConnection, kind: &str, scope: &str, key: &str) -> Result<String> {
    sqlx::query_scalar(
        "SELECT id::text FROM catalog_entities WHERE kind=$1 AND scope=$2 AND key=$3",
    )
    .bind(kind)
    .bind(scope)
    .bind(key)
    .fetch_optional(c)
    .await?
    .with_context(|| format!("unknown {kind} reference {key:?}"))
}
async fn snapshot_legacy(c: &mut PgConnection, items: &[Definition]) -> Result<()> {
    let now = Utc::now();
    for item in items {
        let row:Option<(String,bool)>=match item.kind.as_str() {
            "plan"=>sqlx::query_as(r#"SELECT p.id::text, EXISTS(SELECT 1 FROM tenants t WHERE t.plan_id=p.id AND NOT EXISTS (
 SELECT 1 FROM tenant_catalog_selections cs WHERE cs.tenant_id=t.id AND cs.product_id=t.plan_id AND cs.ends_at IS NULL))
 FROM catalog_entities ce JOIN plans p ON p.id=ce.id WHERE ce.kind='plan' AND ce.scope='' AND ce.key=$1"#).bind(&item.key).fetch_optional(&mut *c).await?,
            "addon"=>sqlx::query_as(r#"SELECT a.id::text, EXISTS(SELECT 1 FROM entitlements en JOIN prices p ON p.id=en.price_id
 WHERE (en.ends_at IS NULL OR en.ends_at>$3) AND p.addon_id=a.id AND NOT EXISTS (
 SELECT 1 FROM tenant_catalog_selections cs WHERE cs.tenant_id=en.tenant_id AND cs.product_id=a.id AND (cs.ends_at IS NULL OR cs.ends_at>$3)))
 FROM catalog_entities ce JOIN catalog_entities se ON se.kind='service' AND se.scope='' AND se.key=$2 AND ce.scope=se.id::text
 JOIN addons a ON a.id=ce.id AND a.service_id=se.id WHERE ce.kind='addon' AND ce.key=$1"#).bind(&item.key).bind(&item.service).bind(now).fetch_optional(&mut *c).await?,
            _=>continue,
        };
        if let Some((id, true)) = row {
            if item.kind == "plan" {
                db::snapshot_legacy_plan_selections(c, &id, now).await?;
            } else {
                db::snapshot_legacy_add_on_selections(c, &id, now).await?;
            }
        }
    }
    Ok(())
}
async fn validate_database(c: &mut PgConnection, items: &[Definition]) -> Result<()> {
    let mut plans = BTreeMap::new();
    let mut ranks = BTreeSet::new();
    let mut new_default = "";
    for d in items.iter().filter(|i| i.kind == "plan") {
        plans.insert(d.key.as_str(), d);
        ensure!(ranks.insert(d.rank.unwrap()), "duplicate plan rank");
        if d.default_free == Some(true) {
            ensure!(new_default.is_empty(), "multiple default free plans");
            new_default = &d.key;
        }
    }
    let (default_id,default_key):(String,String)=sqlx::query_as("SELECT COALESCE((SELECT p.id::text FROM plans p WHERE p.is_default),''), COALESCE((SELECT ce.key FROM plans p LEFT JOIN catalog_entities ce ON ce.id=p.id WHERE p.is_default),'')").fetch_one(&mut *c).await?;
    if !default_id.is_empty() {
        ensure!(
            new_default.is_empty()
                || new_default == default_key
                || plans.contains_key(default_key.as_str()),
            "cannot replace an omitted default free plan"
        );
        ensure!(
            !new_default.is_empty()
                || !plans
                    .get(default_key.as_str())
                    .is_some_and(|d| d.default_free != Some(true)),
            "cannot clear the default free plan without a replacement"
        );
    }
    for (key, d) in &plans {
        let existing:Option<String>=sqlx::query_scalar("SELECT COALESCE(ce.key,'') FROM plans p LEFT JOIN catalog_entities ce ON ce.id=p.id WHERE p.rank=$1").bind(d.rank.unwrap()).fetch_optional(&mut *c).await?;
        if let Some(existing) = existing {
            ensure!(
                existing == *key || plans.contains_key(existing.as_str()),
                "plan rank {} belongs to an omitted plan",
                d.rank.unwrap()
            );
        }
    }
    // Temporary negative ranks permit atomic swaps without weakening uniqueness.
    // Rollback restores both ranks and the default when any later check fails.
    if !new_default.is_empty() {
        sqlx::query("UPDATE plans SET is_default=false WHERE is_default")
            .execute(&mut *c)
            .await?;
    }
    let minimum: i32 = sqlx::query_scalar("SELECT COALESCE(min(rank),0) FROM plans")
        .fetch_one(&mut *c)
        .await?;
    let mut minimum = i64::from(minimum.min(0));
    for d in items.iter().filter(|i| i.kind == "plan") {
        let id: Option<String> = sqlx::query_scalar(
            "SELECT id::text FROM catalog_entities WHERE kind='plan' AND scope='' AND key=$1",
        )
        .bind(&d.key)
        .fetch_optional(&mut *c)
        .await?;
        if let Some(id) = id {
            minimum -= 1;
            ensure!(
                minimum >= i64::from(i32::MIN),
                "no available temporary plan rank"
            );
            sqlx::query("UPDATE plans SET rank=$2 WHERE id=$1::text::uuid")
                .bind(id)
                .bind(minimum as i32)
                .execute(&mut *c)
                .await?;
        }
    }
    Ok(())
}
async fn apply_item(c: &mut PgConnection, d: &Definition) -> Result<()> {
    let mut scope = String::new();
    let mut owner_kind = "";
    let mut owner_id = String::new();
    if matches!(d.kind.as_str(), "meter" | "feature" | "addon") {
        scope = resolve(c, "service", "", &d.service).await?;
    }
    if d.kind == "price" {
        if let Some(a) = &d.add_on {
            let sid = resolve(c, "service", "", &a.service).await?;
            owner_kind = "addon";
            owner_id = resolve(c, "addon", &sid, &a.key).await?;
        } else {
            owner_kind = "plan";
            owner_id = resolve(c, "plan", "", &d.plan).await?;
        }
        scope = format!("{owner_kind}:{owner_id}");
    }
    let id = db::ensure_catalog_entity(c, &d.kind, &scope, &d.key, "").await?;
    match d.kind.as_str() {
        "service" => {
            sqlx::query("INSERT INTO services(id,name,client_id,capabilities,scopes) VALUES($1::text::uuid,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,client_id=EXCLUDED.client_id,capabilities=EXCLUDED.capabilities,scopes=EXCLUDED.scopes").bind(&id).bind(&d.name).bind(&d.client_id).bind(&d.capabilities).bind(&d.scopes).execute(&mut *c).await?;
        }
        "meter" => apply_meter(c, d, &id, &scope).await?,
        "feature" => apply_feature(c, d, &id, &scope).await?,
        "plan" => {
            sqlx::query("INSERT INTO plans(id,name,rank,free,is_default,postpaid) VALUES($1::text::uuid,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,rank=EXCLUDED.rank,free=EXCLUDED.free,is_default=EXCLUDED.is_default,postpaid=EXCLUDED.postpaid").bind(&id).bind(&d.name).bind(d.rank.unwrap()).bind(d.free.unwrap()).bind(d.default_free.unwrap_or(false)).bind(d.postpaid.unwrap_or(false)).execute(&mut *c).await?;
        }
        "addon" => {
            let result=sqlx::query("INSERT INTO addons(id,service_id,name) VALUES($1::text::uuid,$2::text::uuid,$3) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE addons.service_id=EXCLUDED.service_id").bind(&id).bind(&scope).bind(&d.name).execute(&mut *c).await?;
            ensure!(result.rows_affected() == 1, "add-on service cannot change");
        }
        "price" => apply_price(c, d, &id, owner_kind, &owner_id).await?,
        _ => bail!("unknown kind"),
    }
    Ok(())
}
async fn apply_meter(c: &mut PgConnection, d: &Definition, id: &str, service: &str) -> Result<()> {
    let mut filters = Vec::new();
    for f in &d.filters {
        let values = f
            .values
            .iter()
            .map(|n| operand(n, &f.operator))
            .collect::<Result<Vec<_>>>()?;
        filters.push(
            json!({"attribute":f.attribute,"operator":operator(&f.operator),"typed_values":values}),
        );
    }
    let filters = crate::events::canonical_filters(&filters)?;
    let mut groups = d.group_by.clone();
    groups.sort();
    let definition = json!({"name":d.key,"event_type":d.event_type,"aggregation":aggregation(&d.aggregation),"value_attribute":d.value_attribute,"filters":filters,"group_by":if groups.is_empty() {Value::Null} else {json!(groups)}});
    let previous = db::active_catalog_version(c, "meter", id).await?;
    let untyped = if previous.is_empty() {
        true
    } else {
        let event:String=sqlx::query_scalar("SELECT COALESCE(definition->>'event_type','') FROM catalog_versions WHERE id=$1::text::uuid").bind(&previous).fetch_one(&mut *c).await?;
        event.is_empty()
    };
    if untyped {
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM metering_events WHERE $1::text::uuid=ANY(meter_ids))",
        )
        .bind(id)
        .fetch_one(&mut *c)
        .await?;
        let compatible = d.event_type == d.key
            && d.aggregation == "sum"
            && d.value_attribute == "quantity"
            && d.filters.is_empty()
            && d.group_by.is_empty();
        ensure!(
            !used || compatible,
            "meter definition is immutable for its key"
        );
    }
    let (version, _) = db::select_catalog_version(c, id, &definition).await?;
    ensure!(
        previous.is_empty() || previous == version || untyped,
        "meter definition is immutable for its key"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM meters WHERE service_id=$1::text::uuid AND name<>$2",
    )
    .bind(service)
    .bind(&d.key)
    .fetch_one(&mut *c)
    .await?;
    ensure!(count < 100, "service meter limit reached");
    sqlx::query(
        "INSERT INTO meters(service_id,name) VALUES($1::text::uuid,$2) ON CONFLICT DO NOTHING",
    )
    .bind(service)
    .bind(&d.key)
    .execute(&mut *c)
    .await?;
    Ok(())
}
async fn apply_feature(
    c: &mut PgConnection,
    d: &Definition,
    id: &str,
    service: &str,
) -> Result<()> {
    if d.feature_type == "metered" {
        resolve(c, "meter", service, &d.meter).await?;
    }
    let kind = feature_kind(&d.feature_type);
    let vt = value_type(&d.value_type);
    let definition = json!({"kind":kind,"value_type":vt,"meter_key":d.meter});
    let previous = db::active_catalog_version(c, "feature", id).await?;
    let (version, _) = db::select_catalog_version(c, id, &definition).await?;
    ensure!(
        previous.is_empty() || previous == version,
        "feature type and meter are immutable for its key"
    );
    let result=sqlx::query("INSERT INTO features(id,service_id,key,name,kind,value_type,meter_key) VALUES($1::text::uuid,$2::text::uuid,$3,$4,$5,$6,$7) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE features.service_id=EXCLUDED.service_id AND features.key=EXCLUDED.key AND features.kind=EXCLUDED.kind AND features.value_type=EXCLUDED.value_type AND features.meter_key=EXCLUDED.meter_key").bind(id).bind(service).bind(&d.key).bind(&d.name).bind(kind).bind(vt).bind(&d.meter).execute(&mut *c).await?;
    ensure!(
        result.rows_affected() == 1,
        "feature definition is immutable for its key"
    );
    Ok(())
}
async fn apply_price(
    c: &mut PgConnection,
    d: &Definition,
    id: &str,
    owner_kind: &str,
    owner: &str,
) -> Result<()> {
    let service = resolve(c, "service", "", &d.service).await?;
    if owner_kind == "addon" {
        let actual: String =
            sqlx::query_scalar("SELECT service_id::text FROM addons WHERE id=$1::text::uuid")
                .bind(owner)
                .fetch_one(&mut *c)
                .await?;
        ensure!(actual == service, "add-on price belongs to another service");
    }
    if d.price_kind == "usage" {
        resolve(c, "meter", &service, &d.meter).await?;
    }
    let unit = decimal(&d.unit_price)?;
    let included = decimal(&d.included)?;
    let kind = if d.price_kind == "recurring" {
        2i32
    } else {
        1i32
    };
    let definition = json!({"service":service,"meter":d.meter,"unit_price":unit,"included":included,"currency":d.currency,"external_price":d.external_price_id,"kind":kind});
    let (version, created) = db::select_catalog_version(c, id, &definition).await?;
    if created {
        sqlx::query("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,addon_id,kind) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,$4,$5::text::numeric,$6::text::numeric,$7,$8,now(),$9::text::uuid,$10)")
        .bind(version).bind((owner_kind=="plan").then_some(owner)).bind(service).bind(&d.meter).bind(unit).bind(included).bind(&d.currency).bind(&d.external_price_id).bind((owner_kind=="addon").then_some(owner)).bind(kind).execute(&mut *c).await?;
    }
    Ok(())
}
async fn replace_grants(
    c: &mut PgConnection,
    product: &str,
    grants: &[Grant],
    addon_service: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM catalog_product_features WHERE product_id=$1::text::uuid")
        .bind(product)
        .execute(&mut *c)
        .await?;
    for g in grants {
        let service = resolve(c, "service", "", &g.service).await?;
        ensure!(
            addon_service.is_empty() || service == addon_service,
            "add-on feature belongs to another service"
        );
        let feature = resolve(c, "feature", &service, &g.key).await?;
        let (kind, value_type): (i32, i32) =
            sqlx::query_as("SELECT kind,value_type FROM features WHERE id=$1::text::uuid")
                .bind(&feature)
                .fetch_one(&mut *c)
                .await?;
        let value = normalize_grant(g, kind, value_type)?;
        sqlx::query("INSERT INTO catalog_product_features(product_id,feature_id,value) VALUES($1::text::uuid,$2::text::uuid,$3)").bind(product).bind(feature).bind(value).execute(&mut *c).await?;
    }
    Ok(())
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn catalog_diagnostic_preserves_file_context_without_database_payloads() {
        let error = anyhow::Error::new(sqlx::Error::Protocol("private database payload".into()))
            .context("catalog file catalog/plans/free.yaml");
        let message = diagnostic(&error);
        assert!(message.contains("catalog/plans/free.yaml"));
        assert!(message.contains("database dependency failure"));
        assert!(!message.contains("private database payload"));
        let error =
            anyhow::anyhow!("duplicate plan rank").context("catalog file catalog/plan.yaml");
        assert_eq!(
            diagnostic(&error),
            "catalog file catalog/plan.yaml: duplicate plan rank"
        );
    }
}
