// SPDX-License-Identifier: FSL-1.1-ALv2

use crate::Result;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};

pub async fn ensure_catalog_entity(
    db: &mut PgConnection,
    kind: &str,
    scope: &str,
    key: &str,
    requested_id: &str,
) -> Result<String> {
    if let Some(id) = sqlx::query_scalar::<_, String>(
        "SELECT id::text FROM catalog_entities WHERE kind=$1 AND scope=$2 AND key=$3 FOR UPDATE",
    )
    .bind(kind)
    .bind(scope)
    .bind(key)
    .fetch_optional(&mut *db)
    .await?
    {
        if !requested_id.is_empty() && id != requested_id {
            return Err(crate::rpc::invalid(
                "catalog identity conflicts with its stable key",
            ));
        }
        return Ok(id);
    }
    let id = if requested_id.is_empty() {
        uuid::Uuid::now_v7().to_string()
    } else {
        requested_id.to_owned()
    };
    sqlx::query("INSERT INTO catalog_entities(id,kind,scope,key) VALUES($1::text::uuid,$2,$3,$4)")
        .bind(&id)
        .bind(kind)
        .bind(scope)
        .bind(key)
        .execute(db)
        .await?;
    Ok(id)
}

pub async fn catalog_key(db: &mut PgConnection, id: &str) -> Result<String> {
    Ok(
        sqlx::query_scalar("SELECT key FROM catalog_entities WHERE id=$1::text::uuid")
            .bind(id)
            .fetch_optional(db)
            .await?
            .unwrap_or_default(),
    )
}

pub async fn select_catalog_version(
    db: &mut PgConnection,
    entity: &str,
    definition: &Value,
) -> Result<(String, bool)> {
    let hash = hex::encode(Sha256::digest(
        serde_json::to_vec(definition).expect("JSON value serializes"),
    ));
    // The old Go implementation hashed struct fields in declaration order. JSONB
    // equality preserves those immutable version IDs across the runtime migration.
    // Keep this fallback while databases may contain Go-created versions.
    let existing: Option<String> = sqlx::query_scalar("SELECT id::text FROM catalog_versions WHERE entity_id=$1::text::uuid AND (semantic_hash=$2 OR definition=$3) ORDER BY created_at,id LIMIT 1").bind(entity).bind(&hash).bind(definition).fetch_optional(&mut *db).await?;
    let created = existing.is_none();
    let id = existing.unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    if created {
        sqlx::query("INSERT INTO catalog_versions(id,entity_id,semantic_hash,definition) VALUES($1::text::uuid,$2::text::uuid,$3,$4)").bind(&id).bind(entity).bind(hash).bind(definition).execute(&mut *db).await?;
    }
    sqlx::query(
        "UPDATE catalog_entities SET active_version_id=$2::text::uuid WHERE id=$1::text::uuid",
    )
    .bind(entity)
    .bind(&id)
    .execute(db)
    .await?;
    Ok((id, created))
}

pub async fn active_catalog_version(db: &mut PgConnection, kind: &str, id: &str) -> Result<String> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT active_version_id::text FROM catalog_entities WHERE id=$1::text::uuid AND kind=$2",
    )
    .bind(id)
    .bind(kind)
    .fetch_optional(db)
    .await?
    .flatten()
    .unwrap_or_default())
}

async fn materialized_product_version(
    db: &mut PgConnection,
    kind: &str,
    id: &str,
) -> Result<String> {
    match sqlx::query_scalar::<_, Option<String>>("SELECT active_version_id::text FROM catalog_entities WHERE id=$1::text::uuid AND kind=$2 FOR UPDATE").bind(id).bind(kind).fetch_optional(&mut *db).await? {
        None => Ok(String::new()),
        Some(Some(version)) => Ok(version),
        Some(None) => rebuild_product_version(db,kind,id).await,
    }
}

pub async fn active_product_versions(
    db: &mut PgConnection,
    plan: &str,
    addons: &[String],
) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let base = materialized_product_version(db, "plan", plan).await?;
    if !base.is_empty() {
        result.push(base);
    }
    let mut sorted = addons.to_vec();
    sorted.sort();
    for addon in sorted {
        let v = materialized_product_version(db, "addon", &addon).await?;
        if !v.is_empty() {
            result.push(v);
        }
    }
    Ok(result)
}

pub async fn product_version_prices(
    db: &mut PgConnection,
    product: &str,
    version: &str,
) -> Result<Vec<String>> {
    let value: sqlx::types::Json<Vec<String>> = sqlx::query_scalar("SELECT definition->'prices' FROM catalog_versions WHERE id=$1::text::uuid AND entity_id=$2::text::uuid").bind(version).bind(product).fetch_one(db).await?;
    Ok(value.0)
}

pub async fn conflicting_feature(db: &mut PgConnection, versions: &[String]) -> Result<bool> {
    if versions.len() < 2 {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_version_features WHERE version_id=ANY($1::text[]::uuid[]) GROUP BY feature_id HAVING count(*)>1)").bind(versions).fetch_one(db).await?)
}

pub async fn active_product_prices(
    db: &mut PgConnection,
    kind: &str,
    owner: &str,
) -> Result<Vec<String>> {
    let column = match kind {
        "plan" => "plan_id",
        "addon" => "addon_id",
        _ => return Err(crate::rpc::invalid("unknown product kind")),
    };
    let query = format!(
        r#"SELECT active_version_id::text FROM catalog_entities WHERE kind='price' AND scope=$2 AND active_version_id IS NOT NULL
 UNION ALL
 SELECT id::text FROM (SELECT DISTINCT ON (p.service_id,p.meter,p.kind) p.id
 FROM prices p LEFT JOIN catalog_versions cv ON cv.id=p.id
 WHERE p.{column}=$1::text::uuid AND cv.id IS NULL
 AND NOT EXISTS (SELECT 1 FROM catalog_entities ae
  JOIN prices ap ON ap.id=ae.active_version_id
  WHERE ae.kind='price' AND ae.scope=$2
    AND ap.plan_id IS NOT DISTINCT FROM p.plan_id
    AND ap.addon_id IS NOT DISTINCT FROM p.addon_id
    AND ap.service_id=p.service_id AND ap.meter=p.meter AND ap.kind=p.kind)
 ORDER BY p.service_id,p.meter,p.kind,p.created_at DESC,p.id DESC) legacy"#
    );
    Ok(sqlx::query_scalar(&query)
        .bind(owner)
        .bind(format!("{kind}:{owner}"))
        .fetch_all(db)
        .await?)
}

pub async fn rebuild_product_version(
    db: &mut PgConnection,
    kind: &str,
    product: &str,
) -> Result<String> {
    let (rank, free, postpaid) = match kind {
        "plan" => {
            sqlx::query_as::<_, (i32, bool, bool)>(
                "SELECT rank,free,postpaid FROM plans WHERE id=$1::text::uuid",
            )
            .bind(product)
            .fetch_one(&mut *db)
            .await?
        }
        "addon" => (0, false, false),
        _ => return Err(crate::rpc::invalid("unknown product kind")),
    };
    let mut prices = active_product_prices(db, kind, product).await?;
    prices.sort();
    let rows = sqlx::query("SELECT feature_id::text,value FROM catalog_product_features WHERE product_id=$1::text::uuid ORDER BY feature_id").bind(product).fetch_all(&mut *db).await?;
    let features: Vec<Value> = rows
        .iter()
        .map(|r| Ok(json!({"id": r.try_get::<String,_>(0)?, "value":r.try_get::<Value,_>(1)?})))
        .collect::<std::result::Result<_, sqlx::Error>>()?;
    let definition =
        json!({"rank":rank,"free":free,"postpaid":postpaid,"prices":prices,"features":features});
    let (version, created) = select_catalog_version(db, product, &definition).await?;
    if created {
        for feature in features {
            sqlx::query("INSERT INTO catalog_version_features(version_id,feature_id,value) VALUES($1::text::uuid,$2::text::uuid,$3)").bind(&version).bind(feature["id"].as_str().unwrap()).bind(&feature["value"]).execute(&mut *db).await?;
        }
    }
    Ok(version)
}

pub async fn snapshot_legacy_plan_selections(
    db: &mut PgConnection,
    plan: &str,
    at: DateTime<Utc>,
) -> Result<()> {
    let version = rebuild_product_version(db, "plan", plan).await?;
    let tenants: Vec<String> = sqlx::query_scalar(r#"SELECT t.id::text FROM tenants t WHERE t.plan_id=$1::text::uuid AND NOT EXISTS(SELECT 1 FROM tenant_catalog_selections cs WHERE cs.tenant_id=t.id AND cs.product_id=t.plan_id AND cs.ends_at IS NULL) FOR UPDATE"#).bind(plan).fetch_all(&mut *db).await?;
    for tenant in tenants {
        sqlx::query(r#"INSERT INTO tenant_catalog_selections(id,tenant_id,product_id,version_id,starts_at,ends_at)
SELECT $1::text::uuid,$2::text::uuid,$3::text::uuid,$4::text::uuid,GREATEST(
 LEAST($5::timestamptz,LEAST((SELECT period_start FROM tenants WHERE id=$2::text::uuid),
 (SELECT min(en.starts_at) FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$2::text::uuid AND en.ends_at IS NULL AND p.plan_id=$3::text::uuid))),
 (SELECT max(ends_at) FROM tenant_catalog_selections WHERE tenant_id=$2::text::uuid AND product_id=$3::text::uuid AND ends_at<=$5::timestamptz)),NULL::timestamptz ON CONFLICT DO NOTHING"#).bind(uuid::Uuid::now_v7().to_string()).bind(&tenant).bind(plan).bind(&version).bind(at).execute(&mut *db).await?;
    }
    Ok(())
}

pub async fn snapshot_legacy_add_on_selections(
    db: &mut PgConnection,
    addon: &str,
    at: DateTime<Utc>,
) -> Result<()> {
    let version = rebuild_product_version(db, "addon", addon).await?;
    let tenants: Vec<String> = sqlx::query_scalar(r#"SELECT t.id::text FROM tenants t WHERE EXISTS(SELECT 1 FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=t.id AND (en.ends_at IS NULL OR en.ends_at>$2) AND p.addon_id=$1::text::uuid) AND NOT EXISTS(SELECT 1 FROM tenant_catalog_selections cs WHERE cs.tenant_id=t.id AND cs.product_id=$1::text::uuid AND (cs.ends_at IS NULL OR cs.ends_at>$2)) FOR UPDATE"#).bind(addon).bind(at).fetch_all(&mut *db).await?;
    for tenant in tenants {
        sqlx::query(r#"INSERT INTO tenant_catalog_selections(id,tenant_id,product_id,version_id,starts_at,ends_at)
SELECT $1::text::uuid,$2::text::uuid,$3::text::uuid,$4::text::uuid,GREATEST(
 LEAST($5::timestamptz,(SELECT min(en.starts_at) FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$2::text::uuid AND (en.ends_at IS NULL OR en.ends_at>$5::timestamptz) AND p.addon_id=$3::text::uuid)),
 (SELECT max(ends_at) FROM tenant_catalog_selections WHERE tenant_id=$2::text::uuid AND product_id=$3::text::uuid AND ends_at<=$5::timestamptz)),
 (SELECT CASE WHEN bool_or(en.ends_at IS NULL) THEN NULL ELSE max(en.ends_at) END FROM entitlements en JOIN prices p ON p.id=en.price_id WHERE en.tenant_id=$2::text::uuid AND (en.ends_at IS NULL OR en.ends_at>$5::timestamptz) AND p.addon_id=$3::text::uuid)
ON CONFLICT DO NOTHING"#).bind(uuid::Uuid::now_v7().to_string()).bind(&tenant).bind(addon).bind(&version).bind(at).execute(&mut *db).await?;
    }
    Ok(())
}

pub async fn replace_tenant_selections(
    db: &mut PgConnection,
    tenant: &str,
    versions: &[String],
    at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("DELETE FROM tenant_catalog_selections WHERE tenant_id=$1::text::uuid AND ends_at IS NULL AND starts_at>=$2").bind(tenant).bind(at).execute(&mut *db).await?;
    sqlx::query("UPDATE tenant_catalog_selections SET ends_at=$2 WHERE tenant_id=$1::text::uuid AND ends_at IS NULL AND starts_at<$2").bind(tenant).bind(at).execute(&mut *db).await?;
    for version in versions.iter().filter(|v| !v.is_empty()) {
        let product: String = sqlx::query_scalar(
            "SELECT entity_id::text FROM catalog_versions WHERE id=$1::text::uuid",
        )
        .bind(version)
        .fetch_one(&mut *db)
        .await?;
        sqlx::query("INSERT INTO tenant_catalog_selections(id,tenant_id,product_id,version_id,starts_at) VALUES($1::text::uuid,$2::text::uuid,$3::text::uuid,$4::text::uuid,$5)").bind(uuid::Uuid::now_v7().to_string()).bind(tenant).bind(product).bind(version).bind(at).execute(&mut *db).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    /// Uses only transaction-local tables on an explicitly supplied disposable DB.
    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to disposable PostgreSQL"]
    async fn immutable_versions_reuse_legacy_hashes_and_preserve_identity() {
        let url = std::env::var("TEST_DATABASE_URL").expect("disposable database URL");
        let mut db = PgConnection::connect(&url).await.unwrap();
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql("CREATE TEMP TABLE catalog_entities(id uuid PRIMARY KEY,kind text,scope text,key text,active_version_id uuid,UNIQUE(kind,scope,key)) ON COMMIT DROP; CREATE TEMP TABLE catalog_versions(id uuid PRIMARY KEY,entity_id uuid,semantic_hash text,definition jsonb,created_at timestamptz DEFAULT now(),UNIQUE(entity_id,semantic_hash)) ON COMMIT DROP; CREATE TEMP TABLE catalog_version_features(version_id uuid,feature_id uuid,value jsonb) ON COMMIT DROP;").execute(&mut *tx).await.unwrap();
        let entity = ensure_catalog_entity(&mut tx, "plan", "", "basic", "")
            .await
            .unwrap();
        assert_eq!(
            entity,
            ensure_catalog_entity(&mut tx, "plan", "", "basic", "")
                .await
                .unwrap()
        );
        assert!(
            ensure_catalog_entity(
                &mut tx,
                "plan",
                "",
                "basic",
                &uuid::Uuid::now_v7().to_string()
            )
            .await
            .is_err()
        );
        let a = json!({"rank":0,"free":true,"postpaid":false,"prices":[],"features":[]});
        // Go's struct JSON order differs from serde_json::Value's sorted keys.
        let legacy_bytes = br#"{"rank":0,"free":true,"postpaid":false,"prices":[],"features":[]}"#;
        let legacy_id = uuid::Uuid::now_v7().to_string();
        sqlx::query("INSERT INTO catalog_versions(id,entity_id,semantic_hash,definition) VALUES($1::text::uuid,$2::text::uuid,$3,$4)").bind(&legacy_id).bind(&entity).bind(hex::encode(Sha256::digest(legacy_bytes))).bind(&a).execute(&mut *tx).await.unwrap();
        assert_eq!(
            select_catalog_version(&mut tx, &entity, &a).await.unwrap(),
            (legacy_id.clone(), false)
        );
        let b = json!({"rank":1,"free":true,"postpaid":false,"prices":[],"features":[]});
        let (version_b, created) = select_catalog_version(&mut tx, &entity, &b).await.unwrap();
        assert!(created);
        assert_ne!(legacy_id, version_b);
        assert_eq!(
            select_catalog_version(&mut tx, &entity, &a).await.unwrap(),
            (legacy_id.clone(), false)
        );
        assert_eq!(
            active_catalog_version(&mut tx, "plan", &entity)
                .await
                .unwrap(),
            legacy_id
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_versions")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 2);
        let feature = uuid::Uuid::now_v7();
        for version in [&legacy_id, &version_b] {
            sqlx::query("INSERT INTO catalog_version_features VALUES($1::text::uuid,$2,'{\"enabled\":true}')").bind(version).bind(feature).execute(&mut *tx).await.unwrap();
        }
        assert!(
            !conflicting_feature(&mut tx, std::slice::from_ref(&legacy_id))
                .await
                .unwrap()
        );
        assert!(
            conflicting_feature(&mut tx, &[legacy_id, version_b])
                .await
                .unwrap()
        );
        tx.rollback().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL pointing to disposable PostgreSQL"]
    async fn legacy_snapshots_preserve_backdated_plan_and_future_addon_entitlements() {
        let mut db = PgConnection::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql("CREATE TEMP TABLE catalog_entities(id uuid PRIMARY KEY,kind text,scope text,key text,active_version_id uuid,UNIQUE(kind,scope,key)) ON COMMIT DROP; CREATE TEMP TABLE catalog_versions(id uuid PRIMARY KEY,entity_id uuid,semantic_hash text,definition jsonb,created_at timestamptz DEFAULT now(),UNIQUE(entity_id,semantic_hash)) ON COMMIT DROP; CREATE TEMP TABLE catalog_product_features(product_id uuid,feature_id uuid,value jsonb) ON COMMIT DROP; CREATE TEMP TABLE catalog_version_features(version_id uuid,feature_id uuid,value jsonb) ON COMMIT DROP; CREATE TEMP TABLE plans(id uuid,rank integer,free boolean,postpaid boolean) ON COMMIT DROP; CREATE TEMP TABLE prices(id uuid,plan_id uuid,addon_id uuid,service_id uuid,meter text,kind integer,created_at timestamptz) ON COMMIT DROP; CREATE TEMP TABLE tenants(id uuid,plan_id uuid,period_start timestamptz) ON COMMIT DROP; CREATE TEMP TABLE entitlements(tenant_id uuid,price_id uuid,starts_at timestamptz,ends_at timestamptz) ON COMMIT DROP; CREATE TEMP TABLE tenant_catalog_selections(id uuid,tenant_id uuid,product_id uuid,version_id uuid,starts_at timestamptz,ends_at timestamptz) ON COMMIT DROP;").execute(&mut *tx).await.unwrap();
        let plan = ensure_catalog_entity(&mut tx, "plan", "", "legacy-plan", "")
            .await
            .unwrap();
        let addon = ensure_catalog_entity(&mut tx, "addon", "service", "legacy-addon", "")
            .await
            .unwrap();
        let tenant = uuid::Uuid::now_v7();
        let plan_price = uuid::Uuid::now_v7();
        let addon_price = uuid::Uuid::now_v7();
        let at: DateTime<Utc> = "2026-10-15T00:00:00Z".parse().unwrap();
        let period_start = at - chrono::Duration::days(14);
        let plan_start = period_start - chrono::Duration::days(3);
        let addon_start = period_start - chrono::Duration::days(1);
        let addon_end = at + chrono::Duration::days(5);
        sqlx::query("INSERT INTO plans VALUES($1::text::uuid,2,false,true)")
            .bind(&plan)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tenants VALUES($1,$2::text::uuid,$3)")
            .bind(tenant)
            .bind(&plan)
            .bind(period_start)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO prices VALUES($1,$2::text::uuid,NULL,$3,'',1,$4),($5,NULL,$6::text::uuid,$3,'',1,$4)").bind(plan_price).bind(&plan).bind(uuid::Uuid::now_v7()).bind(at).bind(addon_price).bind(&addon).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO entitlements VALUES($1,$2,$3,NULL),($1,$4,$5,$6)")
            .bind(tenant)
            .bind(plan_price)
            .bind(plan_start)
            .bind(addon_price)
            .bind(addon_start)
            .bind(addon_end)
            .execute(&mut *tx)
            .await
            .unwrap();
        snapshot_legacy_plan_selections(&mut tx, &plan, at)
            .await
            .unwrap();
        snapshot_legacy_add_on_selections(&mut tx, &addon, at)
            .await
            .unwrap();
        let plan_selection: (DateTime<Utc>,Option<DateTime<Utc>>) = sqlx::query_as("SELECT starts_at,ends_at FROM tenant_catalog_selections WHERE product_id=$1::text::uuid").bind(&plan).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(plan_selection, (plan_start, None));
        let addon_selection: (DateTime<Utc>,Option<DateTime<Utc>>) = sqlx::query_as("SELECT starts_at,ends_at FROM tenant_catalog_selections WHERE product_id=$1::text::uuid").bind(&addon).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(addon_selection, (addon_start, Some(addon_end)));
        sqlx::query("UPDATE plans SET rank=9 WHERE id=$1::text::uuid")
            .bind(&plan)
            .execute(&mut *tx)
            .await
            .unwrap();
        rebuild_product_version(&mut tx, "plan", &plan)
            .await
            .unwrap();
        let selected_rank: String = sqlx::query_scalar("SELECT cv.definition->>'rank' FROM tenant_catalog_selections cs JOIN catalog_versions cv ON cv.id=cs.version_id WHERE cs.product_id=$1::text::uuid").bind(&plan).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(selected_rank, "2");
        tx.rollback().await.unwrap();
    }
}
