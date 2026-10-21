// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{Call, forbidden, id, invalid, nonempty, page, precondition, string, uuid as db_uuid};
use crate::{Op, Result, events, postgres::catalog as postgres};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::HashSet;

fn uuid(value: &str) -> Result<uuid::Uuid> {
    if !super::canonical(value) {
        return Err(invalid("invalid UUID v7"));
    }
    db_uuid(value)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 100
        && key.as_bytes()[0].is_ascii_lowercase()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn derived_key(name: &str) -> String {
    let name = name.trim();
    let key = name.to_lowercase().replace([' ', '_'], "-");
    if valid_key(&key) {
        return key;
    }
    let mut base = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            base.push(c);
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    base = base.trim_matches('-').to_owned();
    if base.is_empty() {
        base = "key".into();
    } else if !base.as_bytes()[0].is_ascii_lowercase() {
        base = format!("key-{base}");
    }
    base.truncate(base.len().min(83));
    format!(
        "{}-{}",
        base.trim_end_matches('-'),
        hex::encode(&Sha256::digest(name.as_bytes())[..8])
    )
}
fn array(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
fn strings(v: &Value, key: &str) -> Vec<String> {
    array(v, key)
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
fn number(v: &Value, key: &str) -> i32 {
    if let Some(n) = v[key].as_i64() {
        return n as i32;
    }
    let text = string(v, key);
    let families: &[(&str, &[&str])] = &[
        (
            "FEATURE_KIND_",
            &["UNSPECIFIED", "ACCESS", "METERED", "CONFIGURATION"],
        ),
        ("PRICE_KIND_", &["UNSPECIFIED", "USAGE", "RECURRING"]),
        (
            "CONFIGURATION_VALUE_TYPE_",
            &[
                "UNSPECIFIED",
                "BOOLEAN",
                "INTEGER",
                "DECIMAL",
                "STRING",
                "STRING_LIST",
            ],
        ),
        (
            "METER_AGGREGATION_",
            &[
                "UNSPECIFIED",
                "COUNT",
                "SUM",
                "MIN",
                "MAX",
                "AVG",
                "UNIQUE_COUNT",
            ],
        ),
        (
            "METER_VALUE_SOURCE_",
            &["UNSPECIFIED", "ATTRIBUTE", "AI_TOKEN_USD"],
        ),
        (
            "METER_FILTER_OPERATOR_",
            &[
                "UNSPECIFIED",
                "EQUAL",
                "NOT_EQUAL",
                "IN",
                "GREATER",
                "GREATER_OR_EQUAL",
                "LESS",
                "LESS_OR_EQUAL",
                "EXISTS",
            ],
        ),
    ];
    for (prefix, names) in families {
        if let Some(name) = text.strip_prefix(prefix) {
            return names
                .iter()
                .position(|n| *n == name)
                .map(|n| n as i32)
                .unwrap_or(-1);
        }
    }
    if text.is_empty() {
        0
    } else {
        text.parse().unwrap_or(-1)
    }
}
// JSON uses protobuf field names at the boundary and SQL definition names internally.
fn field_names(value: &Value, camel: bool) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, v)| {
                    let mut name = String::new();
                    let mut upper = false;
                    for c in key.chars() {
                        if camel && c == '_' {
                            upper = true;
                            continue;
                        }
                        if camel {
                            if upper {
                                name.extend(c.to_uppercase());
                                upper = false;
                            } else {
                                name.push(c);
                            }
                        } else if c.is_ascii_uppercase() {
                            name.push('_');
                            name.push(c.to_ascii_lowercase());
                        } else {
                            name.push(c);
                        }
                    }
                    (name, field_names(v, camel))
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.iter().map(|v| field_names(v, camel)).collect())
        }
        _ => value.clone(),
    }
}
fn catalog_page(value: &Value) -> Result<(i64, String)> {
    page(&field_names(value, true))
}

fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
fn decimal(s: &str, nonnegative: bool) -> Result<String> {
    let d = crate::accounting::Decimal::parse(s).map_err(|_| invalid("invalid decimal"))?;
    if s.len() > 64 || (nonnegative && d < crate::accounting::Decimal::zero()) {
        return Err(invalid("invalid decimal"));
    }
    Ok(d.to_string())
}
fn configuration(v: &Value, kind: i32) -> Result<Value> {
    let keys = ["boolean", "integer", "decimal", "text", "string_list"];
    if keys.iter().filter(|k| !v[**k].is_null()).count() != 1 {
        return Err(invalid(
            "configuration value does not match its feature type",
        ));
    }
    match kind {
        1 if v["boolean"].is_boolean() => Ok(json!({"boolean":v["boolean"]})),
        2 => {
            let x = v["integer"]
                .as_i64()
                .or_else(|| v["integer"].as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| invalid("invalid integer configuration"))?;
            Ok(json!({"integer":x}))
        }
        3 => Ok(json!({"decimal":decimal(string(v,"decimal"),false)?})),
        4 if v["text"].is_string() && string(v, "text").len() <= 4096 => {
            Ok(json!({"text":v["text"]}))
        }
        5 => {
            let list = v["string_list"]
                .as_object()
                .ok_or_else(|| invalid("invalid string list"))?;
            // Protobuf JSON omits an empty repeated field inside the selected
            // string-list message. Its presence still grants an empty list.
            let values: &[Value] = match list.get("values") {
                None => &[],
                Some(Value::Array(values)) => values,
                Some(_) => return Err(invalid("invalid string list")),
            };
            if values.len() > 100
                || values
                    .iter()
                    .any(|v| v.as_str().is_none_or(|s| s.len() > 4096))
            {
                return Err(invalid("configuration list item is too long"));
            }
            Ok(json!({"string_list":values}))
        }
        _ => Err(invalid(
            "configuration value does not match its feature type",
        )),
    }
}
fn stored_configuration(mut value: Value) -> Value {
    if let Some(list) = value.get("string_list") {
        // Legacy Go grants can store a nil slice as null. Preserve the selected
        // string-list oneof by returning a message with an empty values array.
        let values = if list.is_null() {
            json!([])
        } else {
            list.clone()
        };
        value["string_list"] = json!({"values":values});
    }
    if let Some(integer) = value["integer"].as_i64() {
        value["integer"] = json!(integer.to_string());
    }
    value
}

impl Call<'_> {
    async fn catalog_key(
        &mut self,
        entity: &str,
        supplied: &str,
        fallback: &str,
    ) -> Result<String> {
        if !supplied.is_empty() {
            if !valid_key(supplied) {
                return Err(invalid("invalid catalog key"));
            }
            return Ok(supplied.into());
        }
        if !entity.is_empty() {
            let key = postgres::catalog_key(self.tx, entity).await?;
            if !key.is_empty() {
                return Ok(key);
            }
        }
        Ok(derived_key(fallback))
    }
    async fn replace_product_features(
        &mut self,
        product: &str,
        values: &[Value],
        replace: bool,
        service: &str,
    ) -> Result<()> {
        if !replace && values.is_empty() {
            return Ok(());
        }
        if values.len() > 100 {
            return Err(invalid("too many feature values"));
        }
        let mut seen = HashSet::new();
        let mut grants = Vec::new();
        for value in values {
            let feature = string(value, "feature_id");
            uuid(feature)?;
            if !seen.insert(feature) {
                return Err(invalid("invalid or repeated feature"));
            }
            let row =
                sqlx::query("SELECT kind,value_type,service_id::text FROM features WHERE id=$1")
                    .bind(uuid(feature)?)
                    .fetch_optional(&mut *self.tx)
                    .await?
                    .ok_or_else(|| invalid("unknown feature"))?;
            if !service.is_empty() && row.get::<String, _>("service_id") != service {
                return Err(invalid("add-on feature belongs to another service"));
            }
            let normalized = match row.get::<i32, _>("kind") {
                1 => {
                    if !value["enabled"].is_boolean()
                        || !string(value, "limit").is_empty()
                        || !value["configuration"].is_null()
                    {
                        return Err(invalid("access feature requires a boolean value"));
                    }
                    json!({"enabled":value["enabled"]})
                }
                2 => {
                    if !value["enabled"].is_null() || !value["configuration"].is_null() {
                        return Err(invalid(
                            "metered feature requires a nonnegative decimal limit",
                        ));
                    }
                    json!({"limit":decimal(string(value,"limit"),true)?})
                }
                3 => {
                    if !value["enabled"].is_null()
                        || !string(value, "limit").is_empty()
                        || value["configuration"].is_null()
                    {
                        return Err(invalid("configuration feature requires a typed value"));
                    }
                    configuration(&value["configuration"], row.get("value_type"))?
                }
                _ => return Err(invalid("unknown feature kind")),
            };
            grants.push((feature.to_owned(), normalized));
        }
        sqlx::query("DELETE FROM catalog_product_features WHERE product_id=$1")
            .bind(uuid(product)?)
            .execute(&mut *self.tx)
            .await?;
        for (feature, value) in grants {
            sqlx::query("INSERT INTO catalog_product_features(product_id,feature_id,value) VALUES($1,$2,$3)").bind(uuid(product)?).bind(uuid(&feature)?).bind(value).execute(&mut *self.tx).await?;
        }
        Ok(())
    }
    async fn catalog_feature_values(&mut self, version: &str) -> Result<Vec<Value>> {
        if version.is_empty() {
            return Ok(vec![]);
        }
        let rows=sqlx::query("SELECT feature_id::text,value FROM catalog_version_features WHERE version_id=$1 ORDER BY feature_id").bind(uuid(version)?).fetch_all(&mut *self.tx).await?;
        Ok(rows
            .iter()
            .map(|r| {
                let raw: Value = r.get("value");
                let mut v = json!({"feature_id":r.get::<String,_>("feature_id")});
                if !raw["enabled"].is_null() {
                    v["enabled"] = raw["enabled"].clone();
                } else if !raw["limit"].is_null() {
                    v["limit"] = raw["limit"].clone();
                } else {
                    v["configuration"] = stored_configuration(raw);
                }
                v
            })
            .collect())
    }
    pub async fn catalog(&mut self, op: Op, q: &Value) -> Result<Value> {
        let normalized = field_names(q, false);
        let value = self.catalog_internal(op, &normalized).await?;
        Ok(field_names(&value, true))
    }
    async fn catalog_internal(&mut self, op: Op, q: &Value) -> Result<Value> {
        match op {
            Op::GetCatalog => return self.get_catalog(q).await,
            Op::SetGroup | Op::ListGroups | Op::DeleteGroup => {
                return self.catalog_groups(op, q).await;
            }
            Op::ListBillingHistory => return self.catalog_billing_history(q).await,
            _ => {}
        }
        let reason = string(q, "reason");
        self.operator(reason)?;
        let (field, kind) = match op {
            Op::PutService => ("service", "service"),
            Op::PutPlan => ("plan", "plan"),
            Op::PutAddOn => ("add_on", "addon"),
            Op::PutFeature => ("feature", "feature"),
            Op::PutMeter => ("meter", "meter"),
            Op::CreatePrice => ("price", "price"),
            _ => return Err(invalid("unknown catalog operation")),
        };
        let mut v = q[field].clone();
        if !v.is_object() {
            return Err(invalid("catalog object required"));
        }
        if kind == "price" {
            v = self.create_catalog_price(v).await?;
        } else {
            nonempty(string(&v, "name"), if kind == "meter" { 100 } else { 200 })?;
            if !string(&v, "id").is_empty() {
                uuid(string(&v, "id"))?;
            }
            let service = string(&v, "service_id").to_owned();
            if matches!(kind, "addon" | "feature" | "meter") {
                uuid(&service)?;
            }
            let key = if kind == "feature" {
                let k = string(&v, "key");
                if !valid_key(k) {
                    return Err(invalid("invalid feature key"));
                }
                k.to_owned()
            } else {
                self.catalog_key(string(&v, "id"), string(&v, "key"), string(&v, "name"))
                    .await?
            };
            let scope = if matches!(kind, "addon" | "feature" | "meter") {
                service.as_str()
            } else {
                ""
            };
            if kind == "service" {
                self.check_reserved_service(&v, &key).await?;
            }
            let entity =
                postgres::ensure_catalog_entity(self.tx, kind, scope, &key, string(&v, "id"))
                    .await?;
            v["id"] = json!(entity);
            v["key"] = json!(key);
            match kind {
                "service" => {
                    nonempty(string(&v, "client_id"), 200)?;
                    let caps = strings(&v, "capabilities");
                    let scopes = strings(&v, "scopes");
                    if caps.is_empty() || caps.len() > 100 || scopes.len() > 10 {
                        return Err(invalid("invalid service permissions"));
                    }
                    for cap in &caps {
                        nonempty(cap, 100)?;
                    }
                    if scopes
                        .iter()
                        .any(|s| !matches!(s.as_str(), "authorize" | "usage:write" | "usage:read"))
                    {
                        return Err(invalid("unknown service scope"));
                    }
                    sqlx::query("INSERT INTO services VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,client_id=EXCLUDED.client_id,capabilities=EXCLUDED.capabilities,scopes=EXCLUDED.scopes").bind(uuid(&entity)?).bind(string(&v,"name")).bind(string(&v,"client_id")).bind(caps).bind(scopes).execute(&mut *self.tx).await?;
                }
                "plan" => {
                    let free = flag(&v, "free");
                    let default = flag(&v, "default_free");
                    let postpaid = flag(&v, "postpaid");
                    if number(&v, "rank") < 0 || default && !free {
                        return Err(invalid("invalid plan"));
                    }
                    if free && postpaid {
                        return Err(invalid("free plans cannot enable postpaid continuation"));
                    }
                    let existing: bool =
                        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plans WHERE id=$1)")
                            .bind(uuid(&entity)?)
                            .fetch_one(&mut *self.tx)
                            .await?;
                    if existing {
                        postgres::snapshot_legacy_plan_selections(self.tx, &entity, self.now)
                            .await?;
                    }
                    if !default {
                        let current: bool = sqlx::query_scalar(
                            "SELECT EXISTS(SELECT 1 FROM plans WHERE id=$1 AND is_default)",
                        )
                        .bind(uuid(&entity)?)
                        .fetch_one(&mut *self.tx)
                        .await?;
                        if current {
                            return Err(precondition(
                                "select a replacement default free plan before clearing this default",
                            ));
                        }
                    }
                    if default {
                        sqlx::query("UPDATE plans SET is_default=false WHERE is_default")
                            .execute(&mut *self.tx)
                            .await?;
                    }
                    sqlx::query("INSERT INTO plans VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,rank=EXCLUDED.rank,free=EXCLUDED.free,is_default=EXCLUDED.is_default,postpaid=EXCLUDED.postpaid").bind(uuid(&entity)?).bind(string(&v,"name")).bind(number(&v,"rank")).bind(free).bind(default).bind(postpaid).execute(&mut *self.tx).await?;
                }
                "addon" => {
                    let existing: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM addons WHERE id=$1 AND service_id=$2)",
                    )
                    .bind(uuid(&entity)?)
                    .bind(uuid(&service)?)
                    .fetch_one(&mut *self.tx)
                    .await?;
                    if existing {
                        postgres::snapshot_legacy_add_on_selections(self.tx, &entity, self.now)
                            .await?;
                    }
                    let tag=sqlx::query("INSERT INTO addons VALUES($1,$2,$3) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE addons.service_id=EXCLUDED.service_id").bind(uuid(&entity)?).bind(uuid(&service)?).bind(string(&v,"name")).execute(&mut *self.tx).await?;
                    if tag.rows_affected() != 1 {
                        return Err(invalid("add-on service cannot change"));
                    }
                }
                "feature" => self.put_catalog_feature(&v).await?,
                "meter" => v = self.put_catalog_meter(v).await?,
                _ => unreachable!(),
            }
            if matches!(kind, "plan" | "addon") {
                self.replace_product_features(
                    &entity,
                    &array(&v, "feature_values"),
                    flag(q, "replace_feature_values"),
                    if kind == "addon" { &service } else { "" },
                )
                .await?;
                let version = postgres::rebuild_product_version(self.tx, kind, &entity).await?;
                v["feature_values"] = json!(self.catalog_feature_values(&version).await?);
                v["active_version_id"] = json!(version);
            }
        }
        self.audit("", &format!("catalog.{kind}"), reason).await?;
        Ok(json!({field:v}))
    }
    /// Keep the reserved DeliTools identity bound to its service ID: it cannot be
    /// renamed, and no other service can be renamed into its key or name. Serving
    /// DeliTools also needs the explicit configuration switch.
    async fn check_reserved_service(&mut self, v: &Value, key: &str) -> Result<()> {
        let existing_key: Option<String> = if string(v, "id").is_empty() {
            None
        } else {
            sqlx::query_scalar(
                "SELECT key FROM catalog_entities WHERE id=$1 AND kind='service' AND scope=''",
            )
            .bind(uuid(string(v, "id"))?)
            .fetch_optional(&mut *self.tx)
            .await?
        };
        if crate::delitools::check_service_identity(existing_key.as_deref(), key, string(v, "name"))
            .is_err()
        {
            return Err(precondition(
                "the reserved DeliTools service identity is immutable and bound to its service ID",
            ));
        }
        if key == crate::delitools::SERVICE_KEY && !self.config.delitools_enabled {
            return Err(precondition(
                "registering the DeliTools service requires DELITOOLS_ENABLED=true with 30-day reporting windows",
            ));
        }
        Ok(())
    }
    async fn put_catalog_feature(&mut self, v: &Value) -> Result<()> {
        let kind = number(v, "kind");
        let value_type = number(v, "value_type");
        let meter = string(v, "meter_key");
        let service = string(v, "service_id");
        let entity = string(v, "id");
        match kind {
            1 if value_type == 0 && meter.is_empty() => {}
            2 if value_type == 0 && valid_key(meter) => {
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_entities WHERE kind='meter' AND scope=$1 AND key=$2)").bind(service).bind(meter).fetch_one(&mut *self.tx).await?;
                if !exists {
                    return Err(invalid("unknown feature meter"));
                }
            }
            3 if (1..=5).contains(&value_type) && meter.is_empty() => {}
            _ => return Err(invalid("invalid feature kind, meter or configuration type")),
        }
        let previous = postgres::active_catalog_version(self.tx, "feature", entity).await?;
        let (version, _) = postgres::select_catalog_version(
            self.tx,
            entity,
            &json!({"kind":kind,"value_type":value_type,"meter_key":meter}),
        )
        .await?;
        if !previous.is_empty() && previous != version {
            return Err(invalid("feature type and meter are immutable for its key"));
        }
        let tag=sqlx::query("INSERT INTO features(id,service_id,key,name,kind,value_type,meter_key) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name WHERE features.service_id=EXCLUDED.service_id AND features.key=EXCLUDED.key AND features.kind=EXCLUDED.kind AND features.value_type=EXCLUDED.value_type AND features.meter_key=EXCLUDED.meter_key").bind(uuid(entity)?).bind(uuid(service)?).bind(string(v,"key")).bind(string(v,"name")).bind(kind).bind(value_type).bind(meter).execute(&mut *self.tx).await?;
        if tag.rows_affected() != 1 {
            return Err(invalid("feature definition is immutable for its key"));
        }
        Ok(())
    }
    async fn put_catalog_meter(&mut self, mut v: Value) -> Result<Value> {
        let entity = string(&v, "id").to_owned();
        let service = string(&v, "service_id").to_owned();
        if string(&v, "name") != string(&v, "key") {
            return Err(invalid("meter name must match its stable key"));
        }
        let filters = array(&v, "filters");
        let mut groups = strings(&v, "group_by");
        let value_source = number(&v, "value_source");
        let call_time = value_source == events::VALUE_SOURCE_AI_TOKEN_USD;
        if !(0..=events::VALUE_SOURCE_AI_TOKEN_USD).contains(&value_source) {
            return Err(invalid("unknown meter value source"));
        }
        let markup = if call_time {
            events::canonical_markup(string(&v, "markup"))?
        } else {
            string(&v, "markup").to_owned()
        };
        if call_time && !crate::delitools::is_delitools(self.tx, &service).await? {
            return Err(invalid(
                "AI token USD meters belong to the delitools service",
            ));
        }
        if call_time
            && crate::delitools::meter_has_usage_prices(self.tx, &service, string(&v, "name"))
                .await?
        {
            return Err(invalid(
                "the meter has usage prices; a call-time meter cannot have usage prices",
            ));
        }
        let legacy = string(&v, "event_type").is_empty()
            && number(&v, "aggregation") == 0
            && string(&v, "value_attribute").is_empty()
            && filters.is_empty()
            && groups.is_empty()
            && value_source == 0
            && markup.is_empty();
        if number(&v, "aggregation") == 0 {
            v["aggregation"] = json!(2);
        }
        if !(1..=6).contains(&number(&v, "aggregation")) {
            return Err(invalid("unknown meter aggregation"));
        }
        if string(&v, "event_type").len() > 100
            || string(&v, "value_attribute").len() > 100
            || filters.len() > 100
            || groups.len() > events::MAX_GROUP_ATTRIBUTES
        {
            return Err(invalid("meter definition exceeds catalog limits"));
        }
        if !legacy {
            events::catalog_definition(
                string(&v, "name"),
                string(&v, "event_type"),
                number(&v, "aggregation"),
                string(&v, "value_attribute"),
                &filters,
                &groups,
                value_source,
                &markup,
            )?;
        }
        for f in &filters {
            if string(f, "attribute").is_empty()
                || !(1..=8).contains(&number(f, "operator"))
                || array(f, "values").len() + array(f, "typed_values").len() > 100
            {
                return Err(invalid("invalid meter filter"));
            }
        }
        let filters = events::canonical_filters(&filters)?;
        groups.sort();
        for (i, g) in groups.iter().enumerate() {
            nonempty(g, 100)?;
            if i > 0 && groups[i - 1] == *g {
                return Err(invalid("repeated meter group attribute"));
            }
        }
        let mut definition = json!({"name":string(&v,"name"),"event_type":string(&v,"event_type"),"aggregation":number(&v,"aggregation"),"value_attribute":string(&v,"value_attribute"),"filters":filters,"group_by":if groups.is_empty(){Value::Null}else{json!(groups)}});
        if call_time {
            // Added only for the call-time source so earlier definitions keep their
            // semantic hashes and versions.
            definition["value_source"] = json!(value_source);
            definition["markup"] = json!(markup);
            v["markup"] = json!(markup);
        }
        let previous = postgres::active_catalog_version(self.tx, "meter", &entity).await?;
        let stored: Value = if previous.is_empty() {
            json!({})
        } else {
            sqlx::query_scalar("SELECT definition FROM catalog_versions WHERE id=$1")
                .bind(uuid(&previous)?)
                .fetch_one(&mut *self.tx)
                .await?
        };
        let untyped = string(&stored, "event_type").is_empty();
        if legacy && !previous.is_empty() {
            for (k, value) in stored.as_object().into_iter().flatten() {
                v[k] = value.clone();
            }
        } else {
            if !legacy && untyped {
                let typed = events::catalog_definition(
                    string(&v, "name"),
                    string(&v, "event_type"),
                    number(&v, "aggregation"),
                    string(&v, "value_attribute"),
                    &filters,
                    &groups,
                    value_source,
                    &markup,
                )?;
                if events::untyped_meter_in_use(self.tx, &entity).await?
                    && !events::is_compatibility_definition(&typed)
                {
                    return Err(invalid("meter definition is immutable for its key"));
                }
            }
            let (version, _) =
                postgres::select_catalog_version(self.tx, &entity, &definition).await?;
            if !previous.is_empty() && previous != version && !untyped {
                return Err(invalid("meter definition is immutable for its key"));
            }
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM meters WHERE service_id=$1 AND name<>$2")
                .bind(uuid(&service)?)
                .bind(string(&v, "name"))
                .fetch_one(&mut *self.tx)
                .await?;
        if count >= 100 {
            return Err(invalid("service meter limit reached"));
        }
        sqlx::query("INSERT INTO meters VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(uuid(&service)?)
            .bind(string(&v, "name"))
            .execute(&mut *self.tx)
            .await?;
        Ok(v)
    }
    async fn create_catalog_price(&mut self, mut v: Value) -> Result<Value> {
        let plan = string(&v, "plan_id").to_owned();
        let addon = string(&v, "add_on_id").to_owned();
        let service = string(&v, "service_id").to_owned();
        if plan.is_empty() == addon.is_empty() || string(&v, "currency") != "USD" {
            return Err(invalid("invalid price references or currency"));
        }
        uuid(&service)?;
        let (kind, owner) = if plan.is_empty() {
            ("addon", addon.as_str())
        } else {
            ("plan", plan.as_str())
        };
        uuid(owner)?;
        v["unit_price"] = json!(decimal(string(&v, "unit_price"), true)?);
        v["included"] = json!(decimal(string(&v, "included"), true)?);
        nonempty(string(&v, "meter"), 100)?;
        if number(&v, "kind") == 0 {
            v["kind"] = json!(1);
        }
        let price_kind = number(&v, "kind");
        if !(1..=2).contains(&price_kind) {
            return Err(invalid("unknown price kind"));
        }
        if price_kind == 2 && string(&v, "external_price_id").is_empty() {
            return Err(invalid("recurring price requires Paddle mapping"));
        }
        if price_kind == 1 {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM meters WHERE service_id=$1 AND name=$2)",
            )
            .bind(uuid(&service)?)
            .bind(string(&v, "meter"))
            .fetch_one(&mut *self.tx)
            .await?;
            if !exists {
                return Err(invalid("unregistered meter"));
            }
            // Call-time priced AI usage never uses plan or add-on price versions.
            if crate::delitools::meter_key_is_call_time(self.tx, &service, string(&v, "meter"))
                .await?
            {
                return Err(invalid(
                    "the meter is priced at call time; usage prices do not apply",
                ));
            }
        }
        if !addon.is_empty() {
            let actual: Option<String> =
                sqlx::query_scalar("SELECT service_id::text FROM addons WHERE id=$1")
                    .bind(uuid(&addon)?)
                    .fetch_optional(&mut *self.tx)
                    .await?;
            if actual.as_deref() != Some(&service) {
                return Err(invalid("add-on belongs to another service"));
            }
        }
        let key = if string(&v, "key").is_empty() {
            format!(
                "price-{}",
                hex::encode(
                    &Sha256::digest(
                        format!("{}:{}:{}", service, string(&v, "meter"), price_kind).as_bytes()
                    )[..8]
                )
            )
        } else {
            string(&v, "key").to_owned()
        };
        if !valid_key(&key) {
            return Err(invalid("invalid price key"));
        }
        if !string(&v, "stable_id").is_empty() {
            uuid(string(&v, "stable_id"))?;
        }
        let stable = postgres::ensure_catalog_entity(
            self.tx,
            "price",
            &format!("{kind}:{owner}"),
            &key,
            string(&v, "stable_id"),
        )
        .await?;
        let definition = json!({"service":service,"meter":string(&v,"meter"),"unit_price":string(&v,"unit_price"),"included":string(&v,"included"),"currency":string(&v,"currency"),"external_price":string(&v,"external_price_id"),"kind":price_kind});
        let (version, created) =
            postgres::select_catalog_version(self.tx, &stable, &definition).await?;
        if created {
            sqlx::query("INSERT INTO prices(id,plan_id,service_id,meter,unit_price,included,currency,external_price_id,created_at,addon_id,kind) VALUES($1,$2,$3,$4,$5::text::numeric,$6::text::numeric,$7,$8,$9,$10,$11)").bind(uuid(&version)?).bind(if plan.is_empty(){None}else{Some(uuid(&plan)?)}).bind(uuid(&service)?).bind(string(&v,"meter")).bind(string(&v,"unit_price")).bind(string(&v,"included")).bind(string(&v,"currency")).bind(string(&v,"external_price_id")).bind(self.now).bind(if addon.is_empty(){None}else{Some(uuid(&addon)?)}).bind(price_kind).execute(&mut *self.tx).await?;
        }
        postgres::rebuild_product_version(self.tx, kind, owner).await?;
        v["id"] = json!(version);
        v["stable_id"] = json!(stable);
        v["key"] = json!(key);
        v["active"] = json!(true);
        Ok(v)
    }
    async fn get_catalog(&mut self, q: &Value) -> Result<Value> {
        let (n, after) = catalog_page(&q["page"])?;
        let after = db_uuid(&after)?;
        let queries = [
            (
                "services",
                "SELECT jsonb_build_object('id',s.id,'name',s.name,'client_id',s.client_id,'capabilities',s.capabilities,'scopes',s.scopes,'key',COALESCE(ce.key,'')) FROM services s LEFT JOIN catalog_entities ce ON ce.id=s.id WHERE s.id>$1 ORDER BY s.id LIMIT $2",
            ),
            (
                "plans",
                "SELECT jsonb_build_object('id',p.id,'name',p.name,'rank',p.rank,'free',p.free,'default_free',p.is_default,'postpaid',p.postpaid,'key',COALESCE(ce.key,''),'active_version_id',COALESCE(ce.active_version_id::text,'')) FROM plans p LEFT JOIN catalog_entities ce ON ce.id=p.id WHERE p.id>$1 ORDER BY p.id LIMIT $2",
            ),
            (
                "prices",
                "SELECT jsonb_build_object('id',p.id,'plan_id',COALESCE(p.plan_id::text,''),'service_id',p.service_id,'meter',p.meter,'unit_price',p.unit_price::text,'included',p.included::text,'currency',p.currency,'external_price_id',p.external_price_id,'add_on_id',COALESCE(p.addon_id::text,''),'kind',p.kind,'key',COALESCE(ce.key,''),'stable_id',COALESCE(ce.id::text,''),'active',CASE WHEN ce.id IS NOT NULL THEN COALESCE(ce.active_version_id=p.id,false) ELSE NOT EXISTS (SELECT 1 FROM prices newer WHERE newer.plan_id IS NOT DISTINCT FROM p.plan_id AND newer.addon_id IS NOT DISTINCT FROM p.addon_id AND newer.service_id=p.service_id AND newer.meter=p.meter AND newer.kind=p.kind AND (newer.created_at,newer.id)>(p.created_at,p.id)) END) FROM prices p LEFT JOIN catalog_versions cv ON cv.id=p.id LEFT JOIN catalog_entities ce ON ce.id=cv.entity_id WHERE p.id>$1 ORDER BY p.id LIMIT $2",
            ),
            (
                "add_ons",
                "SELECT jsonb_build_object('id',a.id,'service_id',a.service_id,'name',a.name,'key',COALESCE(ce.key,''),'active_version_id',COALESCE(ce.active_version_id::text,'')) FROM addons a LEFT JOIN catalog_entities ce ON ce.id=a.id WHERE a.id>$1 ORDER BY a.id LIMIT $2",
            ),
            (
                "features",
                "SELECT to_jsonb(f) FROM (SELECT id::text,service_id::text,key,name,kind,value_type,meter_key FROM features WHERE id>$1 ORDER BY id LIMIT $2) f",
            ),
        ];
        let mut out = json!({});
        let mut boundary = String::new();
        for (field, query) in queries {
            let mut values: Vec<Value> = sqlx::query_scalar(query)
                .bind(after)
                .bind(n + 1)
                .fetch_all(&mut *self.tx)
                .await?;
            if field == "services" && !self.identity.operator {
                for v in &mut values {
                    v["client_id"] = json!("");
                    v["scopes"] = json!([]);
                }
            }
            if values.len() > n as usize {
                let last = string(&values[n as usize - 1], "id");
                if boundary.is_empty() || last < boundary.as_str() {
                    boundary = last.into();
                }
            }
            out[field] = json!(values);
        }
        if !boundary.is_empty() {
            for (field, _) in queries {
                out[field]
                    .as_array_mut()
                    .unwrap()
                    .retain(|v| string(v, "id") <= boundary.as_str());
            }
            out["next_after_id"] = json!(boundary);
        }
        let mut meters = Vec::new();
        for service in array(&out, "services") {
            let rows=sqlx::query("SELECT m.service_id::text,m.name,COALESCE(ce.id::text,'') AS id,COALESCE(ce.key,'') AS key,COALESCE(cv.definition,'{}'::jsonb) AS definition FROM meters m LEFT JOIN catalog_entities ce ON ce.kind='meter' AND ce.scope=m.service_id::text AND EXISTS (SELECT 1 FROM catalog_versions cv WHERE cv.id=ce.active_version_id AND cv.definition->>'name'=m.name) LEFT JOIN catalog_versions cv ON cv.id=ce.active_version_id WHERE m.service_id=$1 ORDER BY m.name").bind(uuid(string(&service,"id"))?).fetch_all(&mut *self.tx).await?;
            for row in rows {
                let mut v = json!({"service_id":row.get::<String,_>("service_id"),"name":row.get::<String,_>("name"),"id":row.get::<String,_>("id"),"key":row.get::<String,_>("key")});
                let definition: Value = row.get("definition");
                for (k, value) in definition.as_object().into_iter().flatten() {
                    v[k] = value.clone();
                }
                meters.push(v);
            }
        }
        out["meters"] = json!(meters);
        for field in ["plans", "add_ons"] {
            let values = out[field].as_array_mut().unwrap();
            for v in values {
                v["feature_values"] = json!(
                    self.catalog_feature_values(string(v, "active_version_id"))
                        .await?
                );
            }
        }
        Ok(out)
    }
    async fn catalog_groups(&mut self, op: Op, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenant_id");
        if op == Op::ListGroups {
            self.member(tenant).await?;
            let (n, after) = catalog_page(&q["page"])?;
            let rows=sqlx::query("SELECT id::text,name FROM groups WHERE tenant_id=$1 AND id>$2 ORDER BY id LIMIT $3").bind(uuid(tenant)?).bind(db_uuid(&after)?).bind(n+1).fetch_all(&mut *self.tx).await?;
            let mut groups: Vec<Value> = rows
                .iter()
                .map(|r| json!({"id":r.get::<String,_>("id"),"name":r.get::<String,_>("name")}))
                .collect();
            let mut next = String::new();
            if groups.len() > n as usize {
                groups.truncate(n as usize);
                next = string(groups.last().unwrap(), "id").into();
            }
            for g in &mut groups {
                let gid = uuid(string(g, "id"))?;
                let users: Vec<String> = sqlx::query_scalar(
                    "SELECT user_id::text FROM group_members WHERE group_id=$1 ORDER BY user_id",
                )
                .bind(gid)
                .fetch_all(&mut *self.tx)
                .await?;
                let permissions:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('service_id',service_id,'capability',capability) FROM group_permissions WHERE group_id=$1 ORDER BY service_id,capability").bind(gid).fetch_all(&mut *self.tx).await?;
                g["user_ids"] = json!(users);
                g["permissions"] = json!(permissions);
            }
            return Ok(json!({"groups":groups,"next_after_id":next}));
        }
        self.require(tenant, &[2]).await?;
        if op == Op::DeleteGroup {
            let gid = uuid(string(q, "group_id"))?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM groups WHERE id=$1 AND tenant_id=$2)",
            )
            .bind(gid)
            .bind(uuid(tenant)?)
            .fetch_one(&mut *self.tx)
            .await?;
            if !exists {
                return Err(forbidden());
            }
            for query in [
                "DELETE FROM group_members WHERE group_id=$1",
                "DELETE FROM group_permissions WHERE group_id=$1",
                "DELETE FROM groups WHERE id=$1",
            ] {
                sqlx::query(query).bind(gid).execute(&mut *self.tx).await?;
            }
            self.audit(tenant, "group.delete", "").await?;
            return Ok(json!({}));
        }
        let row = sqlx::query(
            "SELECT personal,(deleting_at IS NOT NULL) AS deleting FROM tenants WHERE id=$1",
        )
        .bind(uuid(tenant)?)
        .fetch_one(&mut *self.tx)
        .await?;
        if row.get::<bool, _>("personal") || row.get::<bool, _>("deleting") {
            return Err(precondition("groups require an active organization"));
        }
        let mut g = q["group"].clone();
        if !g.is_object() {
            return Err(invalid("group required"));
        }
        nonempty(string(&g, "name"), 200)?;
        let users = strings(&g, "user_ids");
        let permissions = array(&g, "permissions");
        if users.len() > 1000 || permissions.len() > 1000 {
            return Err(invalid("group is too large"));
        }
        if string(&g, "id").is_empty() {
            g["id"] = json!(id());
            sqlx::query("INSERT INTO groups VALUES($1,$2,$3)")
                .bind(uuid(string(&g, "id"))?)
                .bind(uuid(tenant)?)
                .bind(string(&g, "name"))
                .execute(&mut *self.tx)
                .await?;
        } else {
            let tag = sqlx::query("UPDATE groups SET name=$3 WHERE id=$1 AND tenant_id=$2")
                .bind(uuid(string(&g, "id"))?)
                .bind(uuid(tenant)?)
                .bind(string(&g, "name"))
                .execute(&mut *self.tx)
                .await?;
            if tag.rows_affected() != 1 {
                return Err(forbidden());
            }
        }
        let gid = uuid(string(&g, "id"))?;
        for query in [
            "DELETE FROM group_members WHERE group_id=$1",
            "DELETE FROM group_permissions WHERE group_id=$1",
        ] {
            sqlx::query(query).bind(gid).execute(&mut *self.tx).await?;
        }
        for user in users {
            sqlx::query("INSERT INTO group_members VALUES($1,$2,$3)")
                .bind(uuid(tenant)?)
                .bind(gid)
                .bind(uuid(&user)?)
                .execute(&mut *self.tx)
                .await
                .map_err(|_| invalid("group contains a non-member or duplicate"))?;
        }
        for p in permissions {
            let service = uuid(string(&p, "service_id"))?;
            let capability = string(&p, "capability");
            let ok: Option<bool> =
                sqlx::query_scalar("SELECT $2=ANY(capabilities) FROM services WHERE id=$1")
                    .bind(service)
                    .bind(capability)
                    .fetch_optional(&mut *self.tx)
                    .await?;
            if ok != Some(true) {
                return Err(invalid("unregistered capability"));
            }
            sqlx::query("INSERT INTO group_permissions VALUES($1,$2,$3)")
                .bind(gid)
                .bind(service)
                .bind(capability)
                .execute(&mut *self.tx)
                .await
                .map_err(|_| invalid("duplicate permission"))?;
        }
        self.audit(tenant, "group.set", "").await?;
        Ok(json!({"group":g}))
    }
    async fn catalog_billing_history(&mut self, q: &Value) -> Result<Value> {
        let tenant = string(q, "tenant_id");
        self.require(tenant, &[3]).await?;
        let (n, after) = catalog_page(&q["page"])?;
        let rows=sqlx::query("SELECT id::text,kind,state,error_category,attempts,checkout_url FROM jobs WHERE tenant_id=$1 AND kind NOT IN ('email','metering') AND id>$2 ORDER BY id LIMIT $3").bind(uuid(tenant)?).bind(db_uuid(&after)?).bind(n+1).fetch_all(&mut *self.tx).await?;
        let mut jobs:Vec<Value>=rows.iter().map(|r| {let state=match r.get::<&str,_>("state"){"pending"=>1,"running"=>2,"reconcile"=>3,"done"=>4,"failed"=>5,_=>0};json!({"id":r.get::<String,_>("id"),"kind":r.get::<String,_>("kind"),"state":state,"error_category":r.get::<String,_>("error_category"),"attempts":r.get::<i32,_>("attempts"),"checkout_url":r.get::<String,_>("checkout_url")})}).collect();
        let mut next = String::new();
        if jobs.len() > n as usize {
            jobs.truncate(n as usize);
            next = string(jobs.last().unwrap(), "id").into();
        }
        Ok(json!({"jobs":jobs,"next_after_id":next}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_keys_preserve_names_and_disambiguate_unicode() {
        assert_eq!(derived_key("Basic Plan"), "basic-plan");
        assert!(valid_key(&derived_key("기본 요금제")));
        assert_ne!(derived_key("기본 요금제"), derived_key("다른 요금제"));
        assert!(valid_key(&derived_key(&"x".repeat(200))));
    }
    #[test]
    fn feature_configuration_is_strict() {
        assert!(configuration(&json!({"boolean":false}), 1).is_ok());
        assert!(configuration(&json!({"boolean":false,"text":"x"}), 1).is_err());
        assert!(configuration(&json!({"text":"x"}), 1).is_err());
        assert!(configuration(&json!({"string_list":{"values":["a"]}}), 5).is_ok());
    }
    #[test]
    fn protobuf_json_catalog_boundary_preserves_typed_values() {
        use delino_protocol::delcore::v1::{GetCatalogResponse, PutFeatureRequest};
        let request: PutFeatureRequest = serde_json::from_value(json!({"reason":"change catalog","feature":{"serviceId":"01900000-0000-7000-8000-000000000001","key":"access","name":"Access","kind":"FEATURE_KIND_CONFIGURATION","valueType":"CONFIGURATION_VALUE_TYPE_BOOLEAN"}})).unwrap();
        let normalized = field_names(&serde_json::to_value(request).unwrap(), false);
        assert_eq!(number(&normalized["feature"], "kind"), 3);
        assert_eq!(number(&normalized["feature"], "value_type"), 1);
        assert_eq!(
            string(&normalized["feature"], "service_id"),
            "01900000-0000-7000-8000-000000000001"
        );
        let response = field_names(
            &json!({"plans":[{"id":"01900000-0000-7000-8000-000000000001","feature_values":[{"feature_id":"01900000-0000-7000-8000-000000000002","configuration":{"string_list":{"values":["alpha"]}}}]}],"features":[{"kind":3,"value_type":5}]}),
            true,
        );
        let response: GetCatalogResponse = serde_json::from_value(response).unwrap();
        assert_eq!(response.plans[0].feature_values.len(), 1);
        assert_eq!(response.features.len(), 1);
    }
    #[test]
    fn protobuf_empty_string_list_grants_survive_plan_and_addon_requests() {
        use delino_protocol::delcore::v1::{PutAddOnRequest, PutPlanRequest};
        let feature = json!({"featureId":"01900000-0000-7000-8000-000000000002","configuration":{"stringList":{"values":[]}}});
        let plan: PutPlanRequest =
            serde_json::from_value(json!({"plan":{"featureValues":[feature.clone()]}})).unwrap();
        let addon: PutAddOnRequest =
            serde_json::from_value(json!({"addOn":{"featureValues":[feature]}})).unwrap();
        for (wire, product) in [
            (serde_json::to_value(plan).unwrap(), "plan"),
            (serde_json::to_value(addon).unwrap(), "add_on"),
        ] {
            let normalized = field_names(&wire, false);
            let grant = &normalized[product]["feature_values"][0]["configuration"];
            assert_eq!(grant, &json!({"string_list":{}}));
            assert_eq!(configuration(grant, 5).unwrap(), json!({"string_list":[]}));
        }
        assert!(configuration(&json!({"string_list":false}), 5).is_err());
        assert!(configuration(&json!({"string_list":{"values":false}}), 5).is_err());
    }
    #[test]
    fn legacy_null_string_list_keeps_configuration_oneof() {
        use delino_protocol::delcore::v1::ConfigurationValue;
        for stored in [json!({"string_list":null}), json!({"string_list":[]})] {
            let configuration = stored_configuration(stored);
            assert_eq!(configuration, json!({"string_list":{"values":[]}}));
            let decoded: ConfigurationValue =
                serde_json::from_value(field_names(&configuration, true)).unwrap();
            assert_eq!(
                serde_json::to_value(decoded).unwrap(),
                json!({"stringList":{}})
            );
        }
    }
    #[test]
    fn decimal_and_identity_validation_match_catalog_contract() {
        assert_eq!(decimal("1.200", true).unwrap(), "1.2");
        for invalid in ["1e3", "+1", "01", "-0.1"] {
            assert!(decimal(invalid, true).is_err(), "{invalid}");
        }
        assert!(uuid("550e8400-e29b-41d4-a716-446655440000").is_err());
        assert!(uuid("01900000-0000-7000-8000-000000000001").is_ok());
    }
}
