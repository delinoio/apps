// SPDX-License-Identifier: Apache-2.0
//! Public Connect JSON types. Preparation, rather than JSON hashing, owns digests.
use perfine_core::types::{CommitSha, Fingerprint, ResourceId, Version};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $wire)] $variant),+ }
    };
}
wire_enum!(Language { Rust => "LANGUAGE_RUST", C => "LANGUAGE_C", Cpp => "LANGUAGE_CPP", Zig => "LANGUAGE_ZIG", Go => "LANGUAGE_GO", JavaScript => "LANGUAGE_JAVASCRIPT" });
wire_enum!(EngineMetricKind { Ir => "ENGINE_METRIC_KIND_IR", Time => "ENGINE_METRIC_KIND_TIME", Throughput => "ENGINE_METRIC_KIND_THROUGHPUT", Memory => "ENGINE_METRIC_KIND_MEMORY", Custom => "ENGINE_METRIC_KIND_CUSTOM" });
wire_enum!(Objective { Minimize => "METRIC_OBJECTIVE_MINIMIZE", Maximize => "METRIC_OBJECTIVE_MAXIMIZE" });
wire_enum!(PolicyKind { Callgrind => "POLICY_KIND_CALLGRIND_IR", Declared => "POLICY_KIND_DECLARED", MeasurementsOnly => "POLICY_KIND_MEASUREMENTS_ONLY" });
wire_enum!(Exception { BelowMinimum => "POLICY_EXCEPTION_BELOW_MINIMUM_IMPROVEMENT", SingleRegression => "POLICY_EXCEPTION_SINGLE_UNPROTECTED_REGRESSION", HumanJudgment => "POLICY_EXCEPTION_HUMAN_JUDGMENT_REQUIRED" });
wire_enum!(FrequencySource { Declared => "FREQUENCY_SOURCE_DECLARED", AssumedOne => "FREQUENCY_SOURCE_ASSUMED_ONE" });

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tool {
    name: String,
    version: String,
    #[serde(default)]
    configuration_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    operating_system: String,
    architecture: String,
    runner: String,
    runtime: String,
    #[serde(default)]
    runtime_version: String,
    #[serde(default)]
    build_options: Vec<String>,
    #[serde(default)]
    runtime_flags: Vec<String>,
    tools: Vec<Tool>,
    #[serde(rename = "environmentSha256")]
    checksum: Fingerprint,
    hardware: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    name: String,
    sha256: Fingerprint,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Case {
    id: String,
    family: String,
    frequency: f64,
    frequency_source: FrequencySource,
    #[serde(default)]
    #[serde(rename = "protectedCase")]
    protected: bool,
    #[serde(default)]
    inputs: Vec<Input>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metric {
    name: String,
    unit: String,
    objective: Objective,
    engine_kind: EngineMetricKind,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Uncertainty {
    engine_schema_version: Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    maximum_relative_range: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision_margin: Option<f64>,
    repeat_tolerance: f64,
    near_threshold: f64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    schema_version: Version,
    kind: PolicyKind,
    metric: Metric,
    #[serde(default)]
    minimum_improvement: f64,
    #[serde(default)]
    maximum_case_regression: f64,
    #[serde(default)]
    repetitions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    uncertainty: Option<Uncertainty>,
    #[serde(default)]
    allowed_exceptions: Vec<Exception>,
    #[serde(rename = "policySha256")]
    checksum: Fingerprint,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    candidate_count: u32,
    simultaneous_candidates: u32,
    start_deadline: Option<String>,
    candidate_timeout: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Specification {
    schema_version: Version,
    pub base_sha: CommitSha,
    pub initial_base_sha: CommitSha,
    pub harness_sha: CommitSha,
    language: Language,
    environment: Environment,
    cases: Vec<Case>,
    policy: Policy,
    limits: Limits,
    pub fingerprint_sha256: Fingerprint,
    workload_representativeness_confirmed: bool,
    #[serde(default)]
    parent_candidate_ids: Vec<ResourceId>,
    pub workflow_sha: CommitSha,
    inputs: Vec<Input>,
    correctness_checks: Vec<String>,
    workload_confirmation: String,
    start_duration: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{Metric, Uncertainty};
    #[test]
    fn final_protocol_requires_engine_metric_and_exact_uncertainty() {
        let metric = serde_json::json!({"name":"Ir","unit":"instructions","objective":"METRIC_OBJECTIVE_MINIMIZE","engineKind":"ENGINE_METRIC_KIND_IR"});
        assert!(serde_json::from_value::<Metric>(metric.clone()).is_ok());
        let mut missing = metric;
        missing.as_object_mut().unwrap().remove("engineKind");
        assert!(serde_json::from_value::<Metric>(missing).is_err());
        let uncertainty = serde_json::json!({"engineSchemaVersion":1,"repeatTolerance":0.01,"nearThreshold":0.01});
        assert!(serde_json::from_value::<Uncertainty>(uncertainty.clone()).is_ok());
        let mut legacy = uncertainty;
        legacy["confidenceLevel"] = serde_json::json!(0.95);
        assert!(serde_json::from_value::<Uncertainty>(legacy).is_err());
    }
}
