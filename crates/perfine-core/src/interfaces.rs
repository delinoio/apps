// SPDX-License-Identifier: Apache-2.0
//! Extension interfaces only. A capability declaration is not verification evidence.
use crate::{
    evidence::{Manifest, RunIdentity, VerifiedRun},
    lifecycle::Workspace,
    types::*,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    C,
    Cpp,
    Zig,
    Go,
    JavaScript,
    Extension,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityLevel {
    Execution,
    ProfileInterpretation,
    VerifiedOptimization,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub language: Language,
    pub level: CapabilityLevel,
    pub tools: BTreeMap<Name, Name>,
    pub metric: MetricSpec,
    pub source_mapping: bool,
    pub dependency_access: Vec<Name>,
    pub network_access: Vec<Name>,
    pub runtime_conditions: Vec<String>,
    pub verification_evidence: Vec<Fingerprint>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub language: Language,
    pub build_commands: Vec<Command>,
    pub correctness_commands: Vec<Command>,
    pub workload_commands: Vec<Command>,
}
/// Argument vectors avoid shell interpretation of source/user-controlled arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub executable: Name,
    pub arguments: Vec<String>,
}
pub trait LanguageAdapter {
    fn capabilities(&self) -> Capabilities;
    fn discover(&self, workspace: &Workspace) -> Result<Project>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancellation {
    Continue,
    GracefulStop,
    Immediate,
}
pub trait CancellationSignal {
    fn state(&self) -> Cancellation;
}
pub struct ExecutionContext<'a> {
    pub workspace: &'a Workspace,
    pub deadline_unix_seconds: u64,
    pub cancellation: &'a dyn CancellationSignal,
}
pub trait MeasurementAdapter {
    fn measure(
        &self,
        identity: &RunIdentity,
        conditions: &Conditions,
        context: &ExecutionContext<'_>,
    ) -> Result<Manifest>;
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: Name,
    pub line: Option<u32>,
    pub symbol: Option<Name>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileCost {
    pub source: SourceLocation,
    pub metric: MetricSpec,
    pub cost: f64,
    pub callees: Vec<SourceLocation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEdge {
    pub caller: SourceLocation,
    pub callee: SourceLocation,
    pub metric: MetricSpec,
    pub cost: f64,
    pub calls: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub edges: Vec<ProfileEdge>,
    pub costs: Vec<ProfileCost>,
    pub raw_artifacts: Vec<Fingerprint>,
    pub missing_source_mappings: Vec<Name>,
}
pub trait ProfileAdapter {
    fn interpret(&self, run: &VerifiedRun) -> Result<Profile>;
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hypothesis {
    pub explanation: Name,
    pub source_paths: BTreeSet<Name>,
    pub profile_evidence: Vec<Fingerprint>,
}
pub trait AgentAdapter {
    fn propose(&self, profile: &Profile, context: &ExecutionContext<'_>)
    -> Result<Vec<Hypothesis>>;
    fn implement(
        &self,
        hypothesis: &Hypothesis,
        context: &ExecutionContext<'_>,
    ) -> Result<CommitSha>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRole {
    Agent,
    Candidate,
    EvidenceUpload,
    Publication,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentRequest {
    pub role: ExecutionRole,
    pub base: CommitSha,
    pub workspace: Workspace,
    /// An explicit allowlist; never copy the parent environment wholesale.
    pub environment: BTreeMap<Name, String>,
    pub dependency_access: Vec<Name>,
    pub network_access: Vec<Name>,
}
pub trait EnvironmentAdapter {
    /// Reject dirty input; reserve separate paths; exclude agent/publication credentials
    /// from Candidate environments. Do not modify uncommitted user files.
    fn prepare(&self, request: &EnvironmentRequest) -> Result<()>;
    fn execute(
        &self,
        command: &Command,
        request: &EnvironmentRequest,
        context: &ExecutionContext<'_>,
    ) -> Result<CommandOutput>;
    /// Terminate candidate processes while retaining evidence and candidate branches.
    fn cancel(&self, workspace: &Workspace) -> Result<()>;
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout_artifact: Fingerprint,
    pub stderr_artifact: Fingerprint,
}
