// SPDX-License-Identifier: FSL-1.1-ALv2
//! LiteLLM price sync: fetch, validate, normalize and version public per-token prices.
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

pub const MAX_DOCUMENT_BYTES: usize = 20 << 20;
pub const MIN_MODELS: usize = 100;
const FIELDS: [&str; 4] = [
    "input_cost_per_token",
    "output_cost_per_token",
    "cache_read_input_token_cost",
    "cache_creation_input_token_cost",
];

/// Stored outcome codes of `price_sync_runs.outcome`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Created = 1,
    Unchanged = 2,
    FetchFailed = 3,
    ValidationFailed = 4,
}

pub struct Fetcher {
    http: reqwest::Client,
    pub url: String,
}

impl Fetcher {
    pub fn new(url: &str) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()?,
            url: url.into(),
        })
    }

    async fn fetch(&self) -> Result<Vec<u8>, &'static str> {
        let mut response = self.http.get(&self.url).send().await.map_err(|_| "fetch")?;
        if !response.status().is_success() {
            return Err("status");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "fetch")? {
            if body.len() + chunk.len() > MAX_DOCUMENT_BYTES {
                return Err("too_large");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// One normalized model price entry; prices are per token in USD.
pub type Entry = (String, [Option<f64>; 4]);

/// Validates the whole document. Any violation rejects it.
pub fn validate(bytes: &[u8]) -> Result<Vec<Entry>, &'static str> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("too_large");
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "invalid_json")?;
    let object = value.as_object().ok_or("not_object")?;
    if object.len() < MIN_MODELS {
        return Err("too_few_models");
    }
    let mut out = Vec::with_capacity(object.len());
    for (model, entry) in object {
        let entry = entry.as_object().ok_or("invalid_entry")?;
        if model.is_empty() || model.len() > 400 {
            return Err("invalid_model_key");
        }
        let mut prices = [None; 4];
        for (i, field) in FIELDS.iter().enumerate() {
            match entry.get(*field) {
                None | Some(Value::Null) => {}
                Some(v) => {
                    let n = v.as_f64().ok_or("invalid_price")?;
                    if !n.is_finite() || n < 0.0 {
                        return Err("invalid_price");
                    }
                    prices[i] = Some(n);
                }
            }
        }
        out.push((model.clone(), prices));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Decimal text of a price without exponent notation.
pub fn price_text(n: f64) -> String {
    format!("{n}")
}

/// SHA-256 of the normalized price set.
pub fn digest(entries: &[Entry]) -> String {
    let mut text = String::new();
    for (model, prices) in entries {
        text.push_str(model);
        for p in prices {
            text.push('\t');
            if let Some(p) = p {
                text.push_str(&price_text(*p));
            }
        }
        text.push('\n');
    }
    crate::sha256(text.as_bytes())
}

/// Runs one sync. A failed fetch or validation keeps the previous version and records the
/// failure; a new version is created only when the normalized prices change.
pub async fn sync(
    db: &PgPool,
    fetcher: &Fetcher,
    metrics: &crate::metrics::Metrics,
    now: DateTime<Utc>,
) -> anyhow::Result<Outcome> {
    let run = crate::new_id();
    sqlx::query("INSERT INTO price_sync_runs(id,started_at,outcome) VALUES($1,$2,0)")
        .bind(run)
        .bind(now)
        .execute(db)
        .await?;
    let finish = |outcome: Outcome, category: &'static str, version: Option<Uuid>| async move {
        sqlx::query(
            "UPDATE price_sync_runs SET finished_at=$2,outcome=$3,failure_category=$4,price_version_id=$5 WHERE id=$1",
        )
        .bind(run)
        .bind(crate::time::now())
        .bind(outcome as i16)
        .bind(category)
        .bind(version)
        .execute(db)
        .await
    };
    let bytes = match fetcher.fetch().await {
        Ok(b) => b,
        Err(category) => {
            tracing::warn!(
                category,
                "LiteLLM price fetch failed; the previous version stays in use"
            );
            finish(Outcome::FetchFailed, category, None).await?;
            return Ok(Outcome::FetchFailed);
        }
    };
    let entries = match validate(&bytes) {
        Ok(e) => e,
        Err(category) => {
            metrics.price_validation_failures.inc();
            tracing::warn!(
                category,
                "LiteLLM price document rejected; the previous version stays in use"
            );
            finish(Outcome::ValidationFailed, category, None).await?;
            return Ok(Outcome::ValidationFailed);
        }
    };
    let hash = digest(&entries);
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('anydash_price_versions',0))")
        .execute(&mut *tx)
        .await?;
    let current: Option<String> = sqlx::query_scalar(
        "SELECT content_sha256 FROM price_versions ORDER BY effective_from DESC LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if current.as_deref() == Some(hash.as_str()) {
        tx.commit().await?;
        finish(Outcome::Unchanged, "", None).await?;
        tracing::info!(models = entries.len(), "LiteLLM prices unchanged");
        return Ok(Outcome::Unchanged);
    }
    let version = crate::new_id();
    sqlx::query(
        "INSERT INTO price_versions(id,effective_from,content_sha256,source_url,fetched_at,model_count) VALUES($1,$2,$3,$4,$2,$5)",
    )
    .bind(version)
    .bind(now)
    .bind(&hash)
    .bind(&fetcher.url)
    .bind(entries.len() as i32)
    .execute(&mut *tx)
    .await?;
    for chunk in entries.chunks(1000) {
        let column = |i: usize| -> Vec<Option<String>> {
            chunk.iter().map(|(_, p)| p[i].map(price_text)).collect()
        };
        sqlx::query(
            "INSERT INTO model_prices(price_version_id,model,input_cost_per_token,output_cost_per_token,cache_read_cost_per_token,cache_write_cost_per_token) \
             SELECT $1,m,i::numeric,o::numeric,r::numeric,w::numeric FROM UNNEST($2::text[],$3::text[],$4::text[],$5::text[],$6::text[]) AS t(m,i,o,r,w)",
        )
        .bind(version)
        .bind(chunk.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>())
        .bind(column(0))
        .bind(column(1))
        .bind(column(2))
        .bind(column(3))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    finish(Outcome::Created, "", Some(version)).await?;
    tracing::info!(models = entries.len(), price_version_id = %version, "LiteLLM price version created");
    Ok(Outcome::Created)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::json;
    /// A valid synthetic LiteLLM document with `n` models.
    pub fn document(n: usize, input: f64) -> Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "gpt-5".into(),
            json!({"input_cost_per_token":input,"output_cost_per_token":0.00001,"cache_read_input_token_cost":0.000000125}),
        );
        map.insert(
            "anthropic/claude-opus-4".into(),
            json!({"input_cost_per_token":0.000015,"output_cost_per_token":0.000075,"cache_read_input_token_cost":0.0000015,"cache_creation_input_token_cost":0.00001875}),
        );
        for i in 0..n.saturating_sub(2) {
            map.insert(
                format!("model-{i}"),
                json!({"input_cost_per_token":0.000001,"mode":"chat"}),
            );
        }
        Value::Object(map)
    }
    #[test]
    fn validation_rejects_the_whole_document_on_any_violation() {
        let good = document(120, 0.00000125);
        let entries = validate(good.to_string().as_bytes()).unwrap();
        assert_eq!(entries.len(), 120);
        assert_eq!(
            validate(document(99, 1e-6).to_string().as_bytes()),
            Err("too_few_models")
        );
        assert_eq!(validate(b"{"), Err("invalid_json"));
        assert_eq!(validate(b"[]"), Err("not_object"));
        let mut bad = good.clone();
        bad["gpt-5"]["output_cost_per_token"] = json!(-1);
        assert_eq!(validate(bad.to_string().as_bytes()), Err("invalid_price"));
        let mut bad = good.clone();
        bad["gpt-5"]["cache_read_input_token_cost"] = json!("0.1");
        assert_eq!(validate(bad.to_string().as_bytes()), Err("invalid_price"));
        let mut bad = good.clone();
        bad["model-1"] = json!(5);
        assert_eq!(validate(bad.to_string().as_bytes()), Err("invalid_entry"));
        // Hashes depend only on the normalized prices, not on key order or other fields.
        let mut reordered = good.clone();
        reordered["model-3"]["mode"] = json!("embedding");
        assert_eq!(
            digest(&validate(reordered.to_string().as_bytes()).unwrap()),
            digest(&entries)
        );
        assert_ne!(
            digest(&validate(document(120, 0.000002).to_string().as_bytes()).unwrap()),
            digest(&entries)
        );
        assert_eq!(price_text(0.000000125), "0.000000125");
    }
}
