// SPDX-License-Identifier: Apache-2.0
use crate::{evidence::*, types::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    Minimize,
    Maximize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IrPolicy {
    pub minimum_improvement: f64,
    pub maximum_case_regression: f64,
    pub near_threshold: f64,
    pub repeat_tolerance: f64,
    pub allow_simple_exception: bool,
    pub allow_single_case_exception: bool,
}
impl Default for IrPolicy {
    fn default() -> Self {
        Self {
            minimum_improvement: 0.01,
            maximum_case_regression: 0.01,
            near_threshold: 0.001,
            repeat_tolerance: 0.001,
            allow_simple_exception: true,
            allow_single_case_exception: true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Uncertainty {
    /// Maximum (max - min) / mean, including same-SHA baseline variability.
    pub maximum_relative_range: f64,
    /// A conservative margin subtracted from improvement and added to regression.
    pub decision_margin: f64,
    pub repeat_tolerance: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeclaredPolicy {
    pub objective: Objective,
    pub minimum_improvement: f64,
    pub maximum_case_regression: f64,
    pub repetitions: u32,
    pub uncertainty: Uncertainty,
    pub near_threshold: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "settings", rename_all = "snake_case")]
pub enum Policy {
    Ir(IrPolicy),
    Declared(DeclaredPolicy),
    MeasurementsOnly,
}
fn fraction(v: f64) -> bool {
    v.is_finite() && (0.0..1.0).contains(&v)
}
impl Policy {
    pub fn validate(&self, metric: Metric) -> Result<()> {
        let values = match self {
            Self::Ir(p) => {
                if metric != Metric::Ir {
                    return Err(Error::Invalid("Ir policy metric"));
                }
                vec![
                    p.minimum_improvement,
                    p.maximum_case_regression,
                    p.near_threshold,
                    p.repeat_tolerance,
                ]
            }
            Self::Declared(p) => {
                if metric == Metric::Ir || p.repetitions < 2 {
                    return Err(Error::Invalid("declared policy repetitions or metric"));
                }
                vec![
                    p.minimum_improvement,
                    p.maximum_case_regression,
                    p.near_threshold,
                    p.uncertainty.maximum_relative_range,
                    p.uncertainty.decision_margin,
                    p.uncertainty.repeat_tolerance,
                ]
            }
            Self::MeasurementsOnly => vec![],
        };
        if values.into_iter().any(|v| !fraction(v)) {
            return Err(Error::Invalid("policy fraction"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseChange {
    pub base: f64,
    pub head: f64,
    pub regression: f64,
    pub weighted_absolute_change: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statistics {
    pub score: f64,
    pub improvement: f64,
    pub geometric_mean_ratio: f64,
    pub cases: BTreeMap<Name, CaseChange>,
    pub unknown_frequencies: Vec<Name>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exception {
    EquallySimple,
    SingleUnprotectedCase,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    DefaultPolicy,
    Exception,
    MissingPolicy,
    Correctness,
    PortfolioDeterioration,
    CaseRegression,
    HumanJudgment,
    RepeatRequired,
    InconsistentRepeat,
    Uncertainty,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    pub verdict: Verdict,
    pub reason: Reason,
    pub exception: Option<Exception>,
    pub human_judgment_required: bool,
    pub repeat_required: bool,
    pub incremental: Statistics,
    pub cumulative: Statistics,
    pub evidence_fingerprints: Vec<Fingerprint>,
}
/// These assertions must refer to retained, reviewable artifacts; they are never inferred from a green job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReviewEvidence {
    pub equally_simple_or_simpler: Option<Fingerprint>,
    pub regression_explanation: Option<Fingerprint>,
    pub no_family_recurrence: Option<Fingerprint>,
    pub profile_aggregate_conflict: bool,
}
pub struct Comparison<'a> {
    pub base: &'a VerifiedRun,
    pub head: &'a VerifiedRun,
    pub baseline: &'a BaselineValidation,
}
pub struct Round<'a> {
    pub incremental: Comparison<'a>,
    pub cumulative: Option<Comparison<'a>>,
}

fn compare(
    config: &FrozenConfig,
    comparison: &Comparison<'_>,
    base_sha: &CommitSha,
) -> Result<Statistics> {
    let cfg = config.config();
    let base = comparison.base.manifest();
    let head = comparison.head.manifest();
    if base.identity.source != *base_sha
        || base.identity.configuration != *config.fingerprint()
        || head.identity.configuration != *config.fingerprint()
        || base.identity.experiment != head.identity.experiment
        || base.identity.run == head.identity.run
        || head.identity.candidate.is_none()
        || !comparison
            .baseline
            .matches(config, base_sha, &head.identity.experiment)
    {
        return Err(Error::Mismatch("comparison provenance or baseline"));
    }
    let objective = match &cfg.policy {
        Policy::Declared(p) => p.objective,
        _ => Objective::Minimize,
    };
    let mut sum_base = 0.0;
    let mut sum_head = 0.0;
    let mut log_ratio = 0.0;
    let mut cases = BTreeMap::new();
    let mut unknown_frequencies = Vec::new();
    for (name, case) in &cfg.conditions.cases {
        let b = base.measurements[name].mean();
        let h = head.measurements[name].mean();
        if cfg.conditions.metric.metric == Metric::Ir && b != comparison.baseline.means[name] {
            return Err(Error::Mismatch("Ir baseline counts changed"));
        }
        let ratio = h / b;
        let regression = match objective {
            Objective::Minimize => ratio - 1.0,
            Objective::Maximize => 1.0 - ratio,
        };
        let change = (h - b) * case.weight();
        if !ratio.is_finite() || ratio <= 0.0 || !change.is_finite() {
            return Err(Error::Invalid("arithmetic overflow"));
        }
        sum_base += case.weight() * b;
        sum_head += case.weight() * h;
        log_ratio += ratio.ln() / cfg.conditions.cases.len() as f64;
        cases.insert(
            name.clone(),
            CaseChange {
                base: b,
                head: h,
                regression,
                weighted_absolute_change: change,
            },
        );
        if case.frequency.is_none() {
            unknown_frequencies.push(name.clone());
        }
    }
    let score = sum_head / sum_base;
    if !sum_base.is_finite() || !sum_head.is_finite() || !score.is_finite() || score <= 0.0 {
        return Err(Error::Invalid("portfolio overflow"));
    }
    let improvement = match objective {
        Objective::Minimize => 1.0 - score,
        Objective::Maximize => score - 1.0,
    };
    Ok(Statistics {
        score,
        improvement,
        geometric_mean_ratio: log_ratio.exp(),
        cases,
        unknown_frequencies,
    })
}
// Avoid classifying an exact decimal boundary below itself due to binary rounding.
fn at_least(value: f64, threshold: f64) -> bool {
    value >= threshold || (value - threshold).abs() <= 1e-12
}
fn positive(value: f64) -> bool {
    value > 1e-12
}
fn evaluate_round(
    config: &FrozenConfig,
    round: &Round<'_>,
    review: &ReviewEvidence,
) -> Result<Evaluation> {
    let cfg = config.config();
    let incremental = compare(config, &round.incremental, &cfg.incremental_base)?;
    let cumulative = if cfg.incremental_base == cfg.initial_base {
        if round.cumulative.is_some() {
            return Err(Error::Invalid("redundant cumulative comparison"));
        }
        incremental.clone()
    } else {
        let c = round
            .cumulative
            .as_ref()
            .ok_or(Error::Incomplete("cumulative comparison"))?;
        if c.head.digest() != round.incremental.head.digest() {
            return Err(Error::Mismatch("cumulative candidate"));
        }
        compare(config, c, &cfg.initial_base)?
    };
    let mut runs = vec![round.incremental.base, round.incremental.head];
    if let Some(c) = &round.cumulative {
        runs.push(c.base);
    }
    let mut result = Evaluation {
        verdict: Verdict::Regression,
        reason: Reason::CaseRegression,
        exception: None,
        human_judgment_required: false,
        repeat_required: false,
        incremental,
        cumulative,
        evidence_fingerprints: runs
            .iter()
            .map(|r| r.digest().clone())
            .chain(
                std::iter::once(&round.incremental)
                    .chain(round.cumulative.iter())
                    .flat_map(|c| c.baseline.evidence.iter().cloned()),
            )
            .collect(),
    };
    if runs.iter().any(|r| !r.correctness_passed()) {
        result.reason = Reason::Correctness;
        return Ok(result);
    }
    if cfg.policy == Policy::MeasurementsOnly {
        result.verdict = Verdict::NotEvaluated;
        result.reason = Reason::MissingPolicy;
        return Ok(result);
    }
    let both = [&result.incremental, &result.cumulative];
    let all_changes: Vec<_> = both.iter().flat_map(|s| s.cases.iter()).collect();
    match &cfg.policy {
        Policy::Ir(p) => {
            if runs.iter().any(|r| {
                r.manifest()
                    .measurements
                    .values()
                    .any(|s| s.relative_range() != 0.0)
            }) {
                result.verdict = Verdict::Inconclusive;
                result.reason = Reason::Uncertainty;
                return Ok(result);
            }
            let hard = all_changes.iter().any(|(name, c)| {
                at_least(c.regression, 0.03)
                    || (cfg.conditions.cases[*name].protected && at_least(c.regression, 0.01))
            });
            result.human_judgment_required = hard;
            let widespread = both.iter().any(|s| {
                s.cases
                    .values()
                    .filter(|c| at_least(c.regression, 0.01))
                    .count()
                    > 1
            });
            if result.incremental.improvement < -1e-12 || result.cumulative.improvement < -1e-12 {
                result.reason = Reason::PortfolioDeterioration;
            } else if widespread {
                result.reason = Reason::CaseRegression;
            } else if hard {
                result.reason = Reason::HumanJudgment;
            } else if both
                .iter()
                .all(|s| at_least(s.improvement, p.minimum_improvement))
                && all_changes
                    .iter()
                    .all(|(_, c)| !at_least(c.regression, p.maximum_case_regression))
            {
                result.verdict = Verdict::Pass;
                result.reason = Reason::DefaultPolicy;
            } else if p.allow_simple_exception
                && both.iter().all(|s| positive(s.improvement))
                && all_changes
                    .iter()
                    .all(|(_, c)| !at_least(c.regression, 0.01))
                && review.equally_simple_or_simpler.is_some()
            {
                result.verdict = Verdict::Pass;
                result.reason = Reason::Exception;
                result.exception = Some(Exception::EquallySimple);
            } else if p.allow_single_case_exception
                && both.iter().all(|s| at_least(s.improvement, 0.05))
                && review.regression_explanation.is_some()
                && review.no_family_recurrence.is_some()
            {
                let regressed: BTreeSet<_> = all_changes
                    .iter()
                    .filter(|(_, c)| at_least(c.regression, 0.01))
                    .map(|(n, _)| *n)
                    .collect();
                if regressed.len() == 1 {
                    let case = *regressed.first().expect("one case");
                    let family = &cfg.conditions.cases[case].family;
                    let recurrence = all_changes.iter().any(|(n, c)| {
                        *n != case
                            && &cfg.conditions.cases[*n].family == family
                            && positive(c.regression)
                    });
                    if !recurrence {
                        result.verdict = Verdict::Pass;
                        result.reason = Reason::Exception;
                        result.exception = Some(Exception::SingleUnprotectedCase);
                    }
                }
            }
            result.repeat_required = review.profile_aggregate_conflict
                || both.iter().any(|s| {
                    [0.0, p.minimum_improvement, 0.05]
                        .iter()
                        .any(|t| (s.improvement - t).abs() <= p.near_threshold + 1e-12)
                })
                || all_changes.iter().any(|(_, c)| {
                    [p.maximum_case_regression, 0.01, 0.03]
                        .iter()
                        .any(|t| (c.regression - t).abs() <= p.near_threshold + 1e-12)
                });
        }
        Policy::Declared(p) => {
            let valid = runs.iter().all(|run| {
                run.manifest().measurements.values().all(|s| {
                    s.values.len() >= p.repetitions as usize
                        && s.relative_range() <= p.uncertainty.maximum_relative_range
                })
            }) && std::iter::once(&round.incremental)
                .chain(round.cumulative.iter())
                .all(|c| {
                    c.baseline.minimum_repetitions >= p.repetitions as usize
                        && c.baseline
                            .variability
                            .values()
                            .all(|v| *v <= p.uncertainty.maximum_relative_range)
                });
            if !valid {
                result.verdict = Verdict::Inconclusive;
                result.reason = Reason::Uncertainty;
                return Ok(result);
            }
            let margin = p.uncertainty.decision_margin;
            if both
                .iter()
                .all(|s| at_least(s.improvement - margin, p.minimum_improvement))
                && all_changes
                    .iter()
                    .all(|(_, c)| c.regression + margin <= p.maximum_case_regression)
            {
                result.verdict = Verdict::Pass;
                result.reason = Reason::DefaultPolicy;
            }
            result.repeat_required = review.profile_aggregate_conflict
                || both.iter().any(|s| {
                    (s.improvement - margin - p.minimum_improvement).abs()
                        <= p.near_threshold + 1e-12
                })
                || all_changes.iter().any(|(_, c)| {
                    (c.regression + margin - p.maximum_case_regression).abs()
                        <= p.near_threshold + 1e-12
                });
        }
        Policy::MeasurementsOnly => unreachable!(),
    }
    // Review attestations must identify retained artifacts, not arbitrary strings.
    let artifacts: BTreeSet<_> = runs
        .iter()
        .flat_map(|r| r.manifest().artifacts.values().map(|a| &a.sha256))
        .collect();
    for proof in [
        &review.equally_simple_or_simpler,
        &review.regression_explanation,
        &review.no_family_recurrence,
    ]
    .into_iter()
    .flatten()
    {
        if !artifacts.contains(proof) {
            return Err(Error::Incomplete("review attestation artifact"));
        }
    }
    Ok(result)
}
/// A repeat is exactly one new round under the same frozen config and source identities.
pub fn evaluate(
    config: &FrozenConfig,
    first: &Round<'_>,
    repeat: Option<&Round<'_>>,
    review: &ReviewEvidence,
) -> Result<Evaluation> {
    config.validate()?;
    let mut initial = evaluate_round(config, first, review)?;
    if !initial.repeat_required && repeat.is_none() {
        return Ok(initial);
    }
    let Some(repeat) = repeat else {
        initial.verdict = Verdict::Inconclusive;
        initial.reason = Reason::RepeatRequired;
        return Ok(initial);
    };
    let mut next = evaluate_round(config, repeat, review)?;
    let first_runs = std::iter::once(&first.incremental)
        .chain(first.cumulative.iter())
        .flat_map(|c| [c.base, c.head]);
    let repeat_runs = std::iter::once(&repeat.incremental)
        .chain(repeat.cumulative.iter())
        .flat_map(|c| [c.base, c.head]);
    let mut seen = BTreeSet::new();
    for (a, b) in first_runs.zip(repeat_runs) {
        let ai = &a.manifest().identity;
        let bi = &b.manifest().identity;
        if ai.source != bi.source
            || ai.candidate != bi.candidate
            || ai.experiment != bi.experiment
            || ai.run == bi.run
        {
            return Err(Error::Mismatch("repeat identity"));
        }
        seen.insert(ai.run.clone());
    }
    for c in std::iter::once(&repeat.incremental).chain(repeat.cumulative.iter()) {
        if seen.contains(&c.base.manifest().identity.run)
            || seen.contains(&c.head.manifest().identity.run)
        {
            return Err(Error::Mismatch("reused repeat run"));
        }
    }
    let tolerance = match &config.config().policy {
        Policy::Ir(p) => p.repeat_tolerance,
        Policy::Declared(p) => p.uncertainty.repeat_tolerance,
        Policy::MeasurementsOnly => 0.0,
    };
    let consistent = initial.verdict == next.verdict
        && initial.exception == next.exception
        && initial.human_judgment_required == next.human_judgment_required
        && [&initial.incremental, &initial.cumulative]
            .into_iter()
            .zip([&next.incremental, &next.cumulative])
            .all(|(a, b)| {
                (a.improvement - b.improvement).abs() <= tolerance + 1e-12
                    && a.cases.iter().all(|(n, c)| {
                        (c.regression - b.cases[n].regression).abs() <= tolerance + 1e-12
                    })
            });
    next.evidence_fingerprints
        .splice(0..0, initial.evidence_fingerprints);
    next.repeat_required = false;
    if !consistent || review.profile_aggregate_conflict {
        next.verdict = Verdict::Inconclusive;
        next.reason = Reason::InconsistentRepeat;
    }
    Ok(next)
}
