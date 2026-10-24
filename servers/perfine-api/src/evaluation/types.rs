// SPDX-License-Identifier: FSL-1.1-ALv2
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("immutable provenance mismatch: {0}")]
    Mismatch(&'static str),
    #[error("incomplete evidence: {0}")]
    Incomplete(&'static str),
    #[error("operation is not valid in this state")]
    State,
    #[error("resource limit or deadline reached")]
    Limit,
    #[error("experiment was deleted")]
    Deleted,
    #[error("request ID reused for different content")]
    RequestConflict,
    #[error("storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization: {0}")]
    Json(#[from] serde_json::Error),
}

macro_rules! validated_string {
    ($name:ident, $check:expr, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if ($check)(&value) {
                    Ok(Self(value))
                } else {
                    Err(Error::Invalid($label))
                }
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(value: String) -> Result<Self> {
                Self::new(value)
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}
fn hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && value.bytes().any(|c| c != b'0')
}
validated_string!(CommitSha, |s: &str| hex(s, &[40, 64]), "commit SHA");
validated_string!(Fingerprint, |s: &str| hex(s, &[64]), "SHA-256 fingerprint");
validated_string!(
    Name,
    |s: &str| !s.trim().is_empty() && !s.chars().any(char::is_control),
    "name"
);
validated_string!(
    ResourceId,
    |s: &str| uuid::Uuid::parse_str(s).is_ok_and(|id| id.get_version_num() == 7
        && id.get_variant() == uuid::Variant::RFC4122
        && id.to_string() == *s),
    "UUID v7"
);
impl ResourceId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
}
validated_string!(
    RequestId,
    |s: &str| !s.is_empty() && s.len() <= 256 && s.bytes().all(|c| c.is_ascii_graphic()),
    "request ID"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Version;
impl TryFrom<u32> for Version {
    type Error = Error;
    fn try_from(v: u32) -> Result<Self> {
        if v == 1 {
            Ok(Self)
        } else {
            Err(Error::Invalid("schema major version"))
        }
    }
}
impl From<Version> for u32 {
    fn from(_: Version) -> u32 {
        1
    }
}

pub fn digest(bytes: &[u8]) -> Fingerprint {
    Fingerprint(format!("{:x}", Sha256::digest(bytes)))
}
/// Struct field order and BTreeMap key order define canonical v1 fingerprints.
pub fn fingerprint(value: &impl Serialize) -> Result<Fingerprint> {
    Ok(digest(&serde_json::to_vec(value)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Ir,
    Time,
    Throughput,
    Memory,
    Custom,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricSpec {
    pub metric: Metric,
    pub unit: Name,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub family: Name,
    pub frequency: Option<f64>,
    pub protected: bool,
}
impl Case {
    pub fn weight(&self) -> f64 {
        self.frequency.unwrap_or(1.0)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub os: Name,
    pub architecture: Name,
    pub runner: Name,
    pub hardware: BTreeMap<Name, Name>,
    pub variables: BTreeMap<Name, Name>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conditions {
    pub harness: CommitSha,
    pub workflow: CommitSha,
    pub inputs: BTreeMap<Name, Fingerprint>,
    pub tools: BTreeMap<Name, Name>,
    pub toolchain: Name,
    pub build_options: Vec<String>,
    pub runtime_flags: Vec<String>,
    pub environment: Environment,
    pub metric: MetricSpec,
    pub cases: BTreeMap<Name, Case>,
    pub correctness_checks: Vec<Name>,
}
impl Conditions {
    pub fn validate(&self) -> Result<()> {
        if self.inputs.is_empty()
            || self.tools.is_empty()
            || self.cases.is_empty()
            || self.correctness_checks.is_empty()
            || self.environment.hardware.is_empty()
        {
            return Err(Error::Invalid("empty measurement conditions"));
        }
        let unique: std::collections::BTreeSet<_> = self.correctness_checks.iter().collect();
        if unique.len() != self.correctness_checks.len() {
            return Err(Error::Invalid("duplicate correctness checks"));
        }
        if self
            .cases
            .values()
            .any(|c| !c.weight().is_finite() || c.weight() <= 0.0)
        {
            return Err(Error::Invalid("frequency"));
        }
        if self.metric.metric == Metric::Ir && self.metric.unit.as_str() != "instructions" {
            return Err(Error::Invalid("Ir unit"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Regression,
    Inconclusive,
    NotEvaluated,
}
