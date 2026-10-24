// SPDX-License-Identifier: Apache-2.0
use crate::{policy::Policy, types::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: Version,
    pub repository: Name,
    pub initial_base: CommitSha,
    pub incremental_base: CommitSha,
    pub conditions: Conditions,
    pub policy: Policy,
    pub limits: crate::lifecycle::Limits,
    pub workload_confirmation: Name,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        self.conditions.validate()?;
        self.policy.validate(self.conditions.metric.metric)?;
        self.limits.validate()?;
        Ok(())
    }
    pub fn freeze(self) -> Result<FrozenConfig> {
        self.validate()?;
        let fingerprint = fingerprint(&self)?;
        Ok(FrozenConfig {
            config: self,
            fingerprint,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrozenConfig {
    config: Config,
    fingerprint: Fingerprint,
}
impl FrozenConfig {
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }
    pub fn validate(&self) -> Result<()> {
        self.config.validate()?;
        if fingerprint(&self.config)? != self.fingerprint {
            return Err(Error::Mismatch("configuration fingerprint"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIdentity {
    pub provider: Name,
    pub workflow_run: Name,
    pub attempt: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIdentity {
    pub experiment: ResourceId,
    pub candidate: Option<ResourceId>,
    pub run: ResourceId,
    pub external_execution: Option<ExecutionIdentity>,
    pub source: CommitSha,
    pub configuration: Fingerprint,
    pub conditions: Fingerprint,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Complete,
    Incomplete,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Measurement,
    Profile,
    Correctness,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub sha256: Fingerprint,
    pub size: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Samples {
    pub values: Vec<f64>,
}
impl Samples {
    pub fn mean(&self) -> f64 {
        self.values
            .iter()
            .map(|v| v / self.values.len() as f64)
            .sum()
    }
    pub fn relative_range(&self) -> f64 {
        let min = self.values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = self.values.iter().copied().fold(0.0, f64::max);
        (max - min) / self.mean()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult {
    Passed,
    Failed,
    NotRun,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: Version,
    pub identity: RunIdentity,
    pub conditions: Conditions,
    pub policy: Policy,
    pub completion: Completion,
    pub measurements: BTreeMap<Name, Samples>,
    pub correctness: BTreeMap<Name, CheckResult>,
    pub artifacts: BTreeMap<Name, Artifact>,
    /// Each case must identify the optimized source paths observed in its profile.
    pub profile_paths: BTreeMap<Name, BTreeSet<Name>>,
}
/// The caller resolves local, GitHub, or hosted bytes. Unavailable bytes fail closed.
pub trait ArtifactReader {
    fn read(&self, name: &Name) -> Result<Vec<u8>>;
}
impl<F: Fn(&Name) -> Result<Vec<u8>>> ArtifactReader for F {
    fn read(&self, name: &Name) -> Result<Vec<u8>> {
        self(name)
    }
}
#[derive(Debug, Clone)]
pub struct VerifiedRun {
    manifest: Manifest,
    digest: Fingerprint,
}
impl VerifiedRun {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn digest(&self) -> &Fingerprint {
        &self.digest
    }
    pub fn correctness_passed(&self) -> bool {
        self.manifest
            .correctness
            .values()
            .all(|v| *v == CheckResult::Passed)
    }
}
/// Verification checks observations against a separately scheduled identity.
/// Adapters are responsible for parsing raw bytes into faithful observations.
pub fn verify(
    config: &FrozenConfig,
    expected: &RunIdentity,
    manifest: Manifest,
    reader: &impl ArtifactReader,
) -> Result<VerifiedRun> {
    config.validate()?;
    let cfg = config.config();
    if expected
        .external_execution
        .as_ref()
        .is_some_and(|run| run.attempt == 0)
    {
        return Err(Error::Invalid("external workflow run attempt"));
    }
    if manifest.identity != *expected
        || expected.configuration != *config.fingerprint()
        || expected.conditions != fingerprint(&cfg.conditions)?
    {
        return Err(Error::Mismatch("run identity"));
    }
    if manifest.conditions != cfg.conditions || manifest.policy != cfg.policy {
        return Err(Error::Mismatch("measurement conditions or policy"));
    }
    if manifest.completion != Completion::Complete {
        return Err(Error::Incomplete("run"));
    }
    let cases: BTreeSet<_> = cfg.conditions.cases.keys().collect();
    if manifest.measurements.keys().collect::<BTreeSet<_>>() != cases
        || manifest.profile_paths.keys().collect::<BTreeSet<_>>() != cases
    {
        return Err(Error::Mismatch("case set"));
    }
    if manifest.profile_paths.values().any(BTreeSet::is_empty) {
        return Err(Error::Incomplete("profile coverage"));
    }
    if manifest.correctness.keys().collect::<BTreeSet<_>>()
        != cfg.conditions.correctness_checks.iter().collect()
    {
        return Err(Error::Incomplete("correctness checks"));
    }
    for samples in manifest.measurements.values() {
        if samples.values.is_empty() || samples.values.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err(Error::Invalid("positive finite measurements"));
        }
        // f64 cannot represent all integers above 2^53; silently rounded Ir is not evidence.
        if cfg.conditions.metric.metric == Metric::Ir
            && samples
                .values
                .iter()
                .any(|v| v.fract() != 0.0 || *v > 9_007_199_254_740_992.0)
        {
            return Err(Error::Invalid("exact positive Ir counts"));
        }
        if !samples.mean().is_finite() {
            return Err(Error::Invalid("measurement overflow"));
        }
    }
    let kinds: BTreeSet<_> = manifest.artifacts.values().map(|a| a.kind).collect();
    if kinds
        != BTreeSet::from([
            ArtifactKind::Measurement,
            ArtifactKind::Profile,
            ArtifactKind::Correctness,
        ])
    {
        return Err(Error::Incomplete("required artifacts"));
    }
    for (name, artifact) in &manifest.artifacts {
        let path = std::path::Path::new(name.as_str());
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || name.as_str().contains('\\')
        {
            return Err(Error::Invalid("artifact relative path"));
        }
        let bytes = reader.read(name)?;
        if bytes.is_empty()
            || bytes.len() as u64 != artifact.size
            || digest(&bytes) != artifact.sha256
        {
            return Err(Error::Mismatch("artifact content"));
        }
    }
    let digest = fingerprint(&manifest)?;
    Ok(VerifiedRun { manifest, digest })
}
#[derive(Debug, Clone)]
pub struct BaselineValidation {
    config: Fingerprint,
    source: CommitSha,
    experiment: ResourceId,
    runs: [ResourceId; 2],
    pub(crate) variability: BTreeMap<Name, f64>,
    pub(crate) means: BTreeMap<Name, f64>,
    pub(crate) minimum_repetitions: usize,
    pub(crate) evidence: [Fingerprint; 2],
}
impl BaselineValidation {
    pub fn variability(&self) -> &BTreeMap<Name, f64> {
        &self.variability
    }
    pub(crate) fn matches(
        &self,
        config: &FrozenConfig,
        source: &CommitSha,
        experiment: &ResourceId,
    ) -> bool {
        self.config == *config.fingerprint()
            && self.source == *source
            && self.experiment == *experiment
    }
    pub fn runs(&self) -> &[ResourceId; 2] {
        &self.runs
    }
}
pub fn validate_baseline(
    config: &FrozenConfig,
    a: &VerifiedRun,
    b: &VerifiedRun,
) -> Result<BaselineValidation> {
    config.validate()?;
    let left = a.manifest();
    let right = b.manifest();
    if left.identity.source != right.identity.source
        || left.identity.configuration != *config.fingerprint()
        || right.identity.configuration != *config.fingerprint()
        || left.identity.experiment != right.identity.experiment
        || left.identity.run == right.identity.run
        || left.identity.candidate.is_some()
        || right.identity.candidate.is_some()
    {
        return Err(Error::Mismatch("same-SHA baseline"));
    }
    if left.identity.source != config.config().initial_base
        && left.identity.source != config.config().incremental_base
    {
        return Err(Error::Mismatch("baseline source"));
    }
    if !a.correctness_passed() || !b.correctness_passed() {
        return Err(Error::Incomplete("baseline correctness"));
    }
    let mut variability = BTreeMap::new();
    for (case, samples) in &left.measurements {
        let other = &right.measurements[case];
        let variation = (other.mean() / samples.mean() - 1.0)
            .abs()
            .max(samples.relative_range())
            .max(other.relative_range());
        if !variation.is_finite() {
            return Err(Error::Invalid("baseline variability"));
        }
        if config.config().conditions.metric.metric == Metric::Ir && variation != 0.0 {
            return Err(Error::Mismatch("deterministic same-SHA Ir"));
        }
        variability.insert(case.clone(), variation);
    }
    Ok(BaselineValidation {
        config: config.fingerprint().clone(),
        source: left.identity.source.clone(),
        experiment: left.identity.experiment.clone(),
        runs: [left.identity.run.clone(), right.identity.run.clone()],
        means: left
            .measurements
            .iter()
            .map(|(n, s)| (n.clone(), s.mean()))
            .collect(),
        minimum_repetitions: left
            .measurements
            .values()
            .chain(right.measurements.values())
            .map(|s| s.values.len())
            .min()
            .unwrap_or(0),
        evidence: [a.digest().clone(), b.digest().clone()],
        variability,
    })
}
