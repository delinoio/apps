// SPDX-License-Identifier: FSL-1.1-ALv2

//! Exact USD charge of one AI usage report. The reporter supplies measured token
//! counts and the frozen public USD price per token of the same request; delcore
//! owns the arithmetic and the markup. Everything here is exact decimal math.
use super::{MeteringError, Result, Value, ValueKind};
use crate::accounting::Decimal;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

/// Token categories in their canonical order. Each pairs the count attribute with
/// the attribute that carries the frozen USD price of one token.
pub const TOKEN_CATEGORIES: [TokenCategory; 4] = [
    TokenCategory {
        tokens: "input_tokens",
        price: "input_usd_per_token",
    },
    TokenCategory {
        tokens: "output_tokens",
        price: "output_usd_per_token",
    },
    TokenCategory {
        tokens: "cache_read_tokens",
        price: "cache_read_usd_per_token",
    },
    TokenCategory {
        tokens: "cache_write_tokens",
        price: "cache_write_usd_per_token",
    },
];
/// Text attribute naming the Gateway model of the report.
pub const MODEL_ATTRIBUTE: &str = "model";
/// Text attribute holding the RFC 3339 time the frozen price became effective.
pub const PRICE_EFFECTIVE_ATTRIBUTE: &str = "price_effective_at";
const MAX_MODEL_BYTES: usize = 200;

#[derive(Clone, Copy, Debug)]
pub struct TokenCategory {
    pub tokens: &'static str,
    pub price: &'static str,
}

fn invalid(message: impl Into<String>) -> MeteringError {
    MeteringError::Event(message.into())
}
fn number(attributes: &BTreeMap<String, Value>, name: &str) -> Result<Option<Decimal>> {
    let Some(value) = attributes.get(name) else {
        return Ok(None);
    };
    if value.kind != ValueKind::Number {
        return Err(invalid(format!("attribute {name:?} must be a number")));
    }
    let d = value.decimal()?;
    if d < Decimal::zero() {
        return Err(invalid(format!("attribute {name:?} must not be negative")));
    }
    Ok(Some(d))
}

/// Reject a noncanonical spelling of a numeric AI evidence attribute. `Value::number`
/// rewrites its input to the canonical decimal, so this must see the text exactly as
/// the reporter wrote it: "1.0" and "0.0100" are malformed evidence, not "1" and
/// "0.01". Attributes that are not AI token or price evidence are not checked.
pub fn check_numeric_spelling(name: &str, text: &str) -> Result<()> {
    let evidence = TOKEN_CATEGORIES
        .iter()
        .any(|category| category.tokens == name || category.price == name);
    if evidence && Value::number(text)?.text != text {
        return Err(invalid(format!(
            "attribute {name:?} must be a canonical decimal"
        )));
    }
    Ok(())
}

/// The time at which the frozen price became effective, validated from the
/// `price_effective_at` text attribute.
pub fn price_effective_at(attributes: &BTreeMap<String, Value>) -> Result<DateTime<Utc>> {
    let value = attributes
        .get(PRICE_EFFECTIVE_ATTRIBUTE)
        .filter(|v| v.kind == ValueKind::String)
        .ok_or_else(|| {
            invalid(format!(
                "attribute {PRICE_EFFECTIVE_ATTRIBUTE:?} is required"
            ))
        })?;
    DateTime::parse_from_rfc3339(&value.text)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|_| {
            invalid(format!(
                "attribute {PRICE_EFFECTIVE_ATTRIBUTE:?} must be an RFC 3339 time"
            ))
        })
}

/// A replacement report keeps the per-token prices frozen by the accepted report:
/// a price present in both must be identical. A category the accepted report did
/// not price may gain a price, and a price may be omitted when the replacement
/// reports no tokens for the category (the token rules reject any other omission).
pub fn frozen_prices_unchanged(
    accepted: &BTreeMap<String, Value>,
    replacement: &BTreeMap<String, Value>,
) -> Result<()> {
    for category in TOKEN_CATEGORIES {
        if let (Some(before), Some(after)) = (
            accepted.get(category.price),
            replacement.get(category.price),
        ) && before != after
        {
            return Err(invalid(format!(
                "attribute {:?} must keep the price of the accepted report",
                category.price
            )));
        }
    }
    Ok(())
}

/// Sum of tokens times USD per token over the four token categories, before any
/// markup. A category without tokens contributes nothing even when it carries no
/// price. A nonzero token count without a price is rejected: usage is never
/// estimated. Token counts must be canonical non-negative integers.
pub fn token_basis(attributes: &BTreeMap<String, Value>) -> Result<Decimal> {
    let model = attributes
        .get(MODEL_ATTRIBUTE)
        .filter(|v| v.kind == ValueKind::String && !v.text.is_empty());
    if model.is_none_or(|v| v.text.len() > MAX_MODEL_BYTES) {
        return Err(invalid(format!(
            "attribute {MODEL_ATTRIBUTE:?} must be text of 1 to {MAX_MODEL_BYTES} bytes"
        )));
    }
    price_effective_at(attributes)?;
    let mut basis = Decimal::zero();
    for category in TOKEN_CATEGORIES {
        let tokens = number(attributes, category.tokens)?.unwrap_or_default();
        if attributes
            .get(category.tokens)
            .is_some_and(|v| v.text.contains('.'))
        {
            return Err(invalid(format!(
                "attribute {:?} must be a whole number",
                category.tokens
            )));
        }
        let price = number(attributes, category.price)?;
        if tokens.is_zero() {
            continue;
        }
        let price = price.ok_or_else(|| {
            invalid(format!(
                "attribute {:?} is required for a nonzero {:?}",
                category.price, category.tokens
            ))
        })?;
        basis = basis.add(&tokens.mul(&price));
    }
    Ok(basis)
}

/// Exact USD charge of one report: the token basis multiplied by the meter
/// markup. Neither step rounds; period rounding happens only in rating.
pub fn usd_charge(attributes: &BTreeMap<String, Value>, markup: &Decimal) -> Result<Decimal> {
    Ok(token_basis(attributes)?.mul(markup))
}

/// Parse a catalog markup factor: a canonical decimal of at least one.
pub fn parse_markup(text: &str) -> Result<Decimal> {
    let markup: Decimal = text
        .parse()
        .map_err(|_| MeteringError::Definition("markup must be a decimal".into()))?;
    if text.len() > 32 || markup < Decimal::parse("1").expect("one is a decimal") {
        return Err(MeteringError::Definition(
            "markup must be a decimal of at least 1 with at most 32 characters".into(),
        ));
    }
    if markup.to_string() != text {
        return Err(MeteringError::Definition(
            "markup must be a canonical decimal".into(),
        ));
    }
    Ok(markup)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }
    fn report(tokens: &[(&str, &str)], prices: &[(&str, &str)]) -> BTreeMap<String, Value> {
        let mut attributes = BTreeMap::from([
            (MODEL_ATTRIBUTE.into(), Value::string("vendor/model")),
            (
                PRICE_EFFECTIVE_ATTRIBUTE.into(),
                Value::string("2026-01-01T00:00:00Z"),
            ),
        ]);
        for (name, value) in tokens.iter().chain(prices) {
            attributes.insert((*name).into(), Value::number(value).unwrap());
        }
        attributes
    }
    fn full() -> BTreeMap<String, Value> {
        report(
            &[
                ("input_tokens", "1000"),
                ("output_tokens", "500"),
                ("cache_read_tokens", "200"),
                ("cache_write_tokens", "100"),
            ],
            &[
                ("input_usd_per_token", "0.000002"),
                ("output_usd_per_token", "0.00001"),
                ("cache_read_usd_per_token", "0.0000005"),
                ("cache_write_usd_per_token", "0.0000025"),
            ],
        )
    }
    #[test]
    fn sums_all_four_categories_exactly() {
        // 1000*0.000002 + 500*0.00001 + 200*0.0000005 + 100*0.0000025
        //   = 0.002 + 0.005 + 0.0001 + 0.00025 = 0.00735
        assert_eq!(token_basis(&full()).unwrap(), d("0.00735"));
    }
    #[test]
    fn applies_both_markups_without_floating_point() {
        let a = full();
        assert_eq!(
            usd_charge(&a, &parse_markup("1.15").unwrap()).unwrap(),
            d("0.0084525")
        );
        assert_eq!(
            usd_charge(&a, &parse_markup("1.3").unwrap()).unwrap(),
            d("0.009555")
        );
        // 0.1 + 0.2 style sums stay exact.
        let a = report(
            &[("input_tokens", "1"), ("output_tokens", "1")],
            &[
                ("input_usd_per_token", "0.1"),
                ("output_usd_per_token", "0.2"),
            ],
        );
        assert_eq!(token_basis(&a).unwrap(), d("0.3"));
        assert_eq!(
            usd_charge(&a, &parse_markup("1.15").unwrap())
                .unwrap()
                .to_string(),
            "0.345"
        );
    }
    #[test]
    fn single_category_and_zero_tokens() {
        let a = report(
            &[("output_tokens", "3")],
            &[("output_usd_per_token", "0.000003")],
        );
        assert_eq!(token_basis(&a).unwrap(), d("0.000009"));
        for zero in [
            report(&[], &[]),
            report(&[("input_tokens", "0")], &[]),
            // A price without tokens is harmless.
            report(&[], &[("input_usd_per_token", "5")]),
            report(
                &[("input_tokens", "0"), ("output_tokens", "0")],
                &[("input_usd_per_token", "5"), ("output_usd_per_token", "5")],
            ),
        ] {
            let charge = usd_charge(&zero, &parse_markup("1.3").unwrap()).unwrap();
            assert!(charge.is_zero());
        }
    }
    #[test]
    fn zero_priced_category_is_free_but_still_priced() {
        let a = report(&[("input_tokens", "1000")], &[("input_usd_per_token", "0")]);
        assert!(token_basis(&a).unwrap().is_zero());
    }
    #[test]
    fn large_values_keep_every_digit() {
        // 999999999999999999 tokens at 0.000000123456789 USD each.
        let a = report(
            &[("input_tokens", "999999999999999999")],
            &[("input_usd_per_token", "0.000000123456789")],
        );
        let basis = token_basis(&a).unwrap();
        assert_eq!(basis.to_string(), "123456788999.999999876543211");
        let charge = usd_charge(&a, &parse_markup("1.15").unwrap()).unwrap();
        assert_eq!(charge.to_string(), "141975307349.99999985802469265");
        // Maximum attribute widths still fit the exact decimal type.
        let long_price = format!("0.{}1", "0".repeat(60));
        let a = report(
            &[("input_tokens", &"9".repeat(60))],
            &[("input_usd_per_token", &long_price)],
        );
        let charge = usd_charge(&a, &parse_markup("1.3").unwrap()).unwrap();
        assert!(Value::number(&charge.to_string()).is_ok());
    }
    #[test]
    fn period_rounding_applies_only_to_the_total() {
        // Each report costs 0.0000115 USD; no report rounds on its own.
        let a = report(
            &[("input_tokens", "10")],
            &[("input_usd_per_token", "0.000001")],
        );
        let each = usd_charge(&a, &parse_markup("1.15").unwrap()).unwrap();
        assert_eq!(each.to_string(), "0.0000115");
        let mut total = Decimal::zero();
        for _ in 0..1000 {
            total = total.add(&each);
        }
        assert_eq!(total.to_string(), "0.0115");
        assert_eq!(
            crate::rating::rounded_charge(&total).unwrap().to_string(),
            "0.01"
        );
        assert_eq!(
            crate::rating::rounded_charge(&each).unwrap().to_string(),
            "0"
        );
    }
    #[test]
    fn missing_price_for_nonzero_tokens_is_rejected() {
        for category in TOKEN_CATEGORIES {
            let a = report(&[(category.tokens, "1")], &[]);
            let error = token_basis(&a).unwrap_err().to_string();
            assert!(error.contains(category.price), "{error}");
        }
        // The other categories' prices do not stand in for the missing one.
        let a = report(
            &[("input_tokens", "1"), ("output_tokens", "1")],
            &[("input_usd_per_token", "1")],
        );
        assert!(token_basis(&a).is_err());
    }
    #[test]
    fn malformed_evidence_is_rejected() {
        let mut cases = Vec::new();
        for (name, value) in [
            ("input_tokens", Value::string("1")),
            ("input_tokens", Value::number("-1").unwrap()),
            ("input_tokens", Value::number("1.5").unwrap()),
            ("input_usd_per_token", Value::string("1")),
            ("input_usd_per_token", Value::number("-0.1").unwrap()),
            ("input_usd_per_token", Value::boolean(true)),
        ] {
            let mut a = report(&[("input_tokens", "1")], &[("input_usd_per_token", "1")]);
            a.insert(name.into(), value);
            cases.push(a);
        }
        for name in [MODEL_ATTRIBUTE, PRICE_EFFECTIVE_ATTRIBUTE] {
            let mut a = full();
            a.remove(name);
            cases.push(a);
        }
        for (name, value) in [
            (MODEL_ATTRIBUTE, Value::string("")),
            (MODEL_ATTRIBUTE, Value::string("m".repeat(201))),
            (MODEL_ATTRIBUTE, Value::number("1").unwrap()),
            (PRICE_EFFECTIVE_ATTRIBUTE, Value::string("yesterday")),
            (PRICE_EFFECTIVE_ATTRIBUTE, Value::number("1").unwrap()),
        ] {
            let mut a = full();
            a.insert(name.into(), value);
            cases.push(a);
        }
        for a in cases {
            assert!(token_basis(&a).is_err(), "{a:?}");
        }
    }
    #[test]
    fn markup_must_be_a_canonical_factor_of_at_least_one() {
        for ok in ["1", "1.15", "1.3", "2"] {
            assert!(parse_markup(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "0.99",
            "-1",
            "1.30",
            "01.5",
            "1e1",
            "abc",
            "1.",
            &"1".repeat(40),
        ] {
            assert!(parse_markup(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn numeric_evidence_must_keep_its_canonical_spelling() {
        for (name, text) in [
            ("input_tokens", "0"),
            ("input_tokens", "1000"),
            ("output_usd_per_token", "0.01"),
            ("cache_read_usd_per_token", "0.0000005"),
            // Other attributes are not AI evidence.
            ("score", "1.0"),
        ] {
            assert!(check_numeric_spelling(name, text).is_ok(), "{name} {text}");
        }
        for (name, text) in [
            ("input_tokens", "1.0"),
            ("cache_write_tokens", "5.00"),
            ("input_usd_per_token", "0.0100"),
            ("output_usd_per_token", "2.0"),
            ("input_tokens", "abc"),
            ("input_tokens", "01"),
        ] {
            assert!(check_numeric_spelling(name, text).is_err(), "{name} {text}");
        }
    }
    #[test]
    fn replacements_keep_the_frozen_prices() {
        let accepted = full();
        assert!(frozen_prices_unchanged(&accepted, &accepted).is_ok());
        // Measured tokens may change.
        let mut more = accepted.clone();
        more.insert("input_tokens".into(), Value::number("3000").unwrap());
        assert!(frozen_prices_unchanged(&accepted, &more).is_ok());
        // Any changed price, cheaper or dearer, would reprice the earlier usage.
        for price in ["0.000001", "0.000003"] {
            let mut repriced = accepted.clone();
            repriced.insert("input_usd_per_token".into(), Value::number(price).unwrap());
            assert!(
                frozen_prices_unchanged(&accepted, &repriced).is_err(),
                "{price}"
            );
        }
        // A category that had no price may gain one; an unpriced zero category may drop it.
        let unpriced = report(&[("output_tokens", "1")], &[("output_usd_per_token", "1")]);
        let mut gained = unpriced.clone();
        gained.insert("input_usd_per_token".into(), Value::number("5").unwrap());
        assert!(frozen_prices_unchanged(&unpriced, &gained).is_ok());
        let mut dropped = accepted.clone();
        dropped.remove("cache_write_usd_per_token");
        dropped.remove("cache_write_tokens");
        assert!(frozen_prices_unchanged(&accepted, &dropped).is_ok());
    }
    #[test]
    fn price_effective_time_is_parsed() {
        let at = price_effective_at(&full()).unwrap();
        assert_eq!(at.to_rfc3339(), "2026-01-01T00:00:00+00:00");
    }
}
