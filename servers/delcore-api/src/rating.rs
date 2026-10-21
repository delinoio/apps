// SPDX-License-Identifier: FSL-1.1-ALv2

//! Pure rating of materialized tenant meter groups. Each group is clamped
//! independently; included usage is shared once across the period.
use crate::{
    accounting::{Decimal, DecimalError, USD_MINOR_UNITS},
    metering::Aggregation,
};
use chrono::{DateTime, Utc};
use std::{
    collections::{HashMap, HashSet},
    fmt,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inconsistent(pub &'static str);
impl fmt::Display for Inconsistent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "inconsistent rating input: {}", self.0)
    }
}
impl std::error::Error for Inconsistent {}
#[derive(Clone, Debug)]
pub struct Group {
    pub key: String,
    pub value: Decimal,
}
#[derive(Clone, Debug)]
pub struct Bucket {
    pub group_key: String,
    /// Empty when no usage price applied at occurrence time.
    pub price_id: String,
    /// Number of grants applied at or before these contributions occurred.
    pub epoch: usize,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    pub count: i64,
    pub sum: Decimal,
}
#[derive(Clone, Debug)]
pub struct Input {
    pub aggregation: Aggregation,
    pub groups: Vec<Group>,
    pub buckets: Vec<Bucket>,
    pub grants: Vec<Decimal>,
    pub prices: HashMap<String, Decimal>,
}
#[derive(Clone, Debug, Default)]
pub struct Segment {
    pub price_id: String,
    pub epoch: usize,
    pub quantity: Decimal,
    pub included: Decimal,
    pub billable: Decimal,
    pub charge: Decimal,
}
#[derive(Clone, Debug, Default)]
pub struct Result {
    pub quantity: Decimal,
    pub unrated: Decimal,
    pub granted: Decimal,
    pub consumed: Decimal,
    pub remaining: Decimal,
    pub billable: Decimal,
    pub charge: Decimal,
    pub segments: Vec<Segment>,
}
pub fn additive(kind: Aggregation) -> bool {
    matches!(kind, Aggregation::Count | Aggregation::Sum)
}
fn max_zero(value: Decimal) -> Decimal {
    value.max(Decimal::zero())
}
fn key(bucket: &Bucket) -> (usize, String) {
    (bucket.epoch, bucket.price_id.clone())
}
fn bucket_value(kind: Aggregation, bucket: &Bucket) -> Decimal {
    if kind == Aggregation::Count {
        Decimal::from_minor(bucket.count.into(), 0)
    } else {
        bucket.sum.clone()
    }
}

pub fn compute(input: &Input) -> std::result::Result<Result, Inconsistent> {
    let mut result = Result::default();
    let mut cumulative = vec![Decimal::zero()];
    for grant in &input.grants {
        if grant < &Decimal::zero() {
            return Err(Inconsistent("negative allowance grant"));
        }
        cumulative.push(cumulative.last().unwrap().add(grant));
    }
    result.granted = cumulative.last().unwrap().clone();
    result.remaining = result.granted.clone();
    if input.groups.is_empty() && input.buckets.is_empty() {
        return Ok(result);
    }
    let mut first = HashMap::new();
    let mut by_group: HashMap<&str, Vec<&Bucket>> = HashMap::new();
    for bucket in &input.buckets {
        if bucket.epoch > input.grants.len() || bucket.count <= 0 {
            return Err(Inconsistent("invalid contribution bucket"));
        }
        if !bucket.price_id.is_empty() && !input.prices.contains_key(&bucket.price_id) {
            return Err(Inconsistent("attributed price is missing"));
        }
        first
            .entry(key(bucket))
            .and_modify(|at: &mut DateTime<Utc>| *at = (*at).min(bucket.first))
            .or_insert(bucket.first);
        by_group.entry(&bucket.group_key).or_default().push(bucket);
    }
    let mut keys: Vec<_> = first.keys().cloned().collect();
    keys.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| first[a].cmp(&first[b]))
            .then_with(|| a.1.cmp(&b.1))
    });
    let index: HashMap<_, _> = keys
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    let mut quantities = vec![Decimal::zero(); keys.len()];
    let mut groups = HashSet::new();
    for group in &input.groups {
        if !groups.insert(group.key.as_str()) {
            return Err(Inconsistent("duplicate group"));
        }
        let Some(buckets) = by_group.get_mut(group.key.as_str()) else {
            return Err(Inconsistent("group has no applied contributions"));
        };
        let clamped = max_zero(group.value.clone());
        result.quantity = result.quantity.add(&clamped);
        if !additive(input.aggregation) {
            // Non-additive groups belong wholly to their latest contribution.
            let latest = buckets
                .iter()
                .max_by(|a, b| {
                    a.last
                        .cmp(&b.last)
                        .then_with(|| a.epoch.cmp(&b.epoch))
                        .then_with(|| a.price_id.cmp(&b.price_id))
                })
                .unwrap();
            let i = index[&key(latest)];
            quantities[i] = quantities[i].add(&clamped);
            continue;
        }
        buckets.sort_by_key(|b| index[&key(b)]);
        let total = buckets.iter().fold(Decimal::zero(), |sum, b| {
            sum.add(&bucket_value(input.aggregation, b))
        });
        if total != group.value {
            return Err(Inconsistent(
                "contributions differ from the materialized aggregate",
            ));
        }
        // Fill earliest positive contributions first. Later negative values
        // reduce total quantity without moving charges to another price.
        let mut left = clamped;
        for bucket in buckets {
            if left.is_zero() {
                break;
            }
            let value = bucket_value(input.aggregation, bucket);
            if value <= Decimal::zero() {
                continue;
            }
            let share = value.min(left.clone());
            left = left.sub(&share);
            let i = index[&key(bucket)];
            quantities[i] = quantities[i].add(&share);
        }
    }
    if by_group.keys().any(|key| !groups.contains(key)) {
        return Err(Inconsistent("contributions have no materialized group"));
    }
    for ((epoch, price_id), quantity) in keys.into_iter().zip(quantities) {
        let mut segment = Segment {
            price_id,
            epoch,
            quantity,
            ..Default::default()
        };
        if segment.price_id.is_empty() {
            result.unrated = result.unrated.add(&segment.quantity);
        } else {
            let available = max_zero(cumulative[epoch].sub(&result.consumed));
            segment.included = segment.quantity.clone().min(available);
            segment.billable = segment.quantity.sub(&segment.included);
            segment.charge = segment.billable.mul(&input.prices[&segment.price_id]);
            result.consumed = result.consumed.add(&segment.included);
            result.billable = result.billable.add(&segment.billable);
            result.charge = result.charge.add(&segment.charge);
        }
        result.segments.push(segment);
    }
    result.remaining = result.granted.sub(&result.consumed);
    Ok(result)
}
pub fn rounded_charge(charge: &Decimal) -> std::result::Result<Decimal, DecimalError> {
    Ok(Decimal::from_minor(
        charge.minor_units(USD_MINOR_UNITS)?,
        USD_MINOR_UNITS,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(s: &str) -> Decimal {
        Decimal::parse(s).unwrap()
    }
    fn bucket(group: &str, price: &str, epoch: usize, minute: i64, sum: &str) -> Bucket {
        let at = DateTime::from_timestamp(1_772_323_200 + minute * 60, 0).unwrap();
        Bucket {
            group_key: group.into(),
            price_id: price.into(),
            epoch,
            first: at,
            last: at,
            count: 1,
            sum: d(sum),
        }
    }
    fn input(
        groups: &[(&str, &str)],
        buckets: Vec<Bucket>,
        grants: &[&str],
        prices: &[(&str, &str)],
    ) -> Input {
        Input {
            aggregation: Aggregation::Sum,
            groups: groups
                .iter()
                .map(|(key, v)| Group {
                    key: (*key).into(),
                    value: d(v),
                })
                .collect(),
            buckets,
            grants: grants.iter().map(|s| d(s)).collect(),
            prices: prices.iter().map(|(k, v)| ((*k).into(), d(v))).collect(),
        }
    }
    #[test]
    fn clamps_each_group_and_shares_allowance_once() {
        let r = compute(&input(
            &[("a", "-5"), ("b", "20")],
            vec![bucket("a", "p", 1, 1, "-5"), bucket("b", "p", 1, 2, "20")],
            &["10"],
            &[("p", "0.5")],
        ))
        .unwrap();
        assert_eq!(r.quantity, d("20"));
        assert_eq!(r.billable, d("10"));
        assert_eq!(r.charge, d("5"));
        assert_eq!(r.remaining, d("0"));
        let mut i = input(
            &[("a", "3"), ("b", "4")],
            vec![bucket("a", "p", 1, 1, "0"), bucket("b", "p", 1, 2, "0")],
            &["5"],
            &[("p", "0.5")],
        );
        i.aggregation = Aggregation::Count;
        i.buckets[0].count = 3;
        i.buckets[1].count = 4;
        let r = compute(&i).unwrap();
        assert_eq!(r.billable, d("2"));
        assert_eq!(r.charge, d("1"));
        let r = compute(&input(
            &[("a", "-10"), ("b", "4")],
            vec![bucket("a", "p", 0, 1, "-10"), bucket("b", "p", 0, 2, "4")],
            &[],
            &[("p", "0.5")],
        ))
        .unwrap();
        assert_eq!(r.billable, d("4"));
        assert_eq!(r.charge, d("2"));
        let r = compute(&input(
            &[("a", "-3")],
            vec![bucket("a", "p", 1, 1, "-3")],
            &["2"],
            &[("p", "0.5")],
        ))
        .unwrap();
        assert_eq!(r.charge, d("0"));
        assert_eq!(r.remaining, d("2"));
    }
    #[test]
    fn empty_period_preserves_grants() {
        let r = compute(&input(&[], vec![], &["2", "1.5"], &[])).unwrap();
        assert_eq!(r.granted, d("3.5"));
        assert_eq!(r.remaining, d("3.5"));
        assert_eq!(r.charge, d("0"));
    }
    #[test]
    fn mixed_prices_follow_grant_epochs_and_occurrences() {
        let prices = [("old", "1"), ("new", "3")];
        let r = compute(&input(
            &[("g", "15")],
            vec![
                bucket("g", "new", 2, 20, "5"),
                bucket("g", "old", 1, 10, "10"),
            ],
            &["4", "3"],
            &prices,
        ))
        .unwrap();
        assert_eq!(r.segments[0].price_id, "old");
        assert_eq!(r.segments[1].price_id, "new");
        assert_eq!(r.segments[0].billable, d("6"));
        assert_eq!(r.segments[1].included, d("3"));
        assert_eq!(r.segments[1].billable, d("2"));
        assert_eq!(r.charge, d("12"));
        assert_eq!(r.remaining, d("0"));
        let r = compute(&input(
            &[("g", "6")],
            vec![
                bucket("g", "old", 0, 10, "10"),
                bucket("g", "new", 0, 20, "-4"),
            ],
            &[],
            &prices,
        ))
        .unwrap();
        assert_eq!(r.charge, d("6"));
        let r = compute(&input(
            &[("g", "7")],
            vec![bucket("g", "", 1, 1, "4"), bucket("g", "old", 1, 2, "3")],
            &["2"],
            &prices,
        ))
        .unwrap();
        assert_eq!(r.unrated, d("4"));
        assert_eq!(r.billable, d("1"));
        assert_eq!(r.charge, d("1"));
        assert_eq!(r.remaining, d("0"));
    }
    #[test]
    fn non_additive_groups_use_latest_contribution() {
        for kind in [
            Aggregation::Min,
            Aggregation::Max,
            Aggregation::Avg,
            Aggregation::UniqueCount,
        ] {
            let mut i = input(
                &[("a", "4.5"), ("b", "-2")],
                vec![
                    bucket("a", "old", 1, 5, "1"),
                    bucket("a", "new", 1, 9, "100"),
                    bucket("b", "old", 1, 2, "-2"),
                ],
                &["1"],
                &[("old", "1"), ("new", "2")],
            );
            i.aggregation = kind;
            let r = compute(&i).unwrap();
            assert_eq!(r.quantity, d("4.5"));
            assert_eq!(r.billable, d("3.5"));
            assert_eq!(r.charge, d("7"));
        }
    }
    #[test]
    fn rejects_inconsistent_inputs() {
        let valid = input(
            &[("a", "3")],
            vec![bucket("a", "p", 0, 1, "3")],
            &[],
            &[("p", "1")],
        );
        let mut cases = vec![];
        let mut i = valid.clone();
        i.groups[0].value = d("2");
        cases.push(i);
        let mut i = valid.clone();
        i.buckets.clear();
        cases.push(i);
        let mut i = valid.clone();
        i.groups.clear();
        cases.push(i);
        let mut i = valid.clone();
        i.prices.clear();
        cases.push(i);
        let mut i = valid.clone();
        i.buckets[0].epoch = 1;
        cases.push(i);
        let mut i = valid.clone();
        i.grants.push(d("-1"));
        cases.push(i);
        let mut i = valid.clone();
        i.groups.push(i.groups[0].clone());
        cases.push(i);
        let mut i = valid.clone();
        i.buckets[0].count = 0;
        cases.push(i);
        for i in cases {
            assert!(compute(&i).is_err());
        }
    }
    #[test]
    fn final_rounding_is_half_away_and_not_per_event() {
        for (value, want) in [
            ("0.125", "0.13"),
            ("0.124999", "0.12"),
            ("-0.125", "-0.13"),
            ("0.006", "0.01"),
            ("0.0049", "0"),
            ("12", "12"),
        ] {
            assert_eq!(rounded_charge(&d(value)).unwrap(), d(want));
        }
        let r = compute(&input(
            &[("g", "2")],
            vec![bucket("g", "p", 0, 1, "2")],
            &[],
            &[("p", "0.006")],
        ))
        .unwrap();
        assert_eq!(r.charge, d("0.012"));
        assert_eq!(rounded_charge(&r.charge).unwrap(), d("0.01"));
    }
    #[test]
    fn equal_occurrences_break_ties_by_epoch_then_price() {
        let mut i = input(
            &[("g", "4")],
            vec![bucket("g", "a", 0, 1, "1"), bucket("g", "b", 1, 1, "3")],
            &["1"],
            &[("a", "1"), ("b", "2")],
        );
        i.aggregation = Aggregation::Max;
        let r = compute(&i).unwrap();
        assert_eq!(r.charge, d("6"));
        i.buckets[0].epoch = 1;
        let r = compute(&i).unwrap();
        assert_eq!(r.charge, d("6"));
    }
}
