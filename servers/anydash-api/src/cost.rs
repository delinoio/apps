// SPDX-License-Identifier: FSL-1.1-ALv2
//! Estimated cost rules: model matching, required token breakdowns and USD totals.
use delino_protocol::anydash::v1::{CostEstimate, LocalTool, MetricState};
use std::collections::BTreeSet;

/// Per-token USD prices of one model as PostgreSQL numeric text; `None` when LiteLLM omits one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prices {
    pub input: Option<String>,
    pub output: Option<String>,
    pub cache_read: Option<String>,
    pub cache_write: Option<String>,
}

/// Token counts of one bucket in the order input, output, cache read, cache write.
pub type Tokens = [Option<i64>; 4];

/// LiteLLM keys tried for a model, in order: the exact key, then the provider-prefixed key.
pub fn model_keys(tool: LocalTool, model: &str) -> Vec<String> {
    let prefix = match tool {
        LocalTool::Codex => "openai/",
        LocalTool::ClaudeCode => "anthropic/",
        _ => return vec![model.into()],
    };
    vec![model.into(), format!("{prefix}{model}")]
}

/// Counts a tool must report for its cost to be computable.
pub fn required(tool: LocalTool) -> [bool; 4] {
    match tool {
        LocalTool::ClaudeCode => [true, true, true, true],
        _ => [true, true, true, false],
    }
}

/// A bucket is computable when every required count is present and every non-zero count has a
/// price in the pinned version.
pub fn computable(tool: LocalTool, tokens: &Tokens, prices: Option<&Prices>) -> bool {
    let Some(prices) = prices else {
        return false;
    };
    let price = [
        &prices.input,
        &prices.output,
        &prices.cache_read,
        &prices.cache_write,
    ];
    let required = required(tool);
    (0..4).all(|i| {
        if required[i] && tokens[i].is_none() {
            return false;
        }
        tokens[i].unwrap_or(0) == 0 || price[i].is_some()
    })
}

/// USD amounts as integer units of 10^-12, the stored numeric scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Usd(pub i128);

impl Usd {
    pub const SCALE: u32 = 12;

    /// Parses PostgreSQL numeric text with at most 12 fractional digits.
    pub fn parse(text: &str) -> Option<Self> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
        if int.is_empty() || frac.len() > Self::SCALE as usize {
            return None;
        }
        let mut value: i128 = int.parse().ok()?;
        let mut frac_value: i128 = if frac.is_empty() {
            0
        } else {
            frac.parse().ok()?
        };
        frac_value *= 10i128.pow(Self::SCALE - frac.len() as u32);
        value = value
            .checked_mul(10i128.pow(Self::SCALE))?
            .checked_add(frac_value)?;
        Some(Self(if negative { -value } else { value }))
    }

    /// Decimal text rounded half up to 6 places.
    pub fn format6(self) -> String {
        let unit = 10i128.pow(Self::SCALE - 6);
        let negative = self.0 < 0;
        let abs = self.0.abs();
        let rounded = (abs + unit / 2) / unit;
        let text = format!("{}.{:06}", rounded / 1_000_000, rounded % 1_000_000);
        if negative && rounded != 0 {
            format!("-{text}")
        } else {
            text
        }
    }
}

/// Accumulates bucket costs into a `CostEstimate`.
#[derive(Clone, Debug, Default)]
pub struct CostSum {
    pub amount: Usd,
    pub computable: u64,
    pub missing: u64,
    pub versions: BTreeSet<String>,
}

impl CostSum {
    pub fn add(&mut self, computable: bool, amount: Option<&str>, version: Option<String>) {
        match (computable, amount.and_then(Usd::parse)) {
            (true, Some(a)) => {
                self.amount = Usd(self.amount.0 + a.0);
                self.computable += 1;
                if let Some(v) = version {
                    self.versions.insert(v);
                }
            }
            _ => self.missing += 1,
        }
    }
    pub fn merge(&mut self, other: &CostSum) {
        self.amount = Usd(self.amount.0 + other.amount.0);
        self.computable += other.computable;
        self.missing += other.missing;
        self.versions.extend(other.versions.iter().cloned());
    }
    pub fn present(&self) -> bool {
        self.computable + self.missing > 0
    }
    /// `VALUE` when every contributing bucket is computable, `CANNOT_BE_CALCULATED` when none
    /// is, and `PARTIAL` with the computable sum otherwise.
    pub fn estimate(&self) -> CostEstimate {
        let state = if self.missing == 0 && self.computable > 0 {
            MetricState::Value
        } else if self.computable == 0 && self.missing > 0 {
            MetricState::CannotBeCalculated
        } else if self.computable == 0 {
            MetricState::Value
        } else {
            MetricState::Partial
        };
        CostEstimate {
            state: state.into(),
            amount_usd: if state == MetricState::CannotBeCalculated {
                String::new()
            } else {
                self.amount.format6()
            },
            price_version_ids: self.versions.iter().cloned().collect(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prices(all: bool) -> Prices {
        Prices {
            input: Some("0.000003".into()),
            output: Some("0.000015".into()),
            cache_read: Some("0.0000003".into()),
            cache_write: all.then(|| "0.00000375".into()),
        }
    }
    #[test]
    fn cannot_be_calculated_without_price_or_required_breakdown() {
        let full: Tokens = [Some(10), Some(5), Some(2), Some(1)];
        assert!(computable(
            LocalTool::ClaudeCode,
            &full,
            Some(&prices(true))
        ));
        assert!(!computable(LocalTool::ClaudeCode, &full, None));
        // A non-zero count without a price.
        assert!(!computable(
            LocalTool::ClaudeCode,
            &full,
            Some(&prices(false))
        ));
        // A zero count needs no price.
        assert!(computable(
            LocalTool::ClaudeCode,
            &[Some(10), Some(5), Some(2), Some(0)],
            Some(&prices(false))
        ));
        // Claude Code requires the cache-write count; Codex does not.
        assert!(!computable(
            LocalTool::ClaudeCode,
            &[Some(1), Some(1), Some(1), None],
            Some(&prices(true))
        ));
        assert!(computable(
            LocalTool::Codex,
            &[Some(1), Some(1), Some(1), None],
            Some(&prices(false))
        ));
        assert!(!computable(
            LocalTool::Codex,
            &[Some(1), None, Some(1), None],
            Some(&prices(true))
        ));
        assert_eq!(
            model_keys(LocalTool::Codex, "gpt-5"),
            vec!["gpt-5", "openai/gpt-5"]
        );
        assert_eq!(
            model_keys(LocalTool::ClaudeCode, "claude-opus-4"),
            vec!["claude-opus-4", "anthropic/claude-opus-4"]
        );
    }
    #[test]
    fn totals_round_half_up_and_report_partial_states() {
        assert_eq!(Usd::parse("1.0000005").unwrap().format6(), "1.000001");
        assert_eq!(Usd::parse("1.00000049").unwrap().format6(), "1.000000");
        assert_eq!(Usd::parse("0").unwrap().format6(), "0.000000");
        assert!(Usd::parse("1.0000000000001").is_none());
        let mut sum = CostSum::default();
        sum.add(true, Some("0.25"), Some("v1".into()));
        sum.add(true, Some("0.5"), Some("v2".into()));
        let e = sum.estimate();
        assert_eq!(e.state, MetricState::Value);
        assert_eq!(e.amount_usd, "0.750000");
        assert_eq!(e.price_version_ids, vec!["v1", "v2"]);
        sum.add(false, None, None);
        assert_eq!(sum.estimate().state, MetricState::Partial);
        assert_eq!(sum.estimate().amount_usd, "0.750000");
        let mut none = CostSum::default();
        none.add(false, None, None);
        assert_eq!(none.estimate().state, MetricState::CannotBeCalculated);
        assert_eq!(none.estimate().amount_usd, "");
    }
}
