// SPDX-License-Identifier: FSL-1.1-ALv2
mod evaluation_common;
use evaluation_common::*;
use perfine_api::evaluation::{evidence::*, policy::*, types::*};

fn review() -> ReviewEvidence {
    ReviewEvidence::default()
}
fn exception_review() -> ReviewEvidence {
    ReviewEvidence {
        equally_simple_or_simpler: Some(digest(b"profile")),
        regression_explanation: Some(digest(b"profile")),
        no_family_recurrence: Some(digest(b"profile")),
        profile_aggregate_conflict: false,
    }
}
#[test]
fn weights_use_absolute_portfolio_counts_not_mean_of_percentages() {
    let mut cfg = config();
    cfg.conditions
        .cases
        .get_mut(&name("large"))
        .unwrap()
        .frequency = Some(2.0);
    let f = Fixture::new(cfg, [1000.0, 100.0], [900.0, 100.0]);
    let r = f.evaluate(&review());
    assert_eq!(r.verdict, Verdict::Pass);
    assert!((r.incremental.score - 1900.0 / 2100.0).abs() < 1e-12);
    assert!((r.incremental.geometric_mean_ratio - 0.9_f64.sqrt()).abs() < 1e-12);
    assert_eq!(
        r.incremental.cases[&name("large")].weighted_absolute_change,
        -200.0
    );
    assert_eq!(r.incremental.unknown_frequencies, vec![name("small")]);
}
#[test]
fn all_default_and_exception_boundaries() {
    let f = Fixture::new(config(), [10000.0, 10000.0], [9800.0, 9800.0]);
    assert_eq!(f.evaluate(&review()).verdict, Verdict::Pass);
    let f = Fixture::new(config(), [10000.0, 10000.0], [9950.0, 9950.0]);
    assert_eq!(f.evaluate(&review()).verdict, Verdict::Regression);
    let r = f.evaluate(&exception_review());
    assert_eq!(r.verdict, Verdict::Pass);
    assert_eq!(r.exception, Some(Exception::EquallySimple));
    let f = Fixture::new(config(), [10000.0, 100.0], [9000.0, 102.0]);
    let r = f.evaluate(&exception_review());
    assert_eq!(r.verdict, Verdict::Pass);
    assert_eq!(r.exception, Some(Exception::SingleUnprotectedCase));
    let f = Fixture::new(config(), [10000.0, 100.0], [9600.0, 102.0]);
    assert_eq!(f.evaluate(&exception_review()).verdict, Verdict::Regression);
    let mut cfg = config();
    cfg.conditions
        .cases
        .get_mut(&name("small"))
        .unwrap()
        .protected = true;
    let f = Fixture::new(cfg, [10000.0, 10000.0], [8000.0, 10200.0]);
    let r = f.evaluate(&exception_review());
    assert_eq!(r.verdict, Verdict::Regression);
    assert!(r.human_judgment_required);
    let f = Fixture::new(config(), [10000.0, 10000.0], [8000.0, 10400.0]);
    let r = f.evaluate(&exception_review());
    assert!(r.human_judgment_required);
    assert_eq!(r.verdict, Verdict::Regression);
}
#[test]
fn exact_one_and_three_percent_regressions_require_repeat_and_cannot_sneak_below_threshold() {
    for small in [101.0, 103.0] {
        let f = Fixture::new(config(), [10000.0, 100.0], [8000.0, small]);
        let r = f.evaluate(&review());
        assert_eq!(r.verdict, Verdict::Inconclusive);
        assert!(r.repeat_required);
        assert_eq!(r.human_judgment_required, small == 103.0);
    }
}
#[test]
fn widespread_regression_and_deterioration_are_rejected() {
    let f = Fixture::new(config(), [10000.0, 10000.0], [10200.0, 10200.0]);
    assert_eq!(
        f.evaluate(&exception_review()).reason,
        Reason::PortfolioDeterioration
    );
    let f = Fixture::new(config(), [10000.0, 10000.0], [10000.0, 10000.0]);
    assert_ne!(f.evaluate(&exception_review()).verdict, Verdict::Pass);
}
#[test]
fn single_case_exception_rejects_family_recurrence() {
    let mut cfg = config();
    cfg.conditions.cases.get_mut(&name("large")).unwrap().family = name("same");
    cfg.conditions.cases.get_mut(&name("small")).unwrap().family = name("same");
    // An explicit family attestation cannot bypass two observed regressing cases.
    let f = Fixture::new(cfg, [10000.0, 10000.0], [10050.0, 10200.0]);
    assert_ne!(f.evaluate(&exception_review()).verdict, Verdict::Pass);
    let f = Fixture::new(config(), [10000.0, 100.0], [8000.0, 102.0]);
    let mut r = exception_review();
    r.no_family_recurrence = None;
    assert_ne!(f.evaluate(&r).verdict, Verdict::Pass);
}
#[test]
fn forged_review_proof_and_failed_correctness_never_pass() {
    let f = Fixture::new(config(), [10000.0, 100.0], [8000.0, 102.0]);
    let mut r = exception_review();
    r.regression_explanation = Some(digest(b"unretained"));
    assert!(evaluate(&f.cfg, &f.round(), None, &r).is_err());
    let mut m = f.head.manifest().clone();
    m.correctness
        .insert(name("project-tests"), CheckResult::NotRun);
    let h = verified(&f.cfg, m);
    let round = Round {
        incremental: Comparison {
            base: &f.base,
            head: &h,
            baseline: &f.baseline,
        },
        cumulative: None,
    };
    assert_eq!(
        evaluate(&f.cfg, &round, None, &review()).unwrap().reason,
        Reason::Correctness
    );
}
#[test]
fn near_threshold_repeat_is_new_evidence_and_disagreement_is_inconclusive() {
    let f = Fixture::new(config(), [10000.0, 10000.0], [9900.0, 9900.0]);
    assert!(f.evaluate(&review()).repeat_required);
    assert!(evaluate(&f.cfg, &f.round(), Some(&f.round()), &review()).is_err());
    let b = run(&f.cfg, &f.experiment, None, sha('a'), [10000.0, 10000.0]);
    for (value, expected) in [(9900.0, Verdict::Pass), (9990.0, Verdict::Inconclusive)] {
        let h = run(
            &f.cfg,
            &f.experiment,
            Some(f.candidate.clone()),
            sha('d'),
            [value, value],
        );
        let repeat = Round {
            incremental: Comparison {
                base: &b,
                head: &h,
                baseline: &f.baseline,
            },
            cumulative: None,
        };
        let r = evaluate(&f.cfg, &f.round(), Some(&repeat), &review()).unwrap();
        assert_eq!(r.verdict, expected);
        assert!(!r.repeat_required);
    }
}
#[test]
fn unresolved_profile_conflict_remains_inconclusive_after_repeat() {
    let f = Fixture::new(config(), [10000.0, 10000.0], [9000.0, 9000.0]);
    let mut r = review();
    r.profile_aggregate_conflict = true;
    assert!(f.evaluate(&r).repeat_required);
    let b = run(&f.cfg, &f.experiment, None, sha('a'), [10000.0, 10000.0]);
    let h = run(
        &f.cfg,
        &f.experiment,
        Some(f.candidate.clone()),
        sha('d'),
        [9000.0, 9000.0],
    );
    let repeat = Round {
        incremental: Comparison {
            base: &b,
            head: &h,
            baseline: &f.baseline,
        },
        cumulative: None,
    };
    assert_eq!(
        evaluate(&f.cfg, &f.round(), Some(&repeat), &r)
            .unwrap()
            .verdict,
        Verdict::Inconclusive
    );
}
#[test]
fn dependent_candidates_require_both_bases_and_same_head() {
    let mut cfg = config();
    cfg.incremental_base = sha('e');
    let f = Fixture::new(cfg, [10000.0, 10000.0], [9000.0, 9000.0]);
    assert!(evaluate(&f.cfg, &f.round(), None, &review()).is_err());
    let b = run(&f.cfg, &f.experiment, None, sha('a'), [8000.0, 8000.0]);
    let b2 = run(&f.cfg, &f.experiment, None, sha('a'), [8000.0, 8000.0]);
    let baseline = validate_baseline(&f.cfg, &b, &b2).unwrap();
    let round = Round {
        incremental: f.round().incremental,
        cumulative: Some(Comparison {
            base: &b,
            head: &f.head,
            baseline: &baseline,
        }),
    };
    assert_eq!(
        evaluate(&f.cfg, &round, None, &review()).unwrap().verdict,
        Verdict::Regression
    );
    let h = run(
        &f.cfg,
        &f.experiment,
        Some(f.candidate.clone()),
        sha('f'),
        [7000.0, 7000.0],
    );
    let bad = Round {
        incremental: f.round().incremental,
        cumulative: Some(Comparison {
            base: &b,
            head: &h,
            baseline: &baseline,
        }),
    };
    assert!(evaluate(&f.cfg, &bad, None, &review()).is_err());
}
fn timing(policy: Policy) -> Config {
    let mut cfg = config();
    cfg.conditions.metric = MetricSpec {
        metric: Metric::Time,
        unit: name("nanoseconds"),
    };
    cfg.policy = policy;
    cfg
}
fn declared() -> Policy {
    Policy::Declared(DeclaredPolicy {
        objective: Objective::Minimize,
        minimum_improvement: 0.01,
        maximum_case_regression: 0.01,
        repetitions: 3,
        uncertainty: Uncertainty {
            maximum_relative_range: 0.05,
            decision_margin: 0.001,
            repeat_tolerance: 0.005,
        },
        near_threshold: 0.001,
    })
}
#[test]
fn non_ir_without_policy_reports_measurements_without_pass() {
    let f = Fixture::new(
        timing(Policy::MeasurementsOnly),
        [100.0, 100.0],
        [50.0, 50.0],
    );
    assert_eq!(f.evaluate(&review()).verdict, Verdict::NotEvaluated);
    let f = Fixture::new(timing(declared()), [100.0, 100.0], [50.0, 50.0]);
    assert_eq!(f.evaluate(&review()).reason, Reason::Uncertainty);
}
#[test]
fn timing_baseline_measures_variability_and_enforces_declared_uncertainty() {
    let cfg = timing(declared()).freeze().unwrap();
    let id = ResourceId::generate();
    let cid = ResourceId::generate();
    let make = |candidate: Option<ResourceId>, source: CommitSha, values: Vec<f64>| {
        let mut m = manifest(&cfg, &id, candidate, source, [100.0, 100.0]);
        for s in m.measurements.values_mut() {
            s.values = values.clone();
        }
        verified(&cfg, m)
    };
    let a = make(None, sha('a'), vec![99.0, 100.0, 101.0]);
    let b = make(None, sha('a'), vec![100.0, 101.0, 102.0]);
    let baseline = validate_baseline(&cfg, &a, &b).unwrap();
    assert!(baseline.variability().values().all(|v| *v > 0.0));
    for (values, verdict) in [
        (vec![89.0, 90.0, 91.0], Verdict::Pass),
        (vec![50.0, 90.0, 130.0], Verdict::Inconclusive),
    ] {
        let h = make(Some(cid.clone()), sha('d'), values);
        let round = Round {
            incremental: Comparison {
                base: &a,
                head: &h,
                baseline: &baseline,
            },
            cumulative: None,
        };
        assert_eq!(
            evaluate(&cfg, &round, None, &review()).unwrap().verdict,
            verdict
        );
    }
}
#[test]
fn arithmetic_overflow_is_an_error_not_a_pass() {
    let mut cfg = config();
    cfg.conditions
        .cases
        .get_mut(&name("large"))
        .unwrap()
        .frequency = Some(f64::MAX);
    let f = Fixture::new(cfg, [100.0, 100.0], [50.0, 50.0]);
    assert!(evaluate(&f.cfg, &f.round(), None, &review()).is_err());
}

#[test]
fn baseline_counts_cannot_be_replaced_after_validation() {
    let f = Fixture::new(config(), [100.0, 100.0], [90.0, 90.0]);
    let b = run(&f.cfg, &f.experiment, None, sha('a'), [200.0, 200.0]);
    let round = Round {
        incremental: Comparison {
            base: &b,
            head: &f.head,
            baseline: &f.baseline,
        },
        cumulative: None,
    };
    assert!(evaluate(&f.cfg, &round, None, &review()).is_err());
}
#[test]
fn maximizing_throughput_uses_declared_direction_and_repetitions() {
    let mut cfg = timing(declared());
    cfg.conditions.metric = MetricSpec {
        metric: Metric::Throughput,
        unit: name("operations/second"),
    };
    if let Policy::Declared(p) = &mut cfg.policy {
        p.objective = Objective::Maximize;
    }
    let cfg = cfg.freeze().unwrap();
    let id = ResourceId::generate();
    let make = |candidate, source, value| {
        let mut m = manifest(&cfg, &id, candidate, source, [value, value]);
        for s in m.measurements.values_mut() {
            s.values = vec![value; 3];
        }
        verified(&cfg, m)
    };
    let a = make(None, sha('a'), 100.0);
    let b = make(None, sha('a'), 100.0);
    let baseline = validate_baseline(&cfg, &a, &b).unwrap();
    for (value, expected) in [(110.0, Verdict::Pass), (90.0, Verdict::Regression)] {
        let h = make(Some(ResourceId::generate()), sha('d'), value);
        let r = Round {
            incremental: Comparison {
                base: &a,
                head: &h,
                baseline: &baseline,
            },
            cumulative: None,
        };
        assert_eq!(
            evaluate(&cfg, &r, None, &review()).unwrap().verdict,
            expected
        );
    }
}
#[test]
fn contradictory_ir_samples_are_inconclusive_even_if_the_average_improves() {
    let f = Fixture::new(config(), [100.0, 100.0], [90.0, 90.0]);
    let mut m = f.head.manifest().clone();
    m.measurements.get_mut(&name("large")).unwrap().values = vec![70.0, 110.0];
    let h = verified(&f.cfg, m);
    let r = Round {
        incremental: Comparison {
            base: &f.base,
            head: &h,
            baseline: &f.baseline,
        },
        cumulative: None,
    };
    assert_eq!(
        evaluate(&f.cfg, &r, None, &review()).unwrap().verdict,
        Verdict::Inconclusive
    );
}

#[test]
fn broad_regressions_cannot_hide_behind_a_large_winning_case() {
    let mut cfg = config();
    cfg.conditions
        .cases
        .insert(name("third"), cfg.conditions.cases[&name("small")].clone());
    let cfg = cfg.freeze().unwrap();
    let id = ResourceId::generate();
    let make = |candidate, source, large, small| {
        let mut m = manifest(&cfg, &id, candidate, source, [large, small]);
        m.measurements.insert(
            name("third"),
            Samples {
                values: vec![small],
            },
        );
        m.profile_paths.insert(
            name("third"),
            std::collections::BTreeSet::from([name("src/hot.rs")]),
        );
        verified(&cfg, m)
    };
    let a = make(None, sha('a'), 10000.0, 100.0);
    let b = make(None, sha('a'), 10000.0, 100.0);
    let baseline = validate_baseline(&cfg, &a, &b).unwrap();
    let h = make(Some(ResourceId::generate()), sha('d'), 8000.0, 102.0);
    let r = Round {
        incremental: Comparison {
            base: &a,
            head: &h,
            baseline: &baseline,
        },
        cumulative: None,
    };
    let result = evaluate(&cfg, &r, None, &exception_review()).unwrap();
    assert!(result.incremental.improvement > 0.05);
    assert_eq!(result.verdict, Verdict::Regression);
    assert_eq!(result.reason, Reason::CaseRegression);
}

#[test]
fn declared_uncertainty_rejects_baseline_drift_and_under_sampled_baseline() {
    let cfg = timing(declared()).freeze().unwrap();
    let id = ResourceId::generate();
    let make = |candidate, source, value, repetitions| {
        let mut m = manifest(&cfg, &id, candidate, source, [value, value]);
        for s in m.measurements.values_mut() {
            s.values = vec![value; repetitions];
        }
        verified(&cfg, m)
    };
    let a = make(None, sha('a'), 100.0, 3);
    let b = make(None, sha('a'), 100.0, 3);
    let baseline = validate_baseline(&cfg, &a, &b).unwrap();
    let drift = make(None, sha('a'), 200.0, 3);
    let h = make(Some(ResourceId::generate()), sha('d'), 90.0, 3);
    let r = Round {
        incremental: Comparison {
            base: &drift,
            head: &h,
            baseline: &baseline,
        },
        cumulative: None,
    };
    assert_eq!(
        evaluate(&cfg, &r, None, &review()).unwrap().verdict,
        Verdict::Inconclusive
    );
    let sparse = make(None, sha('a'), 100.0, 1);
    let baseline = validate_baseline(&cfg, &a, &sparse).unwrap();
    let r = Round {
        incremental: Comparison {
            base: &a,
            head: &h,
            baseline: &baseline,
        },
        cumulative: None,
    };
    assert_eq!(
        evaluate(&cfg, &r, None, &review()).unwrap().verdict,
        Verdict::Inconclusive
    );
}
