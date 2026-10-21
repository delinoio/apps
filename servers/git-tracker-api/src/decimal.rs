// SPDX-License-Identifier: FSL-1.1-ALv2
use delino_protocol::gittracker::v1::{Column, Comparison, ComparisonStatus::*, Direction::*};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Signed, Zero};
use server_common::{Fault, Result};
use std::sync::LazyLock;
static DECIMAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$").unwrap()
});
pub fn parse(s: &str) -> Result<BigRational> {
    if s.len() > 4096 || !DECIMAL.is_match(s) {
        return Err(Fault::precondition("invalid finite decimal"));
    }
    let (mantissa, exponent) = s.split_once(['e', 'E']).unwrap_or((s, "0"));
    let exponent: i32 = exponent
        .parse()
        .map_err(|_| Fault::precondition("decimal exponent out of range"))?;
    if !(-4096..=4096).contains(&exponent) {
        return Err(Fault::precondition("decimal exponent out of range"));
    }
    let fractional = mantissa.split_once('.').map_or(0, |(_, f)| f.len() as i32);
    let value: BigInt = mantissa
        .replace('.', "")
        .parse()
        .map_err(|_| Fault::precondition("invalid decimal"))?;
    let power = exponent - fractional;
    let ten = BigInt::from(10).pow(power.unsigned_abs());
    Ok(if power >= 0 {
        BigRational::from_integer(value * ten)
    } else {
        BigRational::new(value, ten)
    })
}
pub fn text(v: &BigRational) -> String {
    let scale = BigInt::from(10).pow(18);
    let numerator = v.numer().abs() * &scale;
    let mut integer = &numerator / v.denom();
    let remainder = &numerator % v.denom();
    if remainder * 2 >= *v.denom() {
        integer += 1
    }
    let digits = format!("{:0>19}", integer.to_string());
    let split = digits.len() - 18;
    let mut out = format!("{}.{}", &digits[..split], &digits[split..]);
    out = out.trim_end_matches('0').trim_end_matches('.').to_owned();
    if v.is_negative() && out != "0" {
        out.insert(0, '-')
    }
    out
}
#[derive(PartialEq, Eq)]
enum Kind {
    Size,
    Throughput,
    Time,
    Operations,
    Count,
    Ratio,
    Unitless,
}
fn unit(s: &str) -> Option<(Kind, BigRational)> {
    let (name, throughput) = s.strip_suffix("/s").map_or((s, false), |n| (n, true));
    let scale = match name {
        "B" => Some(1),
        "kB" => Some(1000),
        "MB" => Some(1_000_000),
        "GB" => Some(1_000_000_000),
        "KiB" => Some(1024),
        "MiB" => Some(1_048_576),
        "GiB" => Some(1_073_741_824),
        _ => None,
    };
    if let Some(scale) = scale {
        return Some((
            if throughput {
                Kind::Throughput
            } else {
                Kind::Size
            },
            BigRational::from_integer(scale.into()),
        ));
    }
    let (kind, denom) = match s {
        "ns" => (Kind::Time, 1_000_000_000),
        "μs" => (Kind::Time, 1_000_000),
        "ms" => (Kind::Time, 1000),
        "s" => (Kind::Time, 1),
        "ops/s" => (Kind::Operations, 1),
        "count" => (Kind::Count, 1),
        "%" => (Kind::Ratio, 1),
        "unitless" => (Kind::Unitless, 1),
        _ => return None,
    };
    Some((kind, BigRational::new(1.into(), denom.into())))
}
pub fn supported(s: &str) -> bool {
    unit(s).is_some()
}
pub fn compare(target: &Column, base: Option<&Column>, value: &str, baseline: &str) -> Comparison {
    let mut out = Comparison {
        column_id: target.id.clone(),
        value: value.into(),
        unit: target.unit.clone(),
        status: COMPARISON_STATUS_NEW.into(),
        ..Default::default()
    };
    let Some(base) = base else { return out };
    let (Some(a), Some(b)) = (unit(&target.unit), unit(&base.unit)) else {
        return out;
    };
    if a.0 != b.0 {
        out.status = COMPARISON_STATUS_INCOMPATIBLE.into();
        return out;
    }
    let (Ok(value), Ok(old)) = (parse(value), parse(baseline)) else {
        return out;
    };
    let old = old * b.1 / a.1;
    let delta = value - &old;
    out.baseline_value = baseline.into();
    out.baseline_unit = base.unit.clone();
    out.delta = text(&delta);
    let pct = if old.is_zero() {
        None
    } else {
        Some(&delta / old.abs() * BigInt::from(100))
    };
    if let Some(pct) = &pct {
        out.percentage = text(pct)
    }
    out.status = if delta.is_zero() {
        COMPARISON_STATUS_UNCHANGED
    } else {
        COMPARISON_STATUS_CHANGED
    }
    .into();
    if delta.is_zero() || target.direction == DIRECTION_NEUTRAL {
        return out;
    }
    let worse = (target.direction == DIRECTION_LOWER && delta.is_positive())
        || (target.direction == DIRECTION_HIGHER && delta.is_negative());
    if !worse {
        out.status = COMPARISON_STATUS_IMPROVEMENT.into();
        return out;
    }
    for (threshold, value) in [
        (&target.absolute_threshold, Some(&delta)),
        (&target.percentage_threshold, pct.as_ref()),
    ] {
        if let (Ok(limit), Some(value)) = (parse(threshold), value)
            && value.abs() > limit
        {
            out.status = COMPARISON_STATUS_REGRESSION.into()
        }
    }
    out
}
