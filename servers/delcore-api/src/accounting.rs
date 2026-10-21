// SPDX-License-Identifier: FSL-1.1-ALv2

//! Exact decimal arithmetic. Monetary products retain their full precision;
//! rounding happens only at the final currency minor-unit boundary.
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Signed, Zero};
use std::{cmp::Ordering, fmt, str::FromStr};

pub const USD: &str = "USD";
pub const USD_MINOR_UNITS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecimalError;
impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid decimal")
    }
}
impl std::error::Error for DecimalError {}

#[derive(Clone, Debug, Default)]
pub struct Decimal {
    coefficient: BigInt,
    scale: usize,
}
fn pow(n: usize) -> BigInt {
    BigInt::from(10).pow(n.try_into().expect("decimal scale fits u32"))
}
impl Decimal {
    pub fn parse(s: &str) -> Result<Self, DecimalError> {
        if s.len() > 256 {
            return Err(DecimalError);
        }
        let unsigned = s.strip_prefix('-').unwrap_or(s);
        let mut parts = unsigned.split('.');
        let integer = parts.next().ok_or(DecimalError)?;
        let fractional = parts.next();
        if integer.is_empty()
            || !integer.bytes().all(|c| c.is_ascii_digit())
            || (integer.len() > 1 && integer.starts_with('0'))
            || parts.next().is_some()
            || fractional.is_some_and(|v| v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()))
        {
            return Err(DecimalError);
        }
        Ok(Self {
            coefficient: s.replace('.', "").parse().map_err(|_| DecimalError)?,
            scale: fractional.map_or(0, str::len),
        })
    }
    pub fn zero() -> Self {
        Self::default()
    }
    fn coefficient_at(&self, scale: usize) -> BigInt {
        &self.coefficient * pow(scale - self.scale)
    }
    pub fn add(&self, other: &Self) -> Self {
        let scale = self.scale.max(other.scale);
        Self {
            coefficient: self.coefficient_at(scale) + other.coefficient_at(scale),
            scale,
        }
    }
    pub fn neg(&self) -> Self {
        Self {
            coefficient: -&self.coefficient,
            scale: self.scale,
        }
    }
    pub fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }
    pub fn mul(&self, other: &Self) -> Self {
        Self {
            coefficient: &self.coefficient * &other.coefficient,
            scale: self.scale + other.scale,
        }
    }
    /// Round half away from zero, preserving symmetry for reversals.
    pub fn minor_units(&self, places: usize) -> Result<BigInt, DecimalError> {
        if places > 6 {
            return Err(DecimalError);
        }
        if places >= self.scale {
            return Ok(self.coefficient_at(places));
        }
        let den = pow(self.scale - places);
        let mut quotient = &self.coefficient / &den;
        let remainder = &self.coefficient % &den;
        if remainder.abs() * 2 >= den {
            quotient += self.coefficient.signum();
        }
        Ok(quotient)
    }
    pub fn is_minor_exact(&self, places: usize) -> bool {
        self.minor_units(places)
            .is_ok_and(|v| *self == Self::from_minor(v, places))
    }
    pub fn from_minor(units: BigInt, places: usize) -> Self {
        Self {
            coefficient: units,
            scale: places,
        }
    }
    pub fn rational(&self) -> BigRational {
        BigRational::new(self.coefficient.clone(), pow(self.scale))
    }
    pub fn is_zero(&self) -> bool {
        self.coefficient.is_zero()
    }
    /// Divide by a positive count and round half away from zero.
    pub fn div_round(&self, divisor: i64, places: usize) -> Result<Self, DecimalError> {
        if divisor <= 0 || places > 6 {
            return Err(DecimalError);
        }
        let scaled = self.rational() * BigRational::new(pow(places), divisor.into());
        let mut units = scaled.to_integer();
        if (scaled.numer() % scaled.denom()).abs() * 2 >= *scaled.denom() {
            units += scaled.numer().signum();
        }
        Ok(Self::from_minor(units, places))
    }
}
impl FromStr for Decimal {
    type Err = DecimalError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}
impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Decimal {}
impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let scale = self.scale.max(other.scale);
        self.coefficient_at(scale).cmp(&other.coefficient_at(scale))
    }
}
impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut value = self.coefficient.abs().to_string();
        if self.scale > 0 {
            if value.len() <= self.scale {
                value = format!("{}{}", "0".repeat(self.scale - value.len() + 1), value);
            }
            value.insert(value.len() - self.scale, '.');
            value = value.trim_end_matches('0').trim_end_matches('.').to_owned();
        }
        if self.coefficient.is_negative() {
            f.write_str("-")?;
        }
        f.write_str(&value)
    }
}
pub fn prorated(
    amount: &Decimal,
    remaining: i64,
    duration: i64,
) -> Result<BigRational, DecimalError> {
    if remaining < 0 || duration <= 0 || remaining > duration {
        return Err(DecimalError);
    }
    Ok(amount.rational() * BigRational::new(remaining.into(), duration.into()))
}
/// Truncate only the allowance to the supported 1e-6 quantity quantum.
pub fn prorated_allowance(
    amount: &Decimal,
    remaining: i64,
    duration: i64,
) -> Result<Decimal, DecimalError> {
    let value = prorated(amount, remaining, duration)? * BigRational::from_integer(pow(6));
    Ok(Decimal::from_minor(value.to_integer(), 6))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(value: &str) -> Decimal {
        Decimal::parse(value).unwrap()
    }
    #[test]
    fn exact_arithmetic_and_rounding() {
        assert_eq!(d("0.1").add(&d("0.2")).to_string(), "0.3");
        for (input, expected) in [
            ("1.005", "101"),
            ("-1.005", "-101"),
            ("0.004999999999999999999999", "0"),
            ("999999999999999999999.999", "100000000000000000000000"),
        ] {
            assert_eq!(d(input).minor_units(2).unwrap().to_string(), expected);
        }
        assert_eq!(d("1.00"), d("1"));
        assert_eq!(d("-0.000").to_string(), "0");
        assert_eq!(d("2.30").sub(&d("3.40")).to_string(), "-1.1");
        assert!(d("1.23").is_minor_exact(2));
        assert!(!d("1.234").is_minor_exact(2));
        assert!(d("1").minor_units(7).is_err());
        assert!(Decimal::parse(&"9".repeat(256)).is_ok());
        assert!(Decimal::parse(&"9".repeat(257)).is_err());
    }
    #[test]
    fn rejects_non_decimal_syntax() {
        for value in [
            "NaN", "1e3", "+1", "01", ".1", "1.", "1/3", "", "-", "1.2.3", " 1", "１",
        ] {
            assert!(Decimal::parse(value).is_err(), "accepted {value:?}");
        }
    }
    #[test]
    fn exact_proration_and_allowance_precision() {
        assert_eq!(prorated(&d("100"), 1, 3).unwrap().to_string(), "100/3");
        let allowance = prorated_allowance(&d("100"), 1, 3).unwrap();
        assert_eq!(allowance.to_string(), "33.333333");
        assert_eq!(
            allowance.mul(&d("0.0000007")).to_string(),
            "0.0000233333331"
        );
        for (remaining, duration) in [(-1, 3), (1, 0), (4, 3)] {
            assert!(prorated(&d("100"), remaining, duration).is_err());
        }
    }
}
