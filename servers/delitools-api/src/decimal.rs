// SPDX-License-Identifier: FSL-1.1-ALv2
//! Exact decimal helpers. No floating point is used for prices, token counts or enum comparison.
//!
//! [`Exact`] normalizes a JSON number lexeme to `sign`, significant `digits` without leading or
//! trailing zeros and a power of ten, so `1.10`, `1.1` and `11e-1` compare equal without ever
//! constructing `10^exponent` (a hostile `1e999999999` costs only its own spelling). [`price`]
//! turns a Gateway price string into the plain canonical non-negative decimal that delcore
//! accepts. Charge arithmetic itself belongs to delcore.

/// Largest absolute decimal exponent accepted in untrusted input.
const MAX_EXPONENT: i64 = 1_000_000;
/// Longest canonical price text delcore accepts for a numeric attribute.
const MAX_PRICE_BYTES: usize = 64;

/// A normalized exact decimal: `(-1)^negative * 0.digits... = digits * 10^exponent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exact {
    negative: bool,
    /// Significant digits; empty for zero.
    digits: String,
    exponent: i64,
}

impl Exact {
    /// Parse a JSON-grammar or plain decimal lexeme. Returns `None` for malformed text or an
    /// exponent beyond a million.
    pub fn parse(lexeme: &str) -> Option<Exact> {
        let (negative, rest) = match lexeme.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, lexeme),
        };
        let (mantissa, exponent) = match rest.find(['e', 'E']) {
            Some(index) => {
                let text = &rest[index + 1..];
                let (sign, digits) = match text.strip_prefix('-') {
                    Some(d) => (-1_i64, d),
                    None => (1, text.strip_prefix('+').unwrap_or(text)),
                };
                if digits.is_empty()
                    || digits.len() > 9
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                (&rest[..index], sign * digits.parse::<i64>().ok()?)
            }
            None => (rest, 0),
        };
        if exponent.abs() > MAX_EXPONENT {
            return None;
        }
        let (integer, fraction) = match mantissa.split_once('.') {
            Some((i, f)) => (i, f),
            None => (mantissa, ""),
        };
        if integer.is_empty()
            || !integer.bytes().all(|b| b.is_ascii_digit())
            || (mantissa.contains('.') && fraction.is_empty())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let all = format!("{integer}{fraction}");
        let trimmed_leading = all.trim_start_matches('0');
        if trimmed_leading.is_empty() {
            return Some(Exact {
                negative: false,
                digits: String::new(),
                exponent: 0,
            });
        }
        let digits = trimmed_leading.trim_end_matches('0');
        let trailing = (trimmed_leading.len() - digits.len()) as i64;
        Some(Exact {
            negative,
            digits: digits.to_owned(),
            exponent: exponent - fraction.len() as i64 + trailing,
        })
    }

    pub fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }
    /// Whether the value is a whole number (zero counts).
    pub fn is_integer(&self) -> bool {
        self.is_zero() || self.exponent >= 0
    }
}

/// Plain canonical decimal for a published price: digits with an optional fraction, no exponent,
/// no sign, no leading zeros and no trailing fractional zeros. Returns `None` for a negative,
/// malformed or over-long value.
pub fn price(text: &str) -> Option<String> {
    let exact = Exact::parse(text)?;
    if exact.negative {
        return None;
    }
    let plain = plain(&exact)?;
    (plain.len() <= MAX_PRICE_BYTES).then_some(plain)
}

fn plain(exact: &Exact) -> Option<String> {
    if exact.is_zero() {
        return Some("0".into());
    }
    let digits = &exact.digits;
    let length = digits.len() as i64;
    // Bound the expansion; real prices are tiny fractions of a dollar.
    if exact.exponent > 40 || length + exact.exponent.min(0).abs() > 120 {
        return None;
    }
    Some(if exact.exponent >= 0 {
        format!("{digits}{}", "0".repeat(exact.exponent as usize))
    } else {
        let places = (-exact.exponent) as usize;
        if digits.len() > places {
            let split = digits.len() - places;
            format!("{}.{}", &digits[..split], &digits[split..])
        } else {
            format!("0.{}{digits}", "0".repeat(places - digits.len()))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_values_normalize_identically() {
        let reference = Exact::parse("1.1").unwrap();
        for same in [
            "1.10",
            "11e-1",
            "0.11e1",
            "0011e-1".trim_start_matches('0'),
            "1.1E0",
        ] {
            assert_eq!(Exact::parse(same).unwrap(), reference, "{same}");
        }
        assert_ne!(Exact::parse("1.1000000000000001").unwrap(), reference);
        assert_ne!(Exact::parse("-1.1").unwrap(), reference);
        assert_eq!(Exact::parse("-0").unwrap(), Exact::parse("0.000").unwrap());
    }

    #[test]
    fn integer_detection_is_exact() {
        for yes in ["0", "-0", "7", "7.0", "70e-1", "1e3", "-12"] {
            assert!(Exact::parse(yes).unwrap().is_integer(), "{yes}");
        }
        for no in ["7.5", "75e-1", "1e-3", "0.0001", "1.0000000000000000000001"] {
            assert!(!Exact::parse(no).unwrap().is_integer(), "{no}");
        }
    }

    #[test]
    fn hostile_exponents_are_rejected_without_expansion() {
        assert!(Exact::parse("1e99999999999").is_none());
        assert!(Exact::parse("1e1000001").is_none());
        assert!(Exact::parse("1e999999").is_some());
        assert!(price("1e999999").is_none());
    }

    #[test]
    fn malformed_text_is_rejected() {
        for bad in [
            "", "-", ".5", "5.", "1e", "1e+", "a", "1.2.3", "--1", " 1", "1 ",
        ] {
            assert!(Exact::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn prices_become_plain_canonical_decimals() {
        for (input, expected) in [
            ("0.000002", "0.000002"),
            ("2e-6", "0.000002"),
            ("0.0000020", "0.000002"),
            ("0.00000025", "0.00000025"),
            ("1", "1"),
            ("1.50", "1.5"),
            ("0", "0"),
            ("0.0", "0"),
            ("15E-7", "0.0000015"),
            ("12e2", "1200"),
        ] {
            assert_eq!(price(input).as_deref(), Some(expected), "{input}");
        }
        for bad in ["-0.1", "x", "", "1e41", "0.1.2"] {
            assert!(price(bad).is_none(), "{bad}");
        }
        assert!(price(&format!("0.{}1", "0".repeat(70))).is_none());
    }
}
