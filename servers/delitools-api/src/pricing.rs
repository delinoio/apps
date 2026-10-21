// SPDX-License-Identifier: FSL-1.1-ALv2
//! Frozen catalog price snapshots and the delcore usage-event shape.
//!
//! DeliTools never computes a charge. It reports measured token counts together with the frozen
//! public catalog price of the request; delcore multiplies them and applies the 1.15 or 1.30
//! markup with its own exact decimal arithmetic and rounding.
use crate::gateway::Tokens;
use chrono::{DateTime, SecondsFormat, Utc};
use delino_protocol::delitools::v1::Capability;
use serde_json::{Map, Value, json};
use std::time::Duration;
use uuid::Uuid;

/// Catalog price of one model route, effective from `effective_at`. A category without a
/// published price is `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PriceSnapshot {
    pub id: Uuid,
    pub model: String,
    pub route: String,
    pub effective_at: DateTime<Utc>,
    pub input: String,
    pub output: String,
    pub cache_read: Option<String>,
    pub cache_write: Option<String>,
}

/// The four token categories with their snapshot and event attribute names, in report order.
const CATEGORIES: [(&str, &str, &str); 4] = [
    ("input", "input_tokens", "input_usd_per_token"),
    ("output", "output_tokens", "output_usd_per_token"),
    (
        "cache_read",
        "cache_read_tokens",
        "cache_read_usd_per_token",
    ),
    (
        "cache_write",
        "cache_write_tokens",
        "cache_write_usd_per_token",
    ),
];

impl PriceSnapshot {
    /// The JSON stored on every attempt of the request. Unpublished categories are omitted.
    pub fn snapshot_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("input_usd_per_token".into(), json!(self.input));
        object.insert("output_usd_per_token".into(), json!(self.output));
        if let Some(price) = &self.cache_read {
            object.insert("cache_read_usd_per_token".into(), json!(price));
        }
        if let Some(price) = &self.cache_write {
            object.insert("cache_write_usd_per_token".into(), json!(price));
        }
        Value::Object(object)
    }
}

pub fn tokens_json(tokens: &Tokens) -> Value {
    json!({
        "input_tokens": tokens.input,
        "output_tokens": tokens.output,
        "cache_read_tokens": tokens.cache_read,
        "cache_write_tokens": tokens.cache_write,
    })
}

pub fn tokens_from_json(value: &Value) -> Option<Tokens> {
    Some(Tokens {
        input: value["input_tokens"].as_u64()?,
        output: value["output_tokens"].as_u64()?,
        cache_read: value["cache_read_tokens"].as_u64()?,
        cache_write: value["cache_write_tokens"].as_u64()?,
    })
}

/// Usage needs a category price that the frozen snapshot does not publish. The attempt stays
/// pending; usage is never estimated or defaulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingCategoryPrice;

/// Flat delcore event attributes (`AttributeValue` JSON) for one measured attempt.
///
/// A category with a published price is reported with its count (zero included). A category
/// without a price is omitted together with its count when its count is zero; a nonzero count
/// without a price cannot be reported.
pub fn event_attributes(
    model: &str,
    price_effective_at: DateTime<Utc>,
    tokens: &Tokens,
    snapshot: &Value,
) -> Result<Map<String, Value>, MissingCategoryPrice> {
    let mut attributes = Map::new();
    attributes.insert("model".into(), json!({"text": model}));
    attributes.insert(
        "price_effective_at".into(),
        json!({"text": price_effective_at.to_rfc3339_opts(SecondsFormat::Micros, true)}),
    );
    let counts = [
        tokens.input,
        tokens.output,
        tokens.cache_read,
        tokens.cache_write,
    ];
    for ((_, token_name, price_name), count) in CATEGORIES.iter().zip(counts) {
        match snapshot[price_name].as_str() {
            Some(price) => {
                attributes.insert((*token_name).into(), json!({"number": count.to_string()}));
                attributes.insert((*price_name).into(), json!({"number": price}));
            }
            None if count == 0 => {}
            None => return Err(MissingCategoryPrice),
        }
    }
    Ok(attributes)
}

/// Delcore event type per capability.
pub fn event_type(capability: Capability) -> &'static str {
    match capability {
        Capability::CAPABILITY_EXTRACTOR => "extractor.usage",
        _ => "translator.usage",
    }
}

/// RFC 3339 with microseconds, the timestamp precision DeliTools and delcore compare at.
pub fn rfc3339(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Capped exponential delay for background usage delivery and reconciliation: the initial delay
/// doubles with each failure up to the configured maximum.
pub fn backoff(initial_seconds: u32, max_seconds: u32, failures: u32) -> Duration {
    let initial = u64::from(initial_seconds);
    let max = u64::from(max_seconds);
    let factor = 1_u64.checked_shl(failures.min(40)).unwrap_or(u64::MAX);
    Duration::from_secs(
        initial
            .saturating_mul(factor)
            .min(max)
            .max(initial.min(max)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(read: Option<&str>, write: Option<&str>) -> PriceSnapshot {
        PriceSnapshot {
            id: Uuid::now_v7(),
            model: "openai/gpt-6-luna".into(),
            route: "openai".into(),
            effective_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            input: "0.0000001".into(),
            output: "0.0000005".into(),
            cache_read: read.map(Into::into),
            cache_write: write.map(Into::into),
        }
    }
    const TOKENS: Tokens = Tokens {
        input: 1000,
        output: 200,
        cache_read: 50,
        cache_write: 10,
    };

    #[test]
    fn full_snapshot_reports_all_four_categories_without_any_charge() {
        let price = snapshot(Some("0.00000001"), Some("0.000000125"));
        let attributes = event_attributes(
            "openai/gpt-6-luna",
            price.effective_at,
            &TOKENS,
            &price.snapshot_json(),
        )
        .unwrap();
        let expected = json!({
            "model": {"text": "openai/gpt-6-luna"},
            "price_effective_at": {"text": "2027-01-15T08:00:00.000000Z"},
            "input_tokens": {"number": "1000"},
            "input_usd_per_token": {"number": "0.0000001"},
            "output_tokens": {"number": "200"},
            "output_usd_per_token": {"number": "0.0000005"},
            "cache_read_tokens": {"number": "50"},
            "cache_read_usd_per_token": {"number": "0.00000001"},
            "cache_write_tokens": {"number": "10"},
            "cache_write_usd_per_token": {"number": "0.000000125"},
        });
        assert_eq!(Value::Object(attributes), expected);
    }

    #[test]
    fn nothing_but_raw_tokens_and_prices_is_reported() {
        let price = snapshot(Some("0.00000001"), Some("0.000000125"));
        let attributes =
            event_attributes("m", price.effective_at, &TOKENS, &price.snapshot_json()).unwrap();
        for name in attributes.keys() {
            assert!(
                ![
                    "charge", "markup", "amount", "currency", "fee", "cost", "usd"
                ]
                .contains(&name.as_str()),
                "{name}"
            );
        }
        assert_eq!(attributes.len(), 10);
    }

    #[test]
    fn unpublished_categories_are_omitted_with_their_zero_counts() {
        let price = snapshot(None, None);
        let zero = Tokens {
            cache_read: 0,
            cache_write: 0,
            ..TOKENS
        };
        let attributes =
            event_attributes("m", price.effective_at, &zero, &price.snapshot_json()).unwrap();
        assert!(!attributes.contains_key("cache_read_tokens"));
        assert!(!attributes.contains_key("cache_write_usd_per_token"));
        assert_eq!(attributes.len(), 6);
    }

    #[test]
    fn nonzero_usage_without_a_published_price_stays_unreportable() {
        let price = snapshot(None, Some("0.000000125"));
        assert_eq!(
            event_attributes("m", price.effective_at, &TOKENS, &price.snapshot_json()),
            Err(MissingCategoryPrice)
        );
    }

    #[test]
    fn token_counts_are_exact_for_very_large_values() {
        let price = snapshot(Some("0.00000001"), None);
        let tokens = Tokens {
            input: u64::MAX,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        };
        let attributes =
            event_attributes("m", price.effective_at, &tokens, &price.snapshot_json()).unwrap();
        assert_eq!(attributes["input_tokens"]["number"], "18446744073709551615");
        assert_eq!(attributes["output_tokens"]["number"], "0");
    }

    #[test]
    fn markup_inputs_reproduce_the_documented_charge_exactly() {
        // The charge is computed by delcore: (sum tokens*price) * markup. Reproduce it here with
        // integer arithmetic (prices scaled by 1e15) so the report is demonstrably sufficient
        // input for both markups.
        let price = snapshot(Some("0.00000001"), Some("0.000000125"));
        let attributes =
            event_attributes("m", price.effective_at, &TOKENS, &price.snapshot_json()).unwrap();
        let scaled = |price: &str| -> u128 {
            // 15 fractional digits keeps every published price exact.
            let (whole, fraction) = price.split_once('.').unwrap_or((price, ""));
            format!("{whole}{fraction:0<15}").parse().unwrap()
        };
        let mut basis: u128 = 0;
        for (_, tokens, price_name) in CATEGORIES {
            let count: u128 = attributes[tokens]["number"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            basis += count * scaled(attributes[price_name]["number"].as_str().unwrap());
        }
        // 1000*1e-7 + 200*5e-7 + 50*1e-8 + 10*1.25e-7 = 2.0175e-4 USD, i.e. 2.0175e11 at 1e-15.
        assert_eq!(basis, 201_750_000_000);
        // markups as integer percent: translator 115, extractor 130
        assert_eq!(basis * 115 / 100, 232_012_500_000);
        assert_eq!(basis * 130 / 100, 262_275_000_000);
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let secs = |failures| backoff(30, 3600, failures).as_secs();
        assert_eq!(secs(0), 30);
        assert_eq!(secs(1), 60);
        assert_eq!(secs(2), 120);
        assert_eq!(secs(6), 1920);
        assert_eq!(secs(7), 3600);
        assert_eq!(secs(40), 3600);
        assert_eq!(secs(u32::MAX), 3600);
        // The maximum never drops below the cap, and equal initial and maximum stay constant.
        assert_eq!(backoff(10, 10, 5).as_secs(), 10);
        assert_eq!(backoff(1, 1, 0).as_secs(), 1);
    }

    #[test]
    fn event_types_follow_the_capability() {
        assert_eq!(
            event_type(Capability::CAPABILITY_TRANSLATOR),
            "translator.usage"
        );
        assert_eq!(
            event_type(Capability::CAPABILITY_EXTRACTOR),
            "extractor.usage"
        );
    }
}
