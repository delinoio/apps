// SPDX-License-Identifier: Apache-2.0
#![allow(
    dead_code,
    reason = "Shared fixtures expose helpers used by different integration suites"
)]
use perfine_core::{evidence::*, lifecycle::*, policy::*, types::*};
use std::collections::{BTreeMap, BTreeSet};
pub fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}
pub fn sha(n: char) -> CommitSha {
    CommitSha::new(n.to_string().repeat(40)).unwrap()
}
pub fn request() -> RequestId {
    RequestId::new(ResourceId::generate().as_str()).unwrap()
}
pub fn config() -> Config {
    Config {
        schema_version: Version,
        repository: name("fixture/repository"),
        initial_base: sha('a'),
        incremental_base: sha('a'),
        workload_confirmation: name("user confirmed representative workload"),
        limits: Limits {
            candidate_count: 10,
            simultaneous_candidates: 5,
            start_duration_seconds: 100,
            candidate_duration_seconds: 30,
        },
        policy: Policy::Ir(IrPolicy::default()),
        conditions: Conditions {
            harness: sha('b'),
            workflow: sha('c'),
            inputs: BTreeMap::from([(name("input"), digest(b"input"))]),
            tools: BTreeMap::from([(name("callgrind"), name("3.25"))]),
            toolchain: name("rustc-fixture"),
            build_options: vec!["release".into()],
            runtime_flags: vec![],
            environment: Environment {
                os: name("linux"),
                architecture: name("x86_64"),
                runner: name("fixture"),
                hardware: BTreeMap::from([(name("cpu"), name("fixture-cpu"))]),
                variables: BTreeMap::new(),
            },
            metric: MetricSpec {
                metric: Metric::Ir,
                unit: name("instructions"),
            },
            cases: BTreeMap::from([
                (
                    name("large"),
                    Case {
                        family: name("large-family"),
                        frequency: Some(1.0),
                        protected: false,
                    },
                ),
                (
                    name("small"),
                    Case {
                        family: name("small-family"),
                        frequency: None,
                        protected: false,
                    },
                ),
            ]),
            correctness_checks: vec![name("project-tests")],
        },
    }
}
pub fn manifest(
    cfg: &FrozenConfig,
    experiment: &ResourceId,
    candidate: Option<ResourceId>,
    source: CommitSha,
    values: [f64; 2],
) -> Manifest {
    Manifest {
        schema_version: Version,
        identity: RunIdentity {
            experiment: experiment.clone(),
            candidate,
            run: ResourceId::generate(),
            external_execution: None,
            source,
            configuration: cfg.fingerprint().clone(),
            conditions: fingerprint(&cfg.config().conditions).unwrap(),
        },
        conditions: cfg.config().conditions.clone(),
        policy: cfg.config().policy.clone(),
        completion: Completion::Complete,
        measurements: BTreeMap::from([
            (
                name("large"),
                Samples {
                    values: vec![values[0]],
                },
            ),
            (
                name("small"),
                Samples {
                    values: vec![values[1]],
                },
            ),
        ]),
        correctness: BTreeMap::from([(name("project-tests"), CheckResult::Passed)]),
        artifacts: [
            ("measurements", ArtifactKind::Measurement),
            ("profile", ArtifactKind::Profile),
            ("tests", ArtifactKind::Correctness),
        ]
        .into_iter()
        .map(|(n, k)| {
            (
                name(n),
                Artifact {
                    kind: k,
                    sha256: digest(n.as_bytes()),
                    size: n.len() as u64,
                },
            )
        })
        .collect(),
        profile_paths: BTreeMap::from([
            (name("large"), BTreeSet::from([name("src/hot.rs")])),
            (name("small"), BTreeSet::from([name("src/hot.rs")])),
        ]),
    }
}
pub fn verified(cfg: &FrozenConfig, m: Manifest) -> VerifiedRun {
    verify(cfg, &m.identity.clone(), m, &|n: &Name| {
        Ok(n.as_str().as_bytes().to_vec())
    })
    .unwrap()
}
pub fn run(
    cfg: &FrozenConfig,
    experiment: &ResourceId,
    candidate: Option<ResourceId>,
    source: CommitSha,
    values: [f64; 2],
) -> VerifiedRun {
    verified(cfg, manifest(cfg, experiment, candidate, source, values))
}
pub struct Fixture {
    pub cfg: FrozenConfig,
    pub experiment: ResourceId,
    pub candidate: ResourceId,
    pub base: VerifiedRun,
    pub head: VerifiedRun,
    pub baseline: BaselineValidation,
}
impl Fixture {
    pub fn new(cfg: Config, base: [f64; 2], head: [f64; 2]) -> Self {
        let cfg = cfg.freeze().unwrap();
        let experiment = ResourceId::generate();
        let candidate = ResourceId::generate();
        let b = run(
            &cfg,
            &experiment,
            None,
            cfg.config().incremental_base.clone(),
            base,
        );
        let b2 = run(
            &cfg,
            &experiment,
            None,
            cfg.config().incremental_base.clone(),
            base,
        );
        let baseline = validate_baseline(&cfg, &b, &b2).unwrap();
        let h = run(&cfg, &experiment, Some(candidate.clone()), sha('d'), head);
        Self {
            cfg,
            experiment,
            candidate,
            base: b,
            head: h,
            baseline,
        }
    }
    pub fn round(&self) -> Round<'_> {
        Round {
            incremental: Comparison {
                base: &self.base,
                head: &self.head,
                baseline: &self.baseline,
            },
            cumulative: None,
        }
    }
    pub fn evaluate(&self, review: &ReviewEvidence) -> Evaluation {
        evaluate(&self.cfg, &self.round(), None, review).unwrap()
    }
}
pub fn workspace(n: usize) -> Workspace {
    Workspace {
        directory: name(&format!("/tmp/perfine/candidate-{n}")),
        branch: name(&format!("perfine-{n}")),
        build_directory: name(&format!("/tmp/perfine/build-{n}")),
    }
}
pub fn start<S: perfine_core::storage::Store>(
    engine: &Engine<S>,
    f: &Fixture,
    id: ResourceId,
    n: usize,
    now: u64,
) -> perfine_core::Result<Receipt> {
    engine.start_candidate(
        &request(),
        &f.experiment,
        id,
        sha('d'),
        workspace(n),
        &f.base,
        &f.baseline,
        &BTreeSet::from([name("src/hot.rs")]),
        now,
    )
}
